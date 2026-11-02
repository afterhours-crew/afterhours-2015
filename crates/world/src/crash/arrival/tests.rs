// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    replication::{
        ActorUpdate, Initial, Kind, entity,
        entity::creation::{Asset, Catalog},
        goto_waypoint,
        players::Request,
        sublevel,
    },
    sequences::Component,
};

fn join(players: &mut Players, connection: u8, persona: u64) -> Owner {
    let player = players
        .create(
            connection,
            persona,
            Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(connection, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(connection, persona, participant)
        .unwrap();
    Owner {
        connection,
        persona,
        participant,
    }
}

fn setup() -> (Players, Arrivals, Owner) {
    let mut players = Players::default();
    let scenes = sublevel::Content::new(vec![(
        701,
        sublevel::Profile::new(
            false,
            &[
                sublevel::Kind::Noop,
                sublevel::Kind::Rpc,
                sublevel::Kind::Rpc,
            ],
        )
        .unwrap(),
    )])
    .unwrap();
    let scene = sublevel::passive::creation(1, 701, Serial::new(8).unwrap(), &scenes).unwrap();
    players.create_scenes(&[1], vec![scene], &scenes).unwrap();
    let asset = Asset {
        bundle: 9,
        type_id: 77,
        local_index: 4,
    };
    let content = Content::new(
        vec![Catalog::new(9, &[(77, 6)]).unwrap()],
        vec![(asset, Layout::ControlledArrival.profile())],
    )
    .unwrap();
    let model = Arrivals::new(
        Definition {
            layout: Layout::ControlledArrival,
            asset,
            manager: Component {
                scene_key: 701,
                index: 2,
            },
            embedded_bus: 5,
            channel: Component {
                scene_key: 701,
                index: 1,
            },
            presentation: None,
        },
        content,
        700,
    )
    .unwrap();
    let owner = join(&mut players, 4, 400);
    (players, model, owner)
}

fn actor_state(players: &Players, owner: Owner) -> ActorUpdate {
    let id = players
        .owned_actor(owner.connection, owner.persona, owner.participant)
        .unwrap();
    let Update::Actor(state) = &players.objects().get(id).unwrap().current else {
        panic!("actor");
    };
    state.clone()
}

fn actor_id(players: &Players, owner: Owner) -> u16 {
    players
        .owned_actor(owner.connection, owner.persona, owner.participant)
        .unwrap()
}

fn wire_actor(record: &Record) {
    let bytes = record.encode().unwrap();
    assert_eq!(
        Record::decode(bytes.span(), Some(Kind::Actor))
            .unwrap()
            .record,
        *record
    );
}

#[test]
fn timer_release_preserves_live_race_route_and_waits_for_owned_completion() {
    let (mut players, mut model, owner) = setup();
    let route = players
        .start_player_route(
            owner.connection,
            owner.persona,
            owner.participant,
            goto_waypoint::Initial {
                selector: 0,
                serial: 1,
                allocation_serial: 0,
                waypoint: 0,
                word: 1,
                points: vec![[7f32.to_bits(); 3], [9f32.to_bits(); 3]],
                flags: [false; 2],
                radius: 10f32.to_bits(),
                progress_updates: true,
            },
            None,
        )
        .unwrap()
        .goals[0];
    let goal_before = players.objects().get(route).unwrap().clone();
    let actor_before = actor_state(&players, owner);
    let started = model.start(&mut players, owner, 100).unwrap();
    assert_eq!(started.len(), 2);
    let ghost = started[0].id;
    let Some(Initial::Entity { fields, .. }) = &started[0].initial else {
        panic!("sequence");
    };
    assert!(
        matches!(&fields[0], entity::Initial::SequenceRoot { participants, stopping: false, .. }
        if *participants == vec![owner.participant])
    );
    assert!(
        matches!(&fields[1], entity::Initial::RpcOptionalReference { reference: Some(p), .. }
        if *p == owner.participant)
    );
    assert!(matches!(&fields[2], entity::Initial::RpcReference { .. }));
    wire_actor(&started[1]);
    let mut controlled = actor_before.clone();
    controlled.flag_a = Some(true);
    assert_eq!(actor_state(&players, owner), controlled);
    assert!(model.start(&mut players, owner, 200).unwrap().is_empty());
    assert!(model.poll(&mut players, 799, 1).unwrap().is_empty());
    assert!(model.poll(&mut players, 800, 0).unwrap().is_empty());
    assert_eq!(actor_state(&players, owner), controlled);
    assert!(
        model
            .complete(
                &mut players,
                owner,
                Completed {
                    sequence: ghost,
                    participant: owner.participant
                }
            )
            .is_err()
    );
    let stopped = model.poll(&mut players, 805, 1).unwrap();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].stop.sequence, ghost);
    stopped[0].stop.encode().unwrap();
    wire_actor(stopped[0].actor.as_ref().unwrap());
    assert_eq!(actor_state(&players, owner), actor_before);
    assert_eq!(players.objects().get(route), Some(&goal_before));
    assert!(model.active(owner.participant));
    assert!(players.objects().get(ghost).is_some());
    assert!(model.poll(&mut players, 900, 1).unwrap().is_empty());
    let completion = Completed {
        sequence: ghost,
        participant: owner.participant,
    };
    assert_eq!(
        model.complete(&mut players, owner, completion).unwrap(),
        Some(ghost)
    );
    assert_eq!(
        model.complete(&mut players, owner, completion).unwrap(),
        None
    );
    assert!(model.completed(ghost));
    assert!(!model.active(owner.participant));
    assert!(players.objects().get(ghost).is_none());
    assert_eq!(actor_state(&players, owner), actor_before);
    assert_eq!(players.objects().get(route), Some(&goal_before));
}

