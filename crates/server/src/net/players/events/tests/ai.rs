// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::rival_content::population::tests as rivals;
use nfs_world::replication::{Initial, Update};

pub(super) fn harness() -> Harness {
    let (ai, mut world, mut registry) = rivals::event_fixture_with(HOST_SELECTOR as u8);
    let (rivals, _, _, messages) = rivals::fixture(HOST_SELECTOR as u8);
    let (participant, car) =
        crate::event_ai::tests::driver(&ai, &mut world, &mut registry, HOST_SELECTOR as u8);
    fixture::add_scenes(&mut world, 10);
    let content = Arc::new(fixture::ai_content(ai));
    let mut population = Population::new(&messages).unwrap();
    let before = population.registrations().clone();
    population.update_registrations(&before, registry).unwrap();
    let mut listener = PlayerListener::new(100);
    listener.players = world;
    listener.population = Some(population);
    listener.rival_population = Some(rivals.with_limit(Some(0)));
    listener.events = Some(Arc::new(content.as_ref().clone().into()));
    listener.replication_origin_ms = Some(0);
    listener.world_cars.insert(car, (participant, true));
    let mut state = State::default();
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
    Harness {
        listener,
        participant,
        car,
        content,
    }
}

fn start(h: &mut Harness) -> Delivered {
    h.run(1000, vec![h.start_crossing(2)]);
    let prompt =
        h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()].area[1];
    h.run(2000, vec![logic(START, prompt)])
}

pub(super) fn race(h: &mut Harness) {
    start(h);
    h.run(2100, vec![]);
    let intro = h.race().sequence.unwrap().1;
    h.run(3000, vec![logic(INTRO_READY, intro)]);
    h.run(3100, vec![h.completed(intro)]);
    h.run(3200, vec![]);
    let countdown = h.race().sequence.unwrap().1;
    h.run(3300, vec![h.completed(countdown)]);
    h.run(3400, vec![]);
}

fn checkpoint(h: &Harness, car: u16, serializer: usize, time: f64) -> Input {
    let bits = time.to_bits();
    rpc(
        &[h.scene(), car],
        h.selector(serializer),
        2,
        &[(bits >> 32, 32), (bits & 0xffff_ffff, 32), (0, 7)],
    )
}

#[test]
fn a_crash_keeps_the_race_opponents_and_scoring_until_driving_resumes() {
    use nfs_world::participants::Stage;
    let mut h = harness();
    race(&mut h);
    let race_content = h.content.clone();
    super::restart::configure(&mut h);
    // Reuse the lifecycle setup, retaining the racing fixture's navigation.
    h.content = race_content;
    h.listener.events = Some(Arc::new(h.content.as_ref().clone().into()));
    let p = h.participant;
    let game_mode = h.race().game_mode;
    let opponents = h
        .listener
        .event_state
        .delegated(&h.listener.players, h.listener.persona);
    let scoring = h.listener.desired_scoring(true);
    assert!(
        scoring[&p]
            .iter()
            .any(|(kind, _)| *kind == crate::scoring::Which::Event)
    );
    h.listener.participants.crashed(p).unwrap();
    h.listener.free_roam_presented.clear();
    assert_eq!(h.listener.participants.stage(p), Some(Stage::Crashing));
    let during = h.run(5000, vec![]);
    assert!(!during.deleted().contains(&game_mode));
    assert_eq!(h.race().game_mode, game_mode);
    assert_eq!(h.listener.desired_scoring(true), scoring);
    assert_eq!(h.listener.ai_focus().unwrap().len(), 1);
    assert_eq!(
        h.listener
            .event_state
            .delegated(&h.listener.players, h.listener.persona),
        opponents
    );
    assert!(h.listener.event_suspended.contains(&p));
    assert!(h.listener.scoring_state.pending_awards.is_empty());
    h.listener.participants.recovered(p).unwrap();
    h.listener.free_roam_presented.insert(p);
    h.run(5100, vec![]);
    assert_eq!(h.race().game_mode, game_mode);
    assert_eq!(h.listener.desired_scoring(true), scoring);
    // The next checkpoint still applies to the same race after recovery.
    let resumed = h.run(6000, vec![h.checkpoint(6, 30.)]);
    assert!(resumed.rpcs.iter().any(|call| matches!(call,
        HostRpc::GameMode(call) if matches!(call.arguments,
            Arguments::Progress { passed: 0, .. }))));
    assert_eq!(h.race().game_mode, game_mode);
    assert!(h.listener.scoring_state.pending_awards.is_empty());
}

