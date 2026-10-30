// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world::{
    crash::{self, Crashes},
    participants::Stage,
    replication::{
        entity::creation::{Asset, Catalog, Content as Entities},
        vehicle::{
            chassis::{Controls, Motion},
            movement::Sample,
        },
    },
    sequences::{Completed, Component, Owner},
};

fn setup() -> Harness {
    let mut h = ai::harness();
    ai::race(&mut h);
    let content = h.content.clone();
    restart::configure(&mut h);
    h.content = content;
    h.listener.events = Some(Arc::new(h.content.as_ref().clone().into()));
    // Presentation byte/lifetime tests live with the crash transition tests.
    h.listener.logic = None;
    let asset = Asset {
        bundle: 900,
        type_id: 77,
        local_index: 0,
    };
    h.listener.crashes = Some(
        Crashes::new(
            crash::Definition {
                asset,
                manager: Component {
                    scene_key: 996,
                    index: 2,
                },
                embedded_bus: 1,
            },
            Entities::new(
                vec![Catalog::new(900, &[(77, 1)]).unwrap()],
                vec![(asset, crash::profile())],
            )
            .unwrap(),
            4500,
        )
        .unwrap(),
    );
    h
}

fn owner(h: &Harness) -> Owner {
    Owner {
        connection: HOST_SELECTOR as u8,
        persona: h.listener.persona,
        participant: h.participant,
    }
}

fn observe(h: &mut Harness, revision: u32, seconds: f64, x: f32, signal: bool, physical_state: u8) {
    let motion = Motion {
        identity: seconds.to_bits(),
        revision,
        position: [x, 0., 0.],
        // Facing backward does not alter the race-directed recovery transform.
        quaternion: [0., 1., 0., 0.],
        velocity: [-20., 0., 0.],
        angular_velocity: [0.; 3],
    };
    let mut controls = Controls::new(4).unwrap();
    controls.flags[0] = signal;
    controls.counters[0] = physical_state;
    let sample = Sample {
        physics: motion.encode([0.; 3], &controls).unwrap(),
        flag: true,
    };
    let (header, body) = sample.encode().unwrap();
    let movement = frame::Movement {
        header: Some(header.span()),
        records: vec![frame::MovementRecord {
            id: h.car,
            component: 0,
            data: body.span(),
        }],
    };
    PlayerListener::observe_crashes(
        h.listener.crashes.as_mut().unwrap(),
        &h.listener.players,
        &h.listener.participants,
        &h.listener.world_cars,
        h.listener.persona,
        &movement,
        (&h.listener.event_state, h.listener.events.as_deref()),
    )
    .unwrap();
}

fn complete(h: &mut Harness, sequence: u16) -> Option<nfs_world::teleport::Command> {
    let body = Completed {
        sequence,
        participant: h.participant,
    }
    .encode()
    .unwrap();
    let (_, _, command) = PlayerListener::finish_crash_call(
        body.span(),
        h.listener.crashes.as_mut().unwrap(),
        &mut h.listener.players,
        &mut h.listener.participants,
        None,
        &mut h.listener.logic_ghosts,
        h.listener.persona,
        None,
        &h.listener.event_state,
    )
    .unwrap()
    .unwrap()
    .unwrap();
    command
}

fn crash(h: &mut Harness, base: u32, time: f64) -> (u16, crash::route::Key) {
    observe(h, base, time, 0., false, 0);
    observe(h, base + 1, time + 0.1, 1., false, 0);
    let key = h
        .listener
        .event_state
        .recovery_key(h.participant, h.car)
        .unwrap();
    // Airborne and stale reports cannot replace the last grounded sample.
    observe(h, base + 2, time + 0.2, 2., false, 1);
    observe(h, base + 1, time + 0.1, 3., false, 0);
    observe(h, base + 3, time + 0.3, 4., true, 0);
    // A clearing pulse before the next tick must not cancel event recovery.
    observe(h, base + 4, time + 0.4, 5., false, 0);
    h.listener
        .advance_crashes((time * 1000.) as u64 + 500)
        .unwrap();
    let sequence = h
        .listener
        .pending_entries
        .pop_back()
        .unwrap()
        .0
        .records
        .last()
        .unwrap()
        .id;
    assert_eq!(
        h.listener.participants.stage(h.participant),
        Some(Stage::Crashing)
    );
    (sequence, key)
}

