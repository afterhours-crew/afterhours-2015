// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    logic::EntityRef,
    replication::{
        Initial,
        entity::{
            self,
            creation::{Asset, Catalog},
        },
        players::Request,
        sublevel,
    },
    sequences::Component,
};

fn setup() -> (Players, Arrivals) {
    let mut players = Players::default();
    let scenes = sublevel::Content::new(vec![
        (
            7,
            // Field 3 is the scene's third RPC: wire selector 2, as the
            // official Gameplay manager.
            sublevel::Profile::new(
                false,
                &[
                    sublevel::Kind::Noop,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                ],
            )
            .unwrap(),
        ),
        (
            8,
            sublevel::Profile::new(false, &[sublevel::Kind::Noop, sublevel::Kind::Rpc]).unwrap(),
        ),
    ])
    .unwrap();
    let creations = [7, 8].map(|key| {
        sublevel::passive::creation((key - 6) as u16, key, Serial::new(9).unwrap(), &scenes)
            .unwrap()
    });
    players
        .create_scenes(&[1, 2], creations.into(), &scenes)
        .unwrap();
    let asset = Asset {
        bundle: 16,
        type_id: 3424,
        local_index: 9,
    };
    let content = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(asset, profile())],
    )
    .unwrap();
    let definition = Definition {
        presentation: None,
        layout: crate::sequences::Layout::GarageExit,
        asset,
        manager: Component {
            scene_key: 7,
            index: 3,
        },
        embedded_bus: 2,
        channel: Component {
            scene_key: 8,
            index: 1,
        },
    };
    (players, Arrivals::new(definition, content).unwrap())
}