#[test]
fn restart_populates_the_destination_while_rendering_is_still_loading() {
    let mut h = harness();
    race(&mut h);
    super::restart::configure(&mut h);
    let p = h.participant;
    assert_eq!(h.listener.ai_focus().unwrap().len(), 1);
    let target = h.race().logics[&Logic::Restart];
    h.run(5000, vec![logic(RESTART, target)]);
    h.run(5100, vec![]);
    h.run(5200, vec![h.leave()]);
    // Activation has retired the old free-roam presentation.
    h.listener.free_roam_presented.clear();
    assert!(h.listener.ai_focus().unwrap().is_empty());
    h.run(5300, vec![]);
    h.run(5400, vec![]);
    assert!(h.listener.ai_focus().unwrap().is_empty());
    let x = h.listener.participants.exit_bindings().unwrap();
    h.run(
        5500,
        vec![rpc(
            &[x.state_77.scene, p],
            x.state_77.selector,
            3,
            &[(0, 7)],
        )],
    );
    assert_eq!(h.listener.ai_focus().unwrap().len(), 1);
    assert!(h.listener.free_roam_presented.is_empty());
    assert_eq!(
        h.listener.participants.stage(p),
        Some(nfs_world::participants::Stage::Relocating)
    );
    assert!(h.listener.event_suspended.contains(&p));
}