fn finish(h: &mut Harness, now: u64) -> Delivered {
    h.listener.logic = Harness::new().listener.logic;
    h.run(now, vec![h.checkpoint(6, now as f64 / 1000.)]);
    h.run(now + 100, vec![h.checkpoint(7, (now + 100) as f64 / 1000.)])
}

fn no_crash(h: &mut Harness, revision: u32, now: u64) {
    observe(h, revision, now as f64 / 1000., 5., false, 0);
    observe(h, revision + 1, now as f64 / 1000. + 0.01, 6., true, 0);
    h.listener.advance_crashes(now + 10).unwrap();
    assert!(!h.listener.crashes.as_ref().unwrap().active(h.participant));
    assert_eq!(
        h.listener.participants.stage(h.participant),
        Some(Stage::FreeRoam)
    );
    assert!(h.listener.pending_entries.is_empty());
}

#[test]
fn collisions_through_outro_and_final_leave_cannot_replace_the_sequence_or_replay_later() {
    let mut h = setup();
    let game_mode = h.race().game_mode;
    finish(&mut h, 10000);
    let outro = h.race().sequence.unwrap().1;
    no_crash(&mut h, 100, 10200);
    assert_eq!(h.race().sequence.unwrap().1, outro);
    assert!(h.world().get(outro).is_some());
    h.run(10300, vec![logic(OUTRO_READY, outro)]);
    no_crash(&mut h, 102, 10400);
    h.run(10500, vec![h.completed(outro)]);
    assert!(h.world().get(outro).is_none());
    no_crash(&mut h, 104, 10600);
    h.run(10800, vec![]);
    no_crash(&mut h, 106, 10900);
    h.run(11000, vec![h.leave()]);
    assert!(h.world().get(game_mode).is_none());
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    assert!(!h.listener.event_suspended.contains(&h.participant));
    // The protected collision is consumed, not saved for free-roam admission.
    observe(&mut h, 108, 11.1, 7., true, 0);
    h.listener.advance_crashes(11100).unwrap();
    assert!(!h.listener.crashes.as_ref().unwrap().active(h.participant));
    // A fresh free-roam crash still works after the event completely leaves.
    observe(&mut h, 109, 11.2, 8., false, 0);
    observe(&mut h, 110, 11.3, 9., true, 0);
    h.listener.advance_crashes(11300).unwrap();
    assert_eq!(
        h.listener.participants.stage(h.participant),
        Some(Stage::Crashing)
    );
}

#[test]
fn a_latched_crash_yields_to_same_frame_finish_and_is_discarded_under_queue_pressure() {
    let mut h = setup();
    h.listener.logic = Harness::new().listener.logic;
    h.run(10000, vec![h.checkpoint(6, 10.)]);
    observe(&mut h, 100, 10.1, 6., true, 0);
    assert_eq!(h.listener.crashes.as_ref().unwrap().pending().len(), 1);
    h.listener
        .event_state
        .inputs
        .push_back(h.checkpoint(7, 10.2));
    // The actual network loop advances sequences before event inputs.
    h.listener.advance_crashes(10200).unwrap();
    assert_eq!(
        h.listener.participants.stage(h.participant),
        Some(Stage::FreeRoam)
    );
    assert_eq!(h.listener.crashes.as_ref().unwrap().pending().len(), 1);
    h.run(10200, vec![]);
    let outro = h.race().sequence.unwrap().1;
    h.listener.session_backlog = MAX_PENDING;
    h.listener.advance_crashes(10300).unwrap();
    assert!(h.listener.crashes.as_ref().unwrap().pending().is_empty());
    h.listener.session_backlog = 0;
    h.listener.advance_crashes(10400).unwrap();
    assert!(h.listener.pending_entries.is_empty());
    assert_eq!(h.race().sequence.unwrap().1, outro);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
}