#[test]
fn peer_capacity_and_existing_control_are_independent() {
    let (mut players, mut model, a) = setup();
    let b = join(&mut players, 5, 500);
    players
        .actor_arriving(b.connection, b.persona, b.participant)
        .unwrap();
    let a_start = model.start(&mut players, a, 0).unwrap()[0].id;
    let b_start = model.start(&mut players, b, 0).unwrap()[0].id;
    let before = players.objects().snapshot();
    assert!(
        model
            .complete(
                &mut players,
                b,
                Completed {
                    sequence: a_start,
                    participant: a.participant
                }
            )
            .is_err()
    );
    assert!(
        model
            .complete(
                &mut players,
                b,
                Completed {
                    sequence: a_start,
                    participant: b.participant
                }
            )
            .is_err()
    );
    assert_eq!(players.objects().snapshot(), before);
    let first = model.poll(&mut players, 700, 1).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].stop.sequence, a_start);
    assert_eq!(actor_state(&players, a).flag_a, Some(false));
    assert_eq!(actor_state(&players, b).flag_a, Some(true));
    let second = model.poll(&mut players, 701, 1).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].stop.sequence, b_start);
    assert!(
        second[0].actor.is_none(),
        "preserve an existing hold without redundant replication"
    );
    assert_eq!(actor_state(&players, b).flag_a, Some(true));
}

#[test]
fn overlapping_release_rolls_back_the_entire_due_poll() {
    let (mut players, mut model, a) = setup();
    let b = join(&mut players, 5, 500);
    model.start(&mut players, a, 0).unwrap();
    model.start(&mut players, b, 0).unwrap();
    let actor = actor_id(&players, b);
    players
        .set_actor_control(b.connection, b.persona, b.participant, actor, false)
        .unwrap();
    let before = players.objects().snapshot();
    assert!(model.poll(&mut players, 700, 2).is_err());
    assert_eq!(players.objects().snapshot(), before);
    assert_eq!(model.world_ms, 0);
    assert!(
        model
            .entries
            .values()
            .all(|e| matches!(e.phase, Phase::Running { .. }))
    );
    players
        .set_actor_control(b.connection, b.persona, b.participant, actor, true)
        .unwrap();
    assert_eq!(model.poll(&mut players, 700, 2).unwrap().len(), 2);
}

#[test]
fn invalid_bindings_owner_clock_and_deadline_leave_no_partial_creation() {
    let (players, model, owner) = setup();
    for (manager_index, start, clock, who) in [
        (20, 0, 0, owner),
        (2, u64::MAX, 0, owner),
        (2, 4, 5, owner),
        (
            2,
            0,
            0,
            Owner {
                persona: 401,
                ..owner
            },
        ),
    ] {
        let mut next = players.clone();
        let mut model = model.clone();
        model.definition.manager.index = manager_index;
        model.world_ms = clock;
        let before = next.objects().snapshot();
        assert!(model.start(&mut next, who, start).is_err());
        assert_eq!(next.objects().snapshot(), before);
        assert!(model.entries.is_empty());
        assert_eq!(model.world_ms, clock);
    }
    for duration in [0, super::super::MAX_DURATION_MS + 1] {
        assert!(Arrivals::new(model.definition, model.content.clone(), duration).is_err());
    }
    let mut wrong = model.definition;
    wrong.layout = Layout::Waiting;
    assert!(Arrivals::new(wrong, model.content, 700).is_err());
}

#[test]
fn stale_sequence_cannot_finish_a_new_lifetime_and_retirement_is_guarded() {
    let (mut players, mut model, owner) = setup();
    let first = model.start(&mut players, owner, 0).unwrap()[0].id;
    assert!(model.retire(owner.participant).is_err());
    model.poll(&mut players, 700, 1).unwrap();
    let completion = Completed {
        sequence: first,
        participant: owner.participant,
    };
    model.complete(&mut players, owner, completion).unwrap();
    let second = model.start(&mut players, owner, 800).unwrap()[0].id;
    assert_ne!(first, second);
    assert!(!model.claims(first, 0));
    assert!(!model.claims(second, 1));
    assert!(model.claims(second, 0));
    let before = players.objects().snapshot();
    assert!(model.complete(&mut players, owner, completion).is_err());
    assert!(model.poll(&mut players, 799, 1).is_err());
    assert_eq!(players.objects().snapshot(), before);
    assert!(model.active(owner.participant));
    model.poll(&mut players, 1500, 1).unwrap();
    model
        .complete(
            &mut players,
            owner,
            Completed {
                sequence: second,
                participant: owner.participant,
            },
        )
        .unwrap();
    model.retire(owner.participant).unwrap();
    assert!(!model.claims(second, 0));
}