#[test]
fn active_standings_update_but_finished_racers_stay_out_of_the_collector() {
    let mut h = harness();
    race(&mut h);
    let member = h.race().ai.as_ref().unwrap().racers.members[0];
    let game_mode = h.race().game_mode;
    let positions = |delivered: &Delivered| {
        delivered
            .rpcs
            .iter()
            .filter_map(|rpc| match rpc {
                HostRpc::GameMode(call) if call.target == game_mode => match call.arguments {
                    Arguments::EventPosition { position, active } => {
                        Some((call.reference.unwrap(), position, active))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(h.race().ai.as_ref().unwrap().positions.len(), 3);
    // Unchanged ranks do not resend the complete field every refresh.
    assert!(positions(&h.run(6000, vec![])).is_empty());

    h.run(6100, vec![checkpoint(&h, member.ids.vehicle, 6, 6.1)]);
    let finish = positions(&h.run(6200, vec![checkpoint(&h, member.ids.vehicle, 7, 6.2)]));
    assert_eq!(finish, vec![(member.ids.participant, 1, false)]);
    let later = positions(&h.run(9000, vec![]));
    assert!(
        later
            .iter()
            .all(|(id, position, active)| *id != member.ids.participant
                && *active
                && *position >= 2)
    );

    let human = h.participant;
    h.run(9100, vec![h.checkpoint(6, 9.1), h.checkpoint(7, 9.2)]);
    let human_finish = positions(&h.run(9300, vec![]));
    assert_eq!(human_finish, vec![(human, 2, false)]);
    assert!(
        positions(&h.run(12_000, vec![]))
            .iter()
            .all(|(id, _, _)| *id != human && *id != member.ids.participant)
    );
    assert_eq!(h.race().finish.unwrap().position, 2);
}

#[test]
fn overtakes_refresh_both_presentations_on_authored_cadence_and_retry_under_pressure() {
    use nfs_world::replication::vehicle::chassis::Controls;
    let mut h = harness();
    race(&mut h);
    let members = h.race().ai.as_ref().unwrap().racers.members.clone();
    let human = h.participant;
    let game_mode = h.race().game_mode;
    let local = h
        .content
        .state_call(
            h.world(),
            game_mode,
            Gm::LocalPosition,
            0,
            None,
            Arguments::Word(1),
        )
        .unwrap()
        .selector;
    let place = |h: &mut Harness, member: Option<usize>, x: f32| {
        let world = &mut h.listener.players;
        let physics = match member {
            Some(i) => {
                world.ai_vehicle_physics(HOST_SELECTOR as u8, 100, members[i].selection.car_key)
            }
            None => world.vehicle_physics(HOST_SELECTOR as u8, 100, h.car),
        }
        .unwrap();
        let mut motion = physics.decode([0.; 3]).unwrap();
        motion.position = [x, 0., 0.];
        let physics = motion.encode([0.; 3], &Controls::new(4).unwrap()).unwrap();
        match member {
            Some(i) => world.set_ai_vehicle_physics(
                HOST_SELECTOR as u8,
                100,
                members[i].selection.car_key,
                physics,
            ),
            None => world.set_vehicle_physics(HOST_SELECTOR as u8, 100, h.car, physics),
        }
        .unwrap();
    };
    let positions = |d: &Delivered| {
        d.rpcs
            .iter()
            .filter_map(|rpc| match rpc {
                HostRpc::GameMode(c) if c.target == game_mode => match c.arguments {
                    Arguments::EventPosition {
                        position,
                        active: true,
                    } => Some((c.reference.unwrap(), position)),
                    _ => None,
                },
                _ => None,
            })
            .collect::<BTreeMap<_, _>>()
    };
    let hud = |d: &Delivered| {
        d.rpcs
            .iter()
            .filter_map(|rpc| match rpc {
                HostRpc::GameMode(c) if c.target == game_mode && c.selector == local => {
                    match c.arguments {
                        Arguments::Word(v) => Some(v),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    place(&mut h, None, 4.);
    place(&mut h, Some(0), 2.);
    place(&mut h, Some(1), 6.);
    let baseline = h.run(4000, vec![]);
    assert_eq!(h.race().ai.as_ref().unwrap().positions[&human], 2);
    place(&mut h, None, 8.);
    let early = h.run(4299, vec![]);
    assert!(positions(&early).is_empty());
    assert!(hud(&early).is_empty());
    // A full queue cannot consume the refresh or mark unsent positions sent.
    let call = *baseline.rpcs.first().unwrap();
    h.listener.pending_rpcs =
        std::iter::repeat_n(call, nfs_world::session::MAX_RPC_QUEUE).collect();
    h.listener.advance_events(4300).unwrap();
    assert_eq!(h.race().ai.as_ref().unwrap().positions[&human], 2);
    h.listener.pending_rpcs.clear();
    let changed = h.run(4301, vec![]);
    assert_eq!(
        positions(&changed),
        BTreeMap::from([(human, 1), (members[1].ids.participant, 2)])
    );
    assert_eq!(hud(&changed), [1]);
    assert!(positions(&h.run(4601, vec![])).is_empty());
    // A failed road lookup for one opponent must not suppress the others.
    place(&mut h, Some(1), 5000.);
    place(&mut h, Some(0), 9.);
    let changed = h.run(4901, vec![]);
    assert_eq!(
        positions(&changed),
        BTreeMap::from([
            (human, 3),
            (members[0].ids.participant, 2),
            (members[1].ids.participant, 1),
        ])
    );
    assert_eq!(hud(&changed), [3]);
    // Returning to the road restores ordinary distance ranking.
    place(&mut h, Some(1), 1.);
    let changed = h.run(5201, vec![]);
    assert_eq!(
        positions(&changed),
        BTreeMap::from([
            (human, 2),
            (members[0].ids.participant, 1),
            (members[1].ids.participant, 3),
        ])
    );
    assert_eq!(hud(&changed), [2]);
}

#[test]
fn event_racers_join_the_location_map_after_creation_and_survive_adoption_once() {
    let mut h = harness();
    assert_eq!(
        h.listener
            .rival_map(&h.listener.players, &h.listener.intro_modes),
        Ok(Some(vec![]))
    );
    start(&mut h);
    assert_eq!(
        h.listener.players.player_participants().collect::<Vec<_>>(),
        vec![h.participant]
    );
    assert!(
        h.content
            .invitation_update(&mut h.listener.players, 1)
            .unwrap()
            .is_none()
    );
    let members = h.race().ai.as_ref().unwrap().racers.members.clone();
    let expected: BTreeMap<_, _> = members
        .iter()
        .map(|m| {
            (
                m.ids.participant,
                h.listener
                    .players
                    .ai_vehicle_physics(HOST_SELECTOR as u8, 100, m.selection.car_key)
                    .unwrap()
                    .decode([0.; 3])
                    .unwrap()
                    .position,
            )
        })
        .collect();
    let check = |h: &Harness| {
        let locations = h
            .listener
            .rival_map(&h.listener.players, &h.listener.intro_modes)
            .unwrap()
            .unwrap();
        assert_eq!(locations.len(), members.len());
        assert_eq!(locations.into_iter().collect::<BTreeMap<_, _>>(), expected);
    };
    check(&h);
    // Creation sections must reach the client before any map references.
    h.listener
        .pending_entries
        .push_back((returning::empty_section(), Vec::new()));
    assert_eq!(
        h.listener
            .rival_map(&h.listener.players, &h.listener.intro_modes),
        Ok(None)
    );
    h.listener.pending_entries.clear();
    check(&h);

    h.run(2100, vec![]);
    let intro = h.race().sequence.unwrap().1;
    h.run(3000, vec![logic(INTRO_READY, intro)]);
    h.run(3100, vec![h.completed(intro)]);
    h.run(3200, vec![]);
    let countdown = h.race().sequence.unwrap().1;
    h.run(3300, vec![h.completed(countdown)]);
    h.run(3400, vec![]);
    check(&h);
    let ai = members[0];
    h.run(4100, vec![checkpoint(&h, ai.ids.vehicle, 6, 4.1)]);
    h.run(4200, vec![checkpoint(&h, ai.ids.vehicle, 7, 4.2)]);
    check(&h);
    h.run(34_200, vec![]);
    assert_eq!(h.race().ai.as_ref().unwrap().racers.members.len(), 1);
    check(&h);
    // Abandon adopts the remaining opponent; neither marker disappears or
    // becomes a duplicate as the game mode is released.
    let leave = h.race().logics[&Logic::Leave];
    h.run(34_300, vec![logic(LEAVE, leave)]);
    check(&h);
    h.run(34_400, vec![]);
    h.run(34_500, vec![h.leave()]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    check(&h);
    assert_eq!(
        h.listener.event_state.locations(&h.listener.players, 100),
        Ok(vec![])
    );
}

#[test]
fn human_rewards_follow_finish_order_even_after_opponents_leave_the_game_mode() {
    for opponents_finished in 0..=2 {
        let mut h = harness();
        h.listener.logic = Harness::new().listener.logic;
        race(&mut h);
        let members = h.race().ai.as_ref().unwrap().racers.members.clone();
        for (i, member) in members.iter().take(opponents_finished).enumerate() {
            let time = 4. + i as f64;
            h.run(
                4000 + i as u64 * 1000,
                vec![
                    checkpoint(&h, member.ids.vehicle, 6, time),
                    checkpoint(&h, member.ids.vehicle, 7, time + 0.1),
                ],
            );
        }
        // Retire finished AIs before the human finishes. The payout comparison
        // still uses three original members, including the adopted racers.
        h.run(36_000, vec![]);
        assert!(h.listener.scoring_state.pending_awards.is_empty());
        h.run(37_000, vec![h.checkpoint(6, 37.), h.checkpoint(7, 37.1)]);
        let (cash, rep) = [(2000, 9000), (1000, 7000), (500, 1500)][opponents_finished];
        let awards = &h.listener.scoring_state.pending_awards;
        assert_eq!(awards.len(), 1);
        let award = &awards[0];
        let completion = award.event.as_ref().unwrap();
        assert_eq!(
            (completion.position, completion.cash, completion.rep.total),
            (opponents_finished as u32, cash, rep)
        );
        assert_eq!(award.credit.total, rep);
        assert_eq!(h.race().finish.unwrap().participants, 3);
        h.run(38_000, vec![h.checkpoint(7, 38.)]);
        assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    }
}

#[test]
fn grid_and_human_have_independent_progress_and_teardown_preserves_ai_cars() {
    let mut h = harness();
    race(&mut h);
    let members = h.race().ai.as_ref().unwrap().racers.members.clone();
    assert_eq!(members.len(), 2);
    assert_eq!(h.listener.delegated_ai(&h.listener.players).len(), 2);
    let group = h
        .race()
        .ai
        .as_ref()
        .unwrap()
        .commands
        .as_ref()
        .unwrap()
        .pacing()
        .unwrap();
    let ai = members[0];
    // A rival finishing first must not finish the human or pay their rewards.
    let wrong = h.run(4000, vec![checkpoint(&h, ai.ids.vehicle, 7, 4.)]);
    assert!(wrong.created().is_empty());
    h.run(4100, vec![checkpoint(&h, ai.ids.vehicle, 6, 4.1)]);
    let finished = h.run(4200, vec![checkpoint(&h, ai.ids.vehicle, 7, 4.2)]);
    assert_eq!(h.race().phase, Phase::Racing);
    assert!(h.listener.scoring_state.pending_awards.is_empty());
    assert!(finished.sections.iter().flat_map(|s| &s.records).any(
        |r| matches!(r.initial, Some(Initial::WanderGoal(ref w)) if w.flags == [false, true, true])
    ));
    assert_eq!(
        h.race()
            .progress
            .as_ref()
            .unwrap()
            .participant(ai.ids.participant)
            .unwrap()
            .finished,
        Some(1)
    );
    let snapshot = h.world().snapshot();
    h.run(4300, vec![checkpoint(&h, ai.ids.vehicle, 7, 4.2)]);
    assert_eq!(h.world().snapshot(), snapshot);
    h.run(4400, vec![h.checkpoint(6, 4.4)]);
    h.run(4500, vec![h.checkpoint(7, 4.5)]);
    assert_eq!(h.race().finish.unwrap().position, 2);
    assert_eq!(h.race().phase, Phase::Outro);
    assert_eq!(
        h.race()
            .ai
            .as_ref()
            .unwrap()
            .commands
            .as_ref()
            .unwrap()
            .members(),
        &[members[1].ids.participant]
    );
    h.run(4700, vec![]);
    let outro = h.race().sequence.unwrap().1;
    h.run(4800, vec![logic(OUTRO_READY, outro)]);
    h.run(4900, vec![h.completed(outro)]);
    h.run(5100, vec![]);
    h.run(5200, vec![h.leave()]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert!(h.world().get(group).is_none());
    let roaming = h.listener.rival_population.as_ref().unwrap().delegated(
        &h.listener.players,
        HOST_SELECTOR as u8,
        100,
    );
    for member in members {
        assert_eq!(
            roaming.get(&member.ids.vehicle),
            Some(&member.selection.car_key)
        );
        assert_eq!(
            h.listener
                .players
                .ai_ids(HOST_SELECTOR as u8, 100, member.selection.car_key),
            Some(member.ids)
        );
        let Update::Actor(actor) = &h.world().get(member.ids.actor).unwrap().current else {
            panic!()
        };
        let refs = actor.references_a.as_ref().unwrap();
        assert_eq!(refs.len(), 1);
        assert!(matches!(&h.world().get(refs[0]).unwrap().initial,
            Initial::WanderGoal(w) if w.flags == [false, false, true]));
    }
}

#[test]
fn opponent_outro_timeout_retries_atomically_and_releases_only_that_member() {
    let mut h = harness();
    race(&mut h);
    let members = h.race().ai.as_ref().unwrap().racers.members.clone();
    let ai = members[0];
    h.run(4100, vec![checkpoint(&h, ai.ids.vehicle, 6, 4.1)]);
    h.run(4200, vec![checkpoint(&h, ai.ids.vehicle, 7, 4.2)]);
    h.run(34_199, vec![]);
    assert_eq!(h.race().ai.as_ref().unwrap().racers.members.len(), 2);
    let call = HostRpc::GameMode(h.content.scene_call(h.world(), 6, 0, None).unwrap());
    h.listener.pending_rpcs =
        std::iter::repeat_n(call, nfs_world::session::MAX_RPC_QUEUE).collect();
    let snapshot = h.world().snapshot();
    h.listener.advance_events(34_200).unwrap();
    assert_eq!(h.world().snapshot(), snapshot);
    assert_eq!(h.race().ai.as_ref().unwrap().outros.len(), 1);
    assert!(
        h.listener
            .rival_population
            .as_ref()
            .unwrap()
            .delegated(&h.listener.players, HOST_SELECTOR as u8, 100,)
            .is_empty()
    );
    h.listener.pending_rpcs.clear();
    let delivered = h.run(34_200, vec![]);
    assert_eq!(h.race().phase, Phase::Racing);
    assert!(h.race().ai.as_ref().unwrap().outros.is_empty());
    assert_eq!(h.race().ai.as_ref().unwrap().racers.members.len(), 1);
    assert_eq!(
        h.listener
            .rival_population
            .as_ref()
            .unwrap()
            .delegated(&h.listener.players, HOST_SELECTOR as u8, 100,)
            .get(&ai.ids.vehicle),
        Some(&ai.selection.car_key)
    );
    assert_eq!(h.listener.delegated_ai(&h.listener.players).len(), 2);
    assert!(
        !h.listener
            .event_state
            .preserved(&h.listener.players, 100)
            .contains(&ai.ids.participant)
    );
    assert!(delivered.rpcs.iter().any(|rpc| matches!(rpc,
        HostRpc::GameMode(c) if c.target == h.race().game_mode
            && c.selector == 0 && c.method == 3 && c.reference == Some(ai.ids.participant))));
    assert!(!delivered.rpcs.iter().any(|rpc| matches!(rpc,
        HostRpc::GameMode(c) if c.target == h.race().game_mode
            && c.selector == 0 && c.method == 1)));
    assert!(h.listener.scoring_state.pending_awards.is_empty());
    let repeated = h.run(34_201, vec![]);
    assert!(repeated.rpcs.is_empty());
    h.run(34_300, vec![h.checkpoint(6, 34.3)]);
    h.run(34_400, vec![h.checkpoint(7, 34.4)]);
    assert_eq!(h.race().finish.unwrap().position, 2);
}

#[test]
fn opponent_dnf_uses_its_own_refreshed_deadline_without_a_finish_or_award() {
    let mut h = harness();
    race(&mut h);
    let members = h.race().ai.as_ref().unwrap().racers.members.clone();
    let deadline = h.race().ai.as_ref().unwrap().deadlines[&members[1].ids.participant];
    let due = (deadline * 1000.).ceil() as u64;
    // Only the human and the first opponent refresh their racing deadlines.
    h.run(
        10_000,
        vec![
            h.checkpoint(6, 10.),
            checkpoint(&h, members[0].ids.vehicle, 6, 10.),
        ],
    );
    h.run(due - 1, vec![]);
    assert_eq!(h.race().ai.as_ref().unwrap().racers.members.len(), 2);
    let expired = h.run(due, vec![]);
    assert_eq!(h.race().phase, Phase::Racing);
    let grid = h.race().ai.as_ref().unwrap();
    assert_eq!(grid.racers.members.len(), 1);
    assert_eq!(grid.racers.members[0].ids, members[0].ids);
    assert_eq!(
        grid.commands.as_ref().unwrap().members(),
        &[h.participant, members[0].ids.participant]
    );
    let withdrawn = h
        .race()
        .progress
        .as_ref()
        .unwrap()
        .participant(members[1].ids.participant)
        .unwrap();
    assert!(withdrawn.withdrawn);
    assert_eq!(withdrawn.finished, None);
    assert!(expired.rpcs.iter().any(|r| matches!(r,
        HostRpc::GameMode(c) if c.reference == Some(members[1].ids.participant)
        && c.arguments == Arguments::EventPosition { position: 0, active: false }
    )));
    let dnf = h
        .content
        .state_call(
            h.world(),
            h.race().game_mode,
            Gm::Dnf,
            1,
            Some(members[1].ids.participant),
            Arguments::None,
        )
        .unwrap();
    assert!(
        expired
            .rpcs
            .iter()
            .any(|r| matches!(r, HostRpc::GameMode(c) if *c == dnf))
    );
    assert!(h.listener.scoring_state.pending_awards.is_empty());
    // Reports from the adopted car no longer belong to this race.
    h.run(
        due + 1,
        vec![checkpoint(&h, members[1].ids.vehicle, 6, deadline)],
    );
    h.run(due + 2, vec![h.checkpoint(7, deadline)]);
    assert_eq!(h.race().finish.unwrap().position, 1);
}

#[test]
fn a_fault_preserves_delegation_until_cleanup_can_be_published() {
    let mut h = harness();
    race(&mut h);
    let gm = h.race().game_mode;
    let members = h.race().ai.as_ref().unwrap().racers.members.clone();
    let snapshot = h.world().snapshot();
    let call = HostRpc::GameMode(h.content.scene_call(h.world(), 6, 0, None).unwrap());
    h.listener.pending_rpcs =
        std::iter::repeat_n(call, nfs_world::session::MAX_RPC_QUEUE).collect();
    assert!(h.listener.disable_events());
    assert!(h.listener.event_suspended.contains(&h.participant));
    h.listener.advance_events(5000).unwrap();
    assert_eq!(h.world().snapshot(), snapshot);
    assert_eq!(h.listener.delegated_ai(&h.listener.players).len(), 2);
    assert!(h.listener.events.is_some());
    h.listener.pending_rpcs.clear();
    let cleanup = h.run(5000, vec![]);
    assert!(cleanup.deleted().contains(&gm));
    assert!(h.listener.events.is_none());
    assert!(h.listener.event_state.entries.is_empty());
    assert!(h.listener.event_suspended.is_empty());
    assert_eq!(h.listener.delegated_ai(&h.listener.players).len(), 2);
    for member in members {
        assert_eq!(
            h.listener
                .players
                .ai_ids(HOST_SELECTOR as u8, 100, member.selection.car_key),
            Some(member.ids)
        );
    }
    assert!(
        !h.listener.disable_events(),
        "a fault in teardown must close the session"
    );
}

#[test]
fn a_full_rpc_queue_retries_grid_creation_without_consuming_keys_or_world_ids() {
    let mut h = harness();
    h.run(1000, vec![h.start_crossing(2)]);
    let prompt =
        h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()].area[1];
    let call = HostRpc::GameMode(h.content.scene_call(h.world(), 6, 0, None).unwrap());
    h.listener.pending_rpcs =
        std::iter::repeat_n(call, nfs_world::session::MAX_RPC_QUEUE).collect();
    h.listener
        .event_state
        .inputs
        .push_back(logic(START, prompt));
    let snapshot = h.world().snapshot();
    let registrations = h
        .listener
        .population
        .as_ref()
        .unwrap()
        .registrations()
        .clone();
    h.listener.advance_events(2000).unwrap();
    assert_eq!(h.world().snapshot(), snapshot);
    assert_eq!(
        h.listener.population.as_ref().unwrap().registrations(),
        &registrations
    );
    assert_eq!(h.listener.event_state.inputs.len(), 1);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    h.listener.pending_rpcs.clear();
    let delivered = h.run(2000, vec![]);
    let members = &h.race().ai.as_ref().unwrap().racers.members;
    assert_eq!(members[0].selection.car_key, crate::event_ai::KEY_START);
    assert_eq!(h.listener.players.ai_count(), 2);
    assert!(
        delivered
            .sections
            .iter()
            .flat_map(|s| &s.records)
            .any(|r| r.id == members[0].ids.vehicle && r.initial.is_some())
    );
    assert!(
        delivered
            .rpcs
            .iter()
            .any(|r| matches!(r, HostRpc::Participant(_)))
    );
}
