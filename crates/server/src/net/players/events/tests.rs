// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! The event lifecycle on the synthetic event of `event_logic::tests`, driven by
//! constructed client calls; no captured bytes.
use super::*;
use crate::event_logic::tests as fixture;
use nfs_world::{bits::BitWriter, logic::EntityRef};
use std::sync::Arc;

mod ai;
mod catalog;
mod recovery;
mod restart;
mod speedwall;

struct Harness {
    listener: PlayerListener,
    participant: u16,
    car: u16,
    content: Arc<event_logic::Content>,
}
impl Harness {
    fn new() -> Self {
        let (mut players, _, participant) = fixture::world();
        players
            .create_actor(HOST_SELECTOR as u8, fixture::PERSONA, participant)
            .unwrap();
        let mut listener = PlayerListener::new(fixture::PERSONA);
        listener.players = players;
        let content = Arc::new(fixture::content());
        let humans = listener.players.player_participants().count();
        content
            .invitation_update(&mut listener.players, humans)
            .unwrap();
        listener.events = Some(Arc::new(content.as_ref().clone().into()));
        listener.replication_origin_ms = Some(0);
        let car = 4000;
        listener.world_cars.insert(car, (participant, true));
        let thresholds = nfs_services::reputation::Thresholds::new(vec![0, 5000]).unwrap();
        let mut presentation = crate::garage_logic::tests::presentation_json();
        // The event harness has its own persistent scene, separate from the
        // garage fixture. Route synthetic fade events to that live object.
        for phase in ["in_world_teleport", "in_world_arrival"] {
            for event in presentation[phase]["events"].as_array_mut().unwrap() {
                event["target"] = serde_json::json!({"scene_content_key": fixture::RACE});
            }
        }
        listener.logic = Some(Arc::new(
            crate::garage_logic::GarageLogic::from_json_with_reputation(
                &presentation,
                Some(thresholds),
            )
            .unwrap(),
        ));
        let mut state = listener.event_state.clone();
        let mut out = Out::default();
        listener
            .event_join(
                &content,
                &listener.players,
                &mut state,
                participant,
                &mut out,
            )
            .unwrap();
        listener.event_state = state;
        listener.pending_rpcs.extend(out.rpcs);
        Self {
            listener,
            participant,
            car,
            content,
        }
    }
    fn world(&self) -> &nfs_world::replication::state::World {
        self.listener.players.objects()
    }
    fn selector(&self, serializer: usize) -> u16 {
        self.world()
            .scene_endpoint(fixture::RACE, serializer)
            .unwrap()
            .1
    }
    fn scene(&self) -> u16 {
        self.world().scene(fixture::RACE).unwrap()
    }
    /// The in-event flag calls with `method`, in request order.
    fn flags(&self, method: u32) -> Vec<(u16, u16, u32, Option<u16>)> {
        fixture::FLAGS
            .iter()
            .map(|&(key, serializer)| {
                let (scene, selector) = self.world().scene_endpoint(key, serializer).unwrap();
                (scene, selector, method, None)
            })
            .collect()
    }
    fn actor(&self) -> u16 {
        self.listener
            .players
            .owned_actor(HOST_SELECTOR as u8, fixture::PERSONA, self.participant)
            .unwrap()
    }
    /// Deliver inputs at `now` and take everything queued for the session.
    fn run(&mut self, now: u64, inputs: Vec<Input>) -> Delivered {
        self.listener.event_state.inputs.extend(inputs);
        self.listener
            .advance_events(now)
            .unwrap_or_else(|error| panic!("advance_events at {now}: {error:?}"));
        Delivered {
            sections: self.listener.pending.drain(..).collect(),
            rpcs: self.listener.pending_rpcs.drain(..).collect(),
            entries: self.listener.pending_entries.drain(..).collect(),
        }
    }
    fn race(&self) -> &Race {
        self.listener.event_state.entries[&self.participant]
            .race
            .as_ref()
            .unwrap()
    }
    fn start_crossing(&self, method: u32) -> Input {
        let t = 12.5f64.to_bits();
        rpc(
            &[self.scene(), self.participant],
            self.selector(2),
            method,
            &[(t >> 32, 32), (t & 0xffff_ffff, 32), (0, 32), (0, 7)],
        )
    }
    fn checkpoint(&self, serializer: usize, seconds: f64) -> Input {
        let t = seconds.to_bits();
        rpc(
            &[self.scene(), self.car],
            self.selector(serializer),
            2,
            &[(t >> 32, 32), (t & 0xffff_ffff, 32), (0, 7)],
        )
    }
    fn completed(&self, sequence: u16) -> Input {
        rpc(&[sequence, self.participant], 0, 1, &[(0, 7)])
    }
    fn leave(&self) -> Input {
        rpc(&[self.race().game_mode, self.participant], 0, 0, &[(0, 7)])
    }
}
struct Delivered {
    sections: Vec<Section>,
    rpcs: Vec<HostRpc>,
    entries: Vec<(Section, Vec<HostRpc>)>,
}
impl Delivered {
    fn created(&self) -> Vec<u16> {
        self.sections
            .iter()
            .chain(self.entries.iter().map(|e| &e.0))
            .flat_map(|s| s.records.iter().map(|r| r.id))
            .collect()
    }
    fn deleted(&self) -> Vec<u16> {
        self.sections
            .iter()
            .chain(self.entries.iter().map(|e| &e.0))
            .flat_map(|s| s.deleted.clone())
            .collect()
    }
    fn calls(&self) -> Vec<(u16, u16, u32, Option<u16>)> {
        self.rpcs
            .iter()
            .filter_map(|r| match r {
                HostRpc::GameMode(c) => Some((c.target, c.selector, c.method, c.reference)),
                _ => None,
            })
            .collect()
    }
}
fn rpc(refs: &[u16], selector: u16, method: u32, args: &[(u64, usize)]) -> Input {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    for (v, n) in args {
        payload.put(*v, *n);
    }
    let mut body = BitWriter::new();
    body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
    for r in refs {
        body.put((*r).into(), 13);
    }
    body.put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    PlayerListener::event_input_rpc(body.span())
}
fn logic(event: u32, ghost: u16) -> Input {
    Input::Logic(Message::Reached {
        event,
        target: EntityRef { ghost, entity: 1 },
        player: 0,
    })
}

