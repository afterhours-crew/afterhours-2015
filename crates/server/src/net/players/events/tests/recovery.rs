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
    h.listener.events = Some(h.content.clone());
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