#[test]
fn a_running_crash_finishes_before_outro_publication_without_delaying_finish_rank_or_credit() {
    let mut h = setup();
    let (crash, _) = crash(&mut h, 100, 10.);
    let crossed = finish(&mut h, 11000);
    assert!(!crossed.deleted().contains(&crash));
    assert!(!h.race().logics.contains_key(&Logic::Outro));
    assert!(!h.race().logics.contains_key(&Logic::Awards));
    assert!(h.race().sequence.is_none());
    assert!(h.race().pending_outro.is_some());
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    let result = h.race().finish.unwrap();
    let complete_count = h.listener.event_state.completed;
    h.run(11200, vec![h.checkpoint(7, 11.1)]);
    assert_eq!(h.listener.event_state.completed, complete_count);
    assert_eq!(complete(&mut h, crash), None);
    assert_eq!(
        h.listener.participants.stage(h.participant),
        Some(Stage::FreeRoam)
    );
    assert!(h.world().get(crash).is_none());
    // No partial publication or lost snapshot when finish output cannot fit.
    h.listener
        .pending
        .resize(MAX_PENDING - 1, packed(Vec::new(), Vec::new()));
    h.run(11250, vec![]);
    assert!(h.race().pending_outro.is_some());
    assert!(h.race().sequence.is_none());
    let shown = h.run(11300, vec![]);
    assert_eq!(shown.created().len(), 4);
    assert!(h.race().pending_outro.is_none());
    assert_eq!(h.race().finish.unwrap(), result);
    let outro = h.race().sequence.unwrap().1;
    assert_ne!(outro, crash);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    let repeated = Completed {
        sequence: crash,
        participant: h.participant,
    }
    .encode()
    .unwrap();
    let duplicate = PlayerListener::finish_crash_call(
        repeated.span(),
        h.listener.crashes.as_mut().unwrap(),
        &mut h.listener.players,
        &mut h.listener.participants,
        None,
        &mut h.listener.logic_ghosts,
        h.listener.persona,
        None,
        &h.listener.event_state,
    )
    .unwrap();
    assert!(matches!(duplicate, Some(None)));
    assert_eq!(h.race().sequence.unwrap().1, outro);
    no_crash(&mut h, 110, 11400);
    h.run(11500, vec![logic(OUTRO_READY, outro)]);
    h.run(11600, vec![h.completed(outro)]);
    h.run(11800, vec![]);
    h.run(11900, vec![h.leave()]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
}

#[test]
fn event_presentation_and_restart_handoffs_reject_crashes_for_only_their_participant() {
    for phase in [Phase::Intro, Phase::Countdown, Phase::Outro] {
        let mut h = setup();
        h.listener
            .event_state
            .entries
            .get_mut(&h.participant)
            .unwrap()
            .race
            .as_mut()
            .unwrap()
            .phase = phase;
        no_crash(&mut h, 100, 10000);
    }
    for exit in [
        Exit::Completed,
        Exit::Restart,
        Exit::Abandon,
        Exit::TimedOut,
    ] {
        let mut h = setup();
        h.listener
            .event_state
            .entries
            .get_mut(&h.participant)
            .unwrap()
            .race
            .as_mut()
            .unwrap()
            .exit = Some(exit);
        no_crash(&mut h, 100, 10000);
    }
    let mut h = setup();
    h.listener
        .event_state
        .entries
        .get_mut(&h.participant)
        .unwrap()
        .race = None;
    h.listener.event_state.restarts.insert(
        h.participant,
        super::super::restart::Instance::new(
            h.content.record_id(),
            h.car,
            nfs_content::events::restart::Destination {
                position: [0.; 3],
                basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            },
        ),
    );
    no_crash(&mut h, 100, 10000);
    assert!(h.listener.event_state.crash_allowed(h.participant + 1));
    let mut other = setup();
    crash(&mut other, 100, 10.);
    assert_eq!(
        other.listener.participants.stage(other.participant),
        Some(Stage::Crashing)
    );
}

#[test]
fn repeated_moving_recovery_keeps_identity_progress_and_opponents_then_finishes_once() {
    let mut h = setup();
    let gm = h.race().game_mode;
    let opponents = h
        .listener
        .event_state
        .delegated(&h.listener.players, h.listener.persona);
    let original = h.race().progress.clone().unwrap();
    for cycle in 0..2 {
        let (sequence, key) = crash(&mut h, 100 + cycle * 10, 10. + f64::from(cycle) * 10.);
        let command = complete(&mut h, sequence).unwrap();
        assert_eq!(command.position, [1., 0., 0.]);
        assert_eq!(command.basis[2], [1., 0., 0.]);
        assert_eq!(command.speed, 15.);
        assert!(
            h.listener
                .crashes
                .as_ref()
                .unwrap()
                .route_is_current(h.participant, Some(key))
        );
        assert_eq!(h.race().game_mode, gm);
        assert_eq!(
            h.race()
                .progress
                .as_ref()
                .unwrap()
                .participant(h.participant),
            original.participant(h.participant)
        );
        assert_eq!(
            h.listener
                .event_state
                .delegated(&h.listener.players, h.listener.persona),
            opponents
        );
        assert!(h.listener.scoring_state.pending_awards.is_empty());
        let o = owner(&h);
        let crashes = h.listener.crashes.as_mut().unwrap();
        assert!(
            !crashes.finish_relocation(o).unwrap(),
            "ACK before transmission cannot finish recovery"
        );
        crashes.relocation_sent(h.participant, h.car);
        assert!(crashes.finish_relocation(o).unwrap());
        assert!(!crashes.finish_relocation(o).unwrap());
        h.listener.participants.recovered(h.participant).unwrap();
    }
    h.listener.pending_entries.clear();
    h.listener.logic = Harness::new().listener.logic;
    h.run(31000, vec![h.checkpoint(6, 31.)]);
    assert_eq!(h.race().game_mode, gm);
    assert_eq!(
        h.race()
            .progress
            .as_ref()
            .unwrap()
            .participant(h.participant)
            .unwrap()
            .next,
        1
    );
    h.run(32000, vec![h.checkpoint(7, 32.)]);
    let credits = h.listener.scoring_state.pending_awards.len();
    assert_eq!(credits, 1);
    assert!(
        h.listener
            .event_state
            .recovery_key(h.participant, h.car)
            .is_none()
    );
    h.run(32100, vec![h.checkpoint(7, 32.)]);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), credits);
}