#[test]
fn invitation_initialization_retries_after_queue_pressure_and_tracks_human_joins() {
    use nfs_world::replication::{Initial, players::Request, sublevel};
    let (players, _, _) = fixture::world();
    let mut listener = PlayerListener::new(fixture::PERSONA);
    listener.players = players;
    listener.events = Some(Arc::new(fixture::content().into()));
    listener.replication_origin_ms = Some(0);
    let scene = listener.players.objects().scene(fixture::EVENTS).unwrap();
    let value = |listener: &PlayerListener| {
        let Initial::SubLevel { fields, .. } =
            &listener.players.objects().get(scene).unwrap().initial
        else {
            panic!("scene")
        };
        fields[3].clone()
    };
    listener.pending = std::iter::repeat_n(returning::empty_section(), MAX_PENDING).collect();
    listener.advance_events(1000).unwrap();
    assert_eq!(value(&listener), sublevel::Initial::BoolProperty(None));
    listener.pending.clear();
    listener.advance_events(1100).unwrap();
    assert_eq!(
        value(&listener),
        sublevel::Initial::BoolProperty(Some(false))
    );
    let delivered = listener.pending.pop_front().unwrap();
    assert_eq!(delivered.records.len(), 1);
    assert_eq!(delivered.records[0].id, scene);
    assert!(listener.pending.is_empty());
    listener.advance_events(1200).unwrap();
    assert!(listener.pending.is_empty());

    let player = listener
        .players
        .create(
            2,
            88,
            Request {
                name: b"Other".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    listener.players.join(2, 88, player).unwrap().unwrap();
    assert_eq!(listener.players.player_participants().count(), 2);
    listener.advance_events(1300).unwrap();
    assert_eq!(
        value(&listener),
        sublevel::Initial::BoolProperty(Some(true))
    );
    assert_eq!(listener.pending.pop_front().unwrap().records[0].id, scene);
}

/// From the start area to the countdown's end; returns the harness at the race.
fn started() -> Harness {
    let mut h = Harness::new();
    let joined: Vec<HostRpc> = h.listener.pending_rpcs.drain(..).collect();
    // Eligibility notifications, the start-volume subscription, the free-roam
    // notification.
    assert!(matches!(
        joined[2],
        HostRpc::WorldActivation(world_activation::Notification {
            call: world_activation::Call::Subscribe {
                token: 0,
                inside: false
            },
            ..
        })
    ));
    assert_eq!(joined.len(), 4);
    let entered = h.run(1_000, vec![h.start_crossing(2)]);
    assert_eq!(
        entered.created().len(),
        5,
        "three start-area logics and two filters"
    );
    let area = h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()]
        .area
        .clone();
    assert_eq!(area.len(), 3);
    // PageDown on the start prompt.
    let start = h.run(2_000, vec![logic(START, area[1])]);
    assert_eq!(
        start.created().len(),
        4,
        "game mode, sound, intro logic, intro sequence"
    );
    assert_eq!(start.deleted(), area);
    assert_eq!(
        start.calls(),
        h.flags(0)[..3],
        "Events and UI flags at once"
    );
    assert!(h.listener.event_suspended.contains(&h.participant));
    let gm = h.race().game_mode;
    let followed = h.run(2_100, vec![]);
    assert_eq!(
        followed.created().len(),
        9,
        "scene and game-mode logics, two goals and the Actor"
    );
    let goals = h.race().goals.clone();
    assert_eq!(goals.len(), 2);
    let records: Vec<_> = followed.sections.iter().flat_map(|s| &s.records).collect();
    assert!(matches!(
        records[6].initial,
        Some(nfs_world::replication::Initial::GotoWaypointGoal(ref g))
            if g.points == vec![fixture::GOTO.point.map(f32::to_bits)]
                && g.radius == 150f32.to_bits()
    ));
    assert!(matches!(
        records[7].initial,
        Some(nfs_world::replication::Initial::SpeedLimitGoal(ref g))
            if g.limit == 22.35f32.to_bits()
    ));
    assert_eq!(records[8].id, h.actor());
    assert!(matches!(
        &records[8].update,
        nfs_world::replication::Update::Actor(a)
            if a.flag_a == Some(true) && a.references_a.as_deref() == Some(goals.as_slice())
    ));
    let calls = followed.calls();
    assert_eq!(
        calls[0],
        (h.scene(), h.selector(1), 4, Some(gm)),
        "launcher learns its game mode"
    );
    assert_eq!(
        &calls[1..],
        &[
            (h.scene(), h.selector(6), 0, None),
            (h.scene(), h.selector(7), 0, None)
        ]
    );
    // The intro plays: Stop once, then the completion.
    let intro = h.race().sequence.unwrap().1;
    let stop = h.run(
        3_000,
        vec![logic(INTRO_READY, intro), logic(INTRO_READY, intro)],
    );
    assert_eq!(stop.rpcs.len(), 3);
    assert!(
        matches!(stop.rpcs[0], HostRpc::SequenceStop(Stop { sequence, .. }) if sequence == intro)
    );
    assert_eq!(stop.calls(), h.flags(0)[3..], "the delayed flags");
    let counting = h.run(3_100, vec![h.completed(intro)]);
    assert_eq!(
        counting.created(),
        vec![h.actor(), h.race().sequence.unwrap().1],
        "the Actor's release and the countdown sequence"
    );
    assert!(counting.deleted().contains(&intro));
    assert!(goals.iter().all(|g| counting.deleted().contains(g)));
    assert!(h.race().goals.is_empty());
    assert!(h.world().get(goals[0]).is_none());
    let calls = counting.calls();
    let countdown = h.content.state(Gm::Countdown) as u16;
    assert_eq!(calls[0], (gm, countdown, 0, Some(h.participant)));
    assert_eq!(calls[2].2, 2, "countdown start time");
    let HostRpc::GameMode(start_call) = counting.rpcs[2] else {
        panic!("start call")
    };
    assert_eq!(
        start_call.arguments,
        Arguments::time(3.1 + COUNTDOWN_LEAD_S)
    );
    let follow = h.run(3_200, vec![]);
    assert_eq!(follow.created().len(), 3);
    assert!(matches!(follow.rpcs.last(), Some(HostRpc::SequenceStop(_))));
    let countdown_ghost = h.race().sequence.unwrap().1;
    let done = h.run(3_300, vec![h.completed(countdown_ghost)]);
    assert_eq!(done.deleted(), vec![countdown_ghost]);
    let racing = h.run(3_400, vec![]);
    assert_eq!(racing.created().len(), 1, "driving-style logic");
    let waypoints = h.content.state(Gm::Waypoints) as u16;
    assert!(
        racing
            .calls()
            .contains(&(gm, waypoints, 0, Some(h.participant)))
    );
    assert_eq!(h.race().phase, Phase::Racing);
    assert_eq!(h.race().deadline, Some(3.4 + 80.));
    h
}

#[test]
fn a_win_runs_the_official_order_and_pays_once() {
    let mut h = started();
    let gm = h.race().game_mode;
    // Out-of-order and outside crossings are ignored.
    let wrong = h.run(5_000, vec![h.checkpoint(7, 20.)]);
    assert!(wrong.rpcs.is_empty());
    let first = h.run(6_000, vec![h.checkpoint(6, 30.)]);
    let HostRpc::GameMode(progress) = first.rpcs[0] else {
        panic!("progress")
    };
    assert_eq!(
        progress.arguments,
        Arguments::Progress {
            passed: 0,
            next: 1,
            seconds: ((30. - (3.1 + COUNTDOWN_LEAD_S)) as f32).to_bits(),
            lap: 1
        }
    );
    // The last checkpoint finishes: results, outro and one durable credit.
    let finish = h.run(7_000, vec![h.checkpoint(7, 60.)]);
    let HostRpc::GameMode(last) = finish.rpcs[0] else {
        panic!("progress")
    };
    assert!(matches!(
        last.arguments,
        Arguments::Progress {
            passed: 1,
            next: 0,
            lap: 2,
            ..
        }
    ));
    assert_eq!(
        finish.created().len(),
        4,
        "add-set, results, awards, outro sequence"
    );
    assert_eq!(finish.deleted().len(), 6);
    let awards: Vec<_> = h.listener.scoring_state.pending_awards.iter().collect();
    let [award] = awards.as_slice() else {
        panic!("one credit")
    };
    let event = award.event.as_ref().unwrap();
    assert_eq!(
        (
            event.event_id,
            event.kind.as_str(),
            event.position,
            event.cash,
            event.rep.total
        ),
        (1_761_118_161, "SpeedWall_SprintRace", 0, 500, 1500)
    );
    assert_eq!(event.batch_id, award.batch_id);
    assert_eq!(award.credit.total, 1500);
    assert!(
        h.race().finish.is_some_and(|f| f.race_ms
            == ((60. - (3.1 + COUNTDOWN_LEAD_S)) as f32 as f64 * 1000.).round() as i32)
    );
    let calls = h.run(7_200, vec![]).calls();
    let finish_state = h.content.state(Gm::Finish) as u16;
    assert!(calls.contains(&(gm, finish_state, 0, Some(h.participant))));
    // A repeated last crossing pays nothing more.
    h.run(7_300, vec![h.checkpoint(7, 61.)]);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    // The outro plays and completes: scoring resumes, the cooldown appears.
    let outro = h.race().sequence.unwrap().1;
    h.run(8_000, vec![logic(OUTRO_READY, outro)]);
    let ended = h.run(8_100, vec![h.completed(outro)]);
    assert!(ended.deleted().contains(&outro));
    assert_eq!(ended.created().len(), 1, "cooldown logic");
    assert!(!h.listener.event_suspended.contains(&h.participant));
    let ending = h.run(8_300, vec![]).calls();
    assert_eq!(
        ending[ending.len() - 4],
        (gm, 0, 1, None),
        "root method 1 asks the client to leave"
    );
    assert_eq!(
        ending[ending.len() - 3..],
        h.flags(1)[..3],
        "flags clear with it"
    );
    // The client leaves: final calls precede the game mode's deletion.
    let left = h.run(8_400, vec![h.leave()]);
    let (section, before) = &left.entries[0];
    assert!(section.deleted.contains(&gm));
    let mut calls: Vec<_> = left.calls();
    calls.extend(before.iter().filter_map(|r| match r {
        HostRpc::GameMode(c) => Some((c.target, c.selector, c.method, c.reference)),
        _ => None,
    }));
    assert_eq!(calls[0], (gm, 0, 4, Some(h.participant)));
    assert!(
        calls.contains(&(h.scene(), h.selector(6), 1, None)),
        "checkpoints disabled"
    );
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert!(h.listener.players.objects().get(gm).is_none());
    // The delayed flags clear after the leave; the cooldown goes after its lifetime.
    assert_eq!(h.run(9_200, vec![]).calls(), h.flags(1)[3..]);
    let cooled = h.run(8_100 + COOLDOWN_MS, vec![]);
    assert_eq!(cooled.deleted().len(), 1);
}

#[test]
fn selected_award_is_presented_and_credited_once_with_the_finish() {
    let mut h = started();
    h.content = Arc::new((*h.content).clone().with_awards(fixture::awards()).unwrap());
    h.listener.events = Some(Arc::new(h.content.as_ref().clone().into()));
    h.run(6_000, vec![h.checkpoint(6, 30.)]);
    h.run(7_000, vec![h.checkpoint(7, 60.)]);
    let pending = &h.listener.scoring_state.pending_awards;
    assert_eq!(pending.len(), 1);
    let award = pending.front().unwrap();
    assert_eq!(award.credit.total, 2500);
    assert_eq!(award.event.as_ref().unwrap().rep.total, 2500);
    let batch = award.batch_id;
    let selected = h
        .content
        .evaluate_awards(
            h.participant,
            &[(u64::from(h.participant), BTreeMap::new())],
        )
        .unwrap()
        .unwrap();
    assert_eq!(selected.fields, BTreeMap::from([(2, 1000)]));
    let outro = h
        .content
        .game_mode_logic(
            h.content.logic(Owner::GameMode, names::OUTRO, 0).unwrap(),
            h.world(),
            h.participant,
            h.race().game_mode,
            Some(event_logic::Results {
                finish: h.race().finish.unwrap(),
                properties: &BTreeMap::new(),
                awards: Some(&selected),
            }),
        )
        .unwrap();
    assert!(outro.body.initial.as_ref().unwrap().contains(
        &nfs_world::replication::entity::Initial::I32Property(Some(2500))
    ));
    let layout = h.content.logic(Owner::GameMode, names::AWARDS, 0).unwrap();
    let creation = h
        .content
        .game_mode_logic(
            layout,
            h.world(),
            h.participant,
            h.race().game_mode,
            Some(event_logic::Results {
                finish: h.race().finish.unwrap(),
                properties: &BTreeMap::new(),
                awards: Some(&selected),
            }),
        )
        .unwrap();
    assert_eq!(
        creation.body.initial.as_ref().unwrap()[2],
        nfs_world::replication::entity::Initial::I32Property(Some(1000))
    );
    h.run(7_300, vec![h.checkpoint(7, 61.)]);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    assert_eq!(
        h.listener
            .scoring_state
            .pending_awards
            .front()
            .unwrap()
            .batch_id,
        batch
    );
}

#[test]
fn restart_and_leave_choices_exit_through_the_dnf_transition() {
    for (choice, exit) in [(RESTART, Exit::Restart), (LEAVE, Exit::Abandon)] {
        let mut h = started();
        if choice == RESTART {
            restart::configure(&mut h);
        }
        let gm = h.race().game_mode;
        let target = h.race().logics[&if choice == RESTART {
            Logic::Restart
        } else {
            Logic::Leave
        }];
        let chosen = h.run(5_000, vec![logic(choice, target)]);
        assert_eq!(h.race().exit, Some(exit));
        assert!(chosen.deleted().contains(&target));
        assert_eq!(
            chosen.created().len(),
            if choice == RESTART { 5 } else { 1 },
            "cooldown plus the restart's four loading roles"
        );
        assert_eq!(
            h.listener.event_suspended.contains(&h.participant),
            exit == Exit::Restart
        );
        let calls = h.run(5_100, vec![]).calls();
        let triggered = h.content.state(Gm::DnfTriggered) as u16;
        assert_eq!(calls[0], (gm, triggered, 0, Some(h.participant)));
        assert_eq!(
            calls.contains(&(gm, 0, 3, Some(h.participant))),
            exit == Exit::Abandon
        );
        assert_eq!(calls[calls.len() - 4], (gm, 0, 1, None));
        assert_eq!(calls[calls.len() - 3..], h.flags(1)[..3]);
        let left = h.run(5_200, vec![h.leave()]);
        let HostRpc::GameMode(first) = left.entries[0].1[0] else {
            panic!("final call")
        };
        assert_eq!(
            (first.target, first.selector, first.method, first.reference),
            (gm, 0, 7, None)
        );
        assert!(left.entries[0].0.deleted.contains(&gm));
        assert!(
            h.listener.scoring_state.pending_awards.is_empty(),
            "no rewards"
        );
    }
}

#[test]
fn a_reached_dnf_deadline_uses_the_deadline_notification() {
    let mut h = started();
    let deadline = h.race().deadline.unwrap();
    h.run((deadline * 1000.) as u64 - 1, vec![]);
    assert_eq!(h.race().exit, None);
    h.run((deadline * 1000.) as u64, vec![]);
    assert_eq!(h.race().exit, Some(Exit::TimedOut));
    let calls = h.run((deadline * 1000.) as u64 + FOLLOW_MS, vec![]).calls();
    assert_eq!(
        calls[0],
        (
            h.race().game_mode,
            h.content.state(Gm::Dnf) as u16,
            1,
            Some(h.participant)
        )
    );
    assert!(h.listener.scoring_state.pending_awards.is_empty());
}

#[test]
fn leaving_free_roam_mid_race_removes_every_event_object() {
    let mut h = started();
    let race = h.race().clone();
    let mut state = h.listener.event_state.clone();
    let mut players = h.listener.players.clone();
    let mut suspended = h.listener.event_suspended.clone();
    let mut out = Out::default();
    h.listener
        .event_part(
            h.listener.events.as_deref().unwrap(),
            &mut players,
            &mut state,
            &mut suspended,
            h.participant,
            &mut out,
        )
        .unwrap();
    assert!(out.deleted.contains(&race.game_mode));
    for ghost in race.logics.values().chain(&race.scene) {
        assert!(out.deleted.contains(ghost));
    }
    assert!(matches!(
        out.rpcs[0],
        HostRpc::WorldActivation(world_activation::Notification {
            call: world_activation::Call::Unsubscribe,
            ..
        })
    ));
    let cleared: Vec<_> = out
        .rpcs
        .iter()
        .filter_map(|r| match r {
            HostRpc::GameMode(c) => Some((c.target, c.selector, c.method, c.reference)),
            _ => None,
        })
        .filter(|c| c.2 == 1)
        .collect();
    assert!(h.flags(1).iter().all(|f| cleared.contains(f)));
    assert!(!suspended.contains(&h.participant));
    assert!(!state.entries.contains_key(&h.participant));
    // A late start-volume report after the release changes nothing.
    h.listener.event_state = state;
    h.listener.players = players;
    let late = h.run(9_000, vec![h.start_crossing(2)]);
    assert!(late.sections.is_empty() && late.rpcs.is_empty());
}

#[test]
fn malformed_and_foreign_calls_change_nothing() {
    let mut h = Harness::new();
    h.listener.pending_rpcs.clear();
    // A start crossing with the wrong argument size, a stranger's crossing, a
    // logic event on an unknown object: ignored, the event stays enabled.
    let short = rpc(&[h.scene(), h.participant], h.selector(2), 2, &[(0, 7)]);
    let stranger = rpc(
        &[h.scene(), 7000],
        h.selector(2),
        2,
        &[(0, 32), (0, 32), (0, 32), (0, 7)],
    );
    let out = h.run(1_000, vec![short, stranger, logic(START, 7777)]);
    assert!(out.sections.is_empty() && out.rpcs.is_empty());
    assert!(h.listener.events.is_some());
    // Leaving the start area before PageDown deletes what entering created.
    let entered = h.run(1_100, vec![h.start_crossing(2)]);
    let left = h.run(1_200, vec![h.start_crossing(3)]);
    let mut created = entered.created();
    let mut deleted = left.deleted();
    created.sort();
    deleted.sort();
    assert_eq!(created, deleted);
    // A repeated outside report changes nothing.
    assert!(h.run(1_300, vec![h.start_crossing(3)]).sections.is_empty());
    assert!(
        !h.listener
            .event_claims_rpc(BitSpan::new(&[0u8; 16], 0, 128).unwrap())
    );
}

#[test]
fn an_unreported_intro_stops_after_the_fallback() {
    let mut h = Harness::new();
    h.listener.pending_rpcs.clear();
    h.run(1_000, vec![h.start_crossing(2)]);
    let area = h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()]
        .area
        .clone();
    h.run(2_000, vec![logic(START, area[1])]);
    h.run(2_100, vec![]);
    assert_eq!(h.run(3_000, vec![]).calls(), h.flags(0)[3..]);
    let intro = h.race().sequence.unwrap().1;
    assert!(h.run(2_000 + INTRO_FALLBACK_MS - 1, vec![]).rpcs.is_empty());
    assert_eq!(
        h.listener
            .observation(0, 0, 0, false)
            .health
            .event_intro_fallbacks,
        0
    );
    let stop = h.run(2_000 + INTRO_FALLBACK_MS, vec![]);
    assert!(
        matches!(stop.rpcs.as_slice(), [HostRpc::SequenceStop(Stop { sequence, .. })] if *sequence == intro)
    );
    // A late ready report after the fallback sends no second Stop.
    assert!(
        h.run(40_000, vec![logic(INTRO_READY, intro)])
            .rpcs
            .is_empty()
    );
    assert_eq!(
        h.listener
            .observation(40_000, 0, 0, false)
            .health
            .event_intro_fallbacks,
        1
    );
}