fn join(players: &mut Players) -> Owner {
    let player = players
        .create(
            4,
            400,
            Request {
                name: b"Local".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    Owner {
        connection: 4,
        persona: 400,
        participant: players.join(4, 400, player).unwrap().unwrap().id,
    }
}

fn reached(event: u32, ghost: u16, entity: u32) -> Message {
    Message::Reached {
        event,
        target: EntityRef { ghost, entity },
        player: 0,
    }
}

#[test]
fn timed_entry_repeats_with_new_ids_and_preserves_due_stop_under_backpressure() {
    let (mut players, original) = setup();
    let mut definition = original.definition;
    definition.layout = Layout::GarageEntry;
    definition.presentation = Some(definition.channel);
    let content = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(definition.asset, definition.layout.profile())],
    )
    .unwrap();
    let mut returns = Arrivals::returning(definition, content, 7500).unwrap();
    let owner = join(&mut players);
    let mut previous = None;
    for i in 0..3 {
        returns.poll(i * 8000).unwrap();
        let record = returns.start(&mut players, owner).unwrap().unwrap();
        if let Some(id) = previous {
            assert_ne!(record.id, id);
        }
        assert!(returns.reset(&players, owner).is_err());
        assert!(
            returns
                .arrived(
                    &players,
                    owner.connection,
                    owner.persona,
                    &reached(ARRIVED, record.id, 1)
                )
                .unwrap()
                .is_none()
        );
        assert!(returns.poll(i * 8000 + 7499).unwrap().is_empty());
        assert!(
            returns
                .poll_capacity(i * 8000 + 7500, 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            returns.poll_capacity(i * 8000 + 7500, 1).unwrap()[0].sequence,
            record.id
        );
        let call = Completed {
            sequence: record.id,
            participant: owner.participant,
        };
        assert!(
            returns
                .complete(
                    &players,
                    Owner {
                        persona: owner.persona + 1,
                        ..owner
                    },
                    call
                )
                .is_err()
        );
        assert_eq!(
            returns.complete(&players, owner, call).unwrap(),
            Some(record.id)
        );
        assert!(returns.reset(&players, owner).is_err());
        returns.finish_rollout(&mut players, owner).unwrap();
        returns.reset(&players, owner).unwrap();
        assert!(returns.complete(&players, owner, call).is_err());
        previous = Some(record.id);
    }
}

#[test]
fn rollout_goal_is_owned_atomic_ordered_and_cleaned_once() {
    let (mut players, arrivals) = setup();
    let mut arrivals = arrivals
        .with_spline(spline::Definition {
            asset: Asset {
                bundle: 23,
                type_id: 5100,
                local_index: 2,
            },
            catalog: Catalog::new(23, &[(5100, 3)]).unwrap(),
            flag: false,
        })
        .unwrap();
    let owner = join(&mut players);
    let before = players.objects().snapshot();
    // Missing Actor must not consume either the sequence or goal identity.
    assert_eq!(
        arrivals.start_rollout(&mut players, owner),
        Err(Error::UnknownObject)
    );
    assert_eq!(players.objects().snapshot(), before);
    assert_eq!(arrivals.ghost(owner.participant), None);
    let actor = players
        .create_actor(owner.connection, owner.persona, owner.participant)
        .unwrap()
        .unwrap()
        .id;
    let records = arrivals.start_rollout(&mut players, owner).unwrap();
    assert_eq!(records.len(), 3);
    let goal = records[0].id;
    assert!(matches!(
        records[0].initial,
        Some(Initial::ScriptedSplineGoal(_))
    ));
    assert_eq!(records[1].id, actor);
    assert!(
        matches!(&records[1].update,crate::replication::Update::Actor(a) if a.references_a.as_deref()==Some(&[goal]))
    );
    let sequence = records[2].id;
    assert_eq!(sequence + 1, goal);
    assert!(
        arrivals
            .start_rollout(&mut players, owner)
            .unwrap()
            .is_empty()
    );
    let second_player = players
        .create(
            5,
            500,
            Request {
                name: b"Second".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let second = Owner {
        connection: 5,
        persona: 500,
        participant: players.join(5, 500, second_player).unwrap().unwrap().id,
    };
    players.create_actor(5, 500, second.participant).unwrap();
    let other = arrivals.start_rollout(&mut players, second).unwrap();
    let snapshot = players.objects().snapshot();
    assert_eq!(
        players.finish_spline(5, 500, second.participant, goal),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        arrivals.finish_rollout(&mut players, owner),
        Err(Error::Shape)
    );
    assert_eq!(players.objects().snapshot(), snapshot);
    arrivals
        .arrived(
            &players,
            owner.connection,
            owner.persona,
            &reached(ARRIVED, sequence, 1),
        )
        .unwrap()
        .unwrap();
    let call = Completed {
        sequence,
        participant: owner.participant,
    };
    arrivals.complete(&players, owner, call).unwrap().unwrap();
    let (cleared, deleted) = arrivals.finish_rollout(&mut players, owner).unwrap();
    assert_eq!(deleted, vec![sequence, goal]);
    assert!(
        matches!(&cleared[0].update,crate::replication::Update::Actor(a) if a.flag_a == Some(false) && a.references_a.as_deref()==Some(&[]) && a.references_b.as_deref()==Some(&[]))
    );
    assert!(players.objects().get(goal).is_none() && players.objects().get(sequence).is_none());
    assert!(players.objects().get(other[0].id).is_some());
    assert_eq!(arrivals.complete(&players, owner, call), Ok(None));
    assert_eq!(
        arrivals.finish_rollout(&mut players, owner),
        Ok((vec![], vec![]))
    );
    assert_eq!(
        Arrivals::new(arrivals.definition, arrivals.content.clone())
            .unwrap()
            .ghost(owner.participant),
        None
    );
}

#[test]
fn an_arrive_sequence_starts_once_stops_on_the_event_and_completes_once() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    let record = arrivals.start(&mut players, owner).unwrap().unwrap();
    assert_eq!(arrivals.start(&mut players, owner).unwrap(), None);
    let Some(Initial::Entity { prefix, fields }) = &record.initial else {
        panic!()
    };
    // Official layout: manager selector 2, sequence 1, the participant, bus 2.
    assert_eq!((prefix.sub_id, prefix.asset.local_index), (2, 9));
    let entity::Initial::SequenceRoot {
        manager_selector,
        sequence,
        participants,
        ..
    } = &fields[0]
    else {
        panic!()
    };
    assert_eq!(
        (*manager_selector, *sequence, participants.as_slice()),
        (2, 1, [owner.participant].as_slice())
    );
    let ghost = record.id;
    assert_eq!(arrivals.ghost(owner.participant), Some(ghost));
    assert!(arrivals.acknowledges(ghost, 2, 0));
    assert!(arrivals.acknowledges(ghost, 3, 0));
    assert!(!arrivals.acknowledges(ghost, 2, 1) && !arrivals.acknowledges(ghost + 1, 2, 0));
    assert!(arrivals.has_completion(ghost, 0) && !arrivals.has_completion(ghost, 1));
    let completed = Completed {
        sequence: ghost,
        participant: owner.participant,
    };
    // Completing before the stop is a protocol error.
    assert_eq!(
        arrivals.complete(&players, owner, completed),
        Err(Error::Shape)
    );
    for other in [
        reached(ARRIVED + 1, ghost, 1),
        reached(ARRIVED, ghost, 2),
        reached(ARRIVED, ghost + 1, 1),
        Message::Reached {
            event: ARRIVED,
            target: EntityRef { ghost, entity: 1 },
            player: 1,
        },
        Message::Reached {
            event: ARRIVED,
            target: EntityRef { ghost, entity: 1 },
            player: 255,
        },
    ] {
        assert!(!arrivals.recognizes_arrival(&players, owner.connection, owner.persona, &other));
        assert_eq!(
            arrivals
                .arrived(&players, owner.connection, owner.persona, &other)
                .unwrap(),
            None
        );
    }
    let report = reached(ARRIVED, ghost, 1);
    for (connection, persona) in [
        (owner.connection + 1, owner.persona),
        (owner.connection, owner.persona + 1),
    ] {
        assert!(!arrivals.recognizes_arrival(&players, connection, persona, &report));
        assert_eq!(
            arrivals
                .arrived(&players, connection, persona, &report)
                .unwrap(),
            None
        );
    }
    let stop = arrivals
        .arrived(
            &players,
            owner.connection,
            owner.persona,
            &reached(ARRIVED, ghost, 1),
        )
        .unwrap()
        .unwrap();
    assert_eq!(stop.sequence, ghost);
    assert!(arrivals.recognizes_arrival(
        &players,
        owner.connection,
        owner.persona,
        &reached(ARRIVED, ghost, 1)
    ));
    assert_eq!(
        arrivals
            .arrived(
                &players,
                owner.connection,
                owner.persona,
                &reached(ARRIVED, ghost, 1)
            )
            .unwrap(),
        None
    );
    let foreign = Owner {
        persona: 401,
        ..owner
    };
    assert!(arrivals.complete(&players, foreign, completed).is_err());
    assert_eq!(
        arrivals.complete(&players, owner, completed),
        Ok(Some(ghost))
    );
    assert_eq!(arrivals.complete(&players, owner, completed), Ok(None));
    assert!(arrivals.recognizes_arrival(
        &players,
        owner.connection,
        owner.persona,
        &reached(ARRIVED, ghost, 1)
    ));
    arrivals.finish_rollout(&mut players, owner).unwrap();
    arrivals.reset(&players, owner).unwrap();
    assert!(!arrivals.recognizes_arrival(
        &players,
        owner.connection,
        owner.persona,
        &reached(ARRIVED, ghost, 1)
    ));
    let next = arrivals.start(&mut players, owner).unwrap().unwrap();
    assert_ne!(next.id, ghost);
    assert!(arrivals.recognizes_arrival(
        &players,
        owner.connection,
        owner.persona,
        &reached(ARRIVED, next.id, 1)
    ));
}

#[test]
fn a_silent_client_is_stopped_after_the_fallback() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    arrivals.poll(1_000).unwrap();
    let ghost = arrivals.start(&mut players, owner).unwrap().unwrap().id;
    assert!(arrivals.poll(1_000 + FALLBACK_MS - 1).unwrap().is_empty());
    let stops = arrivals.poll(1_000 + FALLBACK_MS).unwrap();
    assert_eq!(
        stops.iter().map(|s| s.sequence).collect::<Vec<_>>(),
        vec![ghost]
    );
    assert!(arrivals.poll(1_000 + 2 * FALLBACK_MS).unwrap().is_empty());
    assert_eq!(arrivals.poll(0), Err(Error::Shape));
}

#[test]
fn rollout_has_both_acknowledgements_before_its_scene_channel() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    let record = arrivals.start(&mut players, owner).unwrap().unwrap();
    let Some(Initial::Entity { fields, .. }) = record.initial else {
        panic!()
    };
    assert_eq!(fields.len(), 5);
    for selector in [2, 3] {
        assert_eq!(
            fields[selector],
            entity::Initial::Rpc(entity::Rpc {
                selector: selector as u16,
                serial: Serial::new(1).unwrap(),
            })
        );
    }
    assert_eq!(
        fields[4],
        entity::Initial::RpcReference {
            rpc: entity::Rpc {
                selector: 4,
                serial: Serial::new(1).unwrap()
            },
            reference: 2,
            target_selector: 0,
        }
    );
    let body = entity::Body {
        initial: Some(fields),
        updates: vec![Some(entity::Update::Noop); 5],
    };
    let encoded = body.encode(&profile()).unwrap();
    assert_eq!(encoded.len(), 228);
    let decoded = entity::Body::decode(encoded.span(), &profile(), true).unwrap();
    assert_eq!(decoded.bits, encoded.len());
    assert_eq!(decoded.body, body);
    for len in 0..encoded.len() {
        assert!(
            entity::Body::decode(encoded.span().slice(0, len).unwrap(), &profile(), true).is_err()
        );
    }
    // The waiting profile would consume RPC3 as a bogus reference/selector.
    assert_ne!(profile(), crate::sequences::profile());
    let mut bad = arrivals.definition;
    bad.layout = Layout::Waiting;
    assert!(Arrivals::new(bad, arrivals.content.clone()).is_err());
    let waiting_content = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(arrivals.definition.asset, crate::sequences::profile())],
    )
    .unwrap();
    assert!(Arrivals::new(arrivals.definition, waiting_content).is_err());
}

#[test]
fn foreign_participants_and_wrong_profiles_are_refused() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    let stranger = Owner {
        persona: 999,
        ..owner
    };
    assert_eq!(
        arrivals.start(&mut players, stranger),
        Err(Error::UnknownObject)
    );
    let asset = Asset {
        bundle: 16,
        type_id: 3424,
        local_index: 9,
    };
    let wrong = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(
            asset,
            entity::Profile::new(&[entity::Kind::SequenceRoot]).unwrap(),
        )],
    )
    .unwrap();
    let definition = Definition {
        presentation: None,
        layout: crate::sequences::Layout::GarageExit,
        asset,
        manager: Component {
            scene_key: 7,
            index: 3,
        },
        embedded_bus: 2,
        channel: Component {
            scene_key: 8,
            index: 1,
        },
    };
    assert!(Arrivals::new(definition, wrong).is_err());
    assert!(
        Arrivals::new(
            Definition {
                embedded_bus: 0,
                ..definition
            },
            setup().1.content.clone()
        )
        .is_err()
    );
}