#[test]
fn finish_abandon_and_restart_remove_boost_before_completion_or_queued_delivery() {
    for exit in [Exit::Completed, Exit::Abandon, Exit::Restart] {
        for queued in [false, true] {
            let mut h = setup();
            let (sequence, _) = crash(&mut h, 100, 10.);
            let command = queued.then(|| complete(&mut h, sequence).unwrap());
            h.listener
                .event_state
                .entries
                .get_mut(&h.participant)
                .unwrap()
                .race
                .as_mut()
                .unwrap()
                .exit = Some(exit);
            if let Some(command) = command {
                h.listener.pending_entries.clear();
                h.listener.pending_rpcs.clear();
                h.listener.pending.clear();
                h.listener.pending_content.clear();
                h.listener.pending_teleports.push_back((0, command));
                assert_eq!(h.listener.due_teleport(20000).unwrap().speed, 0.);
            } else {
                assert!(complete(&mut h, sequence).is_none());
                assert_eq!(
                    h.listener.participants.stage(h.participant),
                    Some(Stage::FreeRoam)
                );
            }
        }
    }
}

#[test]
fn recovery_histories_are_session_local_and_a_new_game_mode_cannot_reuse_one() {
    let mut first = setup();
    let mut second = setup();
    let (sequence, key) = crash(&mut first, 100, 10.);
    assert!(
        second
            .listener
            .crashes
            .as_ref()
            .unwrap()
            .pending()
            .is_empty()
    );
    assert!(
        second
            .listener
            .crashes
            .as_ref()
            .unwrap()
            .event_relocation(second.participant, key)
            .is_none()
    );
    observe(&mut second, 100, 10., 2., false, 0);
    observe(&mut second, 101, 10.1, 3., true, 0);
    second.listener.advance_crashes(10500).unwrap();
    let other_sequence = second
        .listener
        .pending_entries
        .pop_back()
        .unwrap()
        .0
        .records
        .last()
        .unwrap()
        .id;
    let other = complete(&mut second, other_sequence).unwrap();
    assert_eq!(other.position, [2., 0., 0.]);
    let command = complete(&mut first, sequence).unwrap();
    assert_eq!(command.position, [1., 0., 0.]);
    let race = first
        .listener
        .event_state
        .entries
        .get_mut(&first.participant)
        .unwrap()
        .race
        .as_mut()
        .unwrap();
    race.game_mode += 1;
    let current = first
        .listener
        .event_state
        .recovery_key(first.participant, first.car);
    assert!(
        !first
            .listener
            .crashes
            .as_ref()
            .unwrap()
            .route_is_current(first.participant, current)
    );
    assert!(
        first
            .listener
            .event_state
            .recovery_key(first.participant, first.car + 1)
            .is_none()
    );
}

#[test]
fn route_sampling_excludes_only_the_native_airborne_state() {
    let mut h = setup();
    observe(&mut h, 100, 10., 0., false, 2);
    observe(&mut h, 101, 10.1, 2., true, 0);
    h.listener.advance_crashes(10500).unwrap();
    let sequence = h
        .listener
        .pending_entries
        .pop_back()
        .unwrap()
        .0
        .records
        .last()
        .unwrap()
        .id;
    let command = complete(&mut h, sequence).unwrap();
    assert_eq!(command.position, [0.; 3]);
    assert_eq!(command.speed, 15.);
}