#[test]
fn a_restart_during_the_intro_releases_the_goals_and_drops_pending_flags() {
    let mut h = Harness::new();
    h.listener.pending_rpcs.clear();
    h.run(1_000, vec![h.start_crossing(2)]);
    let area = h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()]
        .area
        .clone();
    h.run(2_000, vec![logic(START, area[1])]);
    h.run(2_100, vec![]);
    let goals = h.race().goals.clone();
    assert_eq!(goals.len(), 2);
    // Restart before the delayed flags are set: the goals go with the Actor's
    // release, the immediate flags clear with the root, the pending sets drop.
    let restart = h.race().logics[&Logic::Restart];
    let chosen = h.run(2_200, vec![logic(RESTART, restart)]);
    assert!(goals.iter().all(|g| chosen.deleted().contains(g)));
    assert!(chosen.created().contains(&h.actor()));
    let exiting = h.run(2_300, vec![]).calls();
    assert_eq!(exiting[exiting.len() - 3..], h.flags(1)[..3]);
    let later = h.run(3_500, vec![]).calls();
    assert_eq!(later, h.flags(1)[3..], "no late set after the clear");
    assert!(
        h.listener
            .players
            .release_controlled_goals(HOST_SELECTOR as u8, fixture::PERSONA, h.participant, &goals)
            .is_err(),
        "released once"
    );
}

#[test]
fn a_participant_without_an_actor_still_races_without_goals() {
    let (players, _, participant) = fixture::world();
    let mut h = Harness::new();
    h.listener.players = players;
    h.listener.pending_rpcs.clear();
    assert_eq!(h.participant, participant);
    h.run(1_000, vec![h.start_crossing(2)]);
    let area = h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()]
        .area
        .clone();
    h.run(2_000, vec![logic(START, area[1])]);
    let followed = h.run(2_100, vec![]);
    assert_eq!(followed.created().len(), 6);
    assert!(h.race().goals.is_empty());
    assert!(h.listener.events.is_some());
}
