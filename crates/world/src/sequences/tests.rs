// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
const STREAMING_ASSET: Asset = Asset {
    bundle: 3,
    type_id: 7,
    local_index: 1,
};
use crate::replication::{Initial, players::Request, sublevel};

fn setup() -> (Players, Sequences) {
    let mut players = Players::default();
    let scenes = sublevel::Content::new(vec![
        (
            7,
            sublevel::Profile::new(false, &[sublevel::Kind::Noop, sublevel::Kind::Rpc]).unwrap(),
        ),
        (
            8,
            sublevel::Profile::new(
                false,
                &[
                    sublevel::Kind::Noop,
                    sublevel::Kind::Noop,
                    sublevel::Kind::Rpc,
                ],
            )
            .unwrap(),
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
        bundle: 9,
        type_id: 3424,
        local_index: 0,
    };
    let content = Content::new(
        vec![entity::creation::Catalog::new(9, &[(3424, 1)]).unwrap()],
        vec![(asset, profile())],
    )
    .unwrap();
    let definition = Definition {
        asset,
        manager: Component {
            scene_key: 7,
            index: 1,
        },
        embedded_bus: 2,
        channel: Component {
            scene_key: 8,
            index: 2,
        },
    };
    (players, Sequences::new(definition, content).unwrap())
}
fn join(players: &mut Players, connection: u8, persona: u64) -> Owner {
    let player = players
        .create(
            connection,
            persona,
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
        connection,
        persona,
        participant: players
            .join(connection, persona, player)
            .unwrap()
            .unwrap()
            .id,
    }
}
fn initial(record: &Record) -> &[entity::Initial] {
    let Some(Initial::Entity { fields, .. }) = &record.initial else {
        panic!("entity")
    };
    fields
}

#[test]
fn owned_sequences_resolve_current_endpoints_allocate_and_isolate() {
    let (mut players, mut sequences) = setup();
    let a = join(&mut players, 1, 10);
    let b = join(&mut players, 2, 20);
    let first = sequences.start(&mut players, a).unwrap().unwrap();
    let second = sequences.start(&mut players, b).unwrap().unwrap();
    assert_ne!(first.id, second.id);
    assert!(
        matches!(&initial(&first)[0], entity::Initial::SequenceRoot { manager: 1, manager_selector: 0, sequence: 0, participants, .. } if participants == &[a.participant])
    );
    assert!(
        matches!(&initial(&second)[0], entity::Initial::SequenceRoot { sequence: 1, participants, .. } if participants == &[b.participant])
    );
    assert!(
        matches!(&initial(&second)[1], entity::Initial::RpcOptionalReference { reference: Some(id), .. } if *id == b.participant)
    );
    assert!(matches!(
        initial(&second)[2],
        entity::Initial::RpcReference {
            reference: 2,
            target_selector: 0,
            ..
        }
    ));
    let reply = Completed {
        sequence: first.id,
        participant: a.participant,
    };
    assert!(sequences.complete(&players, b, reply).is_err());
    assert!(
        sequences
            .complete(&players, Owner { persona: 20, ..a }, reply)
            .is_err()
    );
    assert!(
        sequences
            .complete(
                &players,
                a,
                Completed {
                    sequence: second.id,
                    ..reply
                }
            )
            .is_err()
    );
    assert!(!sequences.is_complete(a.participant));
    assert_eq!(sequences.complete(&players, a, reply), Err(Error::Shape));
    assert_eq!(sequences.poll(FALLBACK_MS, 1).unwrap().len(), 1);
    assert!(sequences.complete(&players, a, reply).unwrap());
    assert!(!sequences.complete(&players, a, reply).unwrap());
    assert!(!sequences.is_complete(b.participant));
    assert!(sequences.start(&mut players, a).unwrap().is_none());
    assert!(sequences.start(&mut players, b).unwrap().is_none());
    let (mut independent, mut fresh) = setup();
    let c = join(&mut independent, 3, 30);
    let creation = fresh.start(&mut independent, c).unwrap().unwrap();
    assert!(matches!(
        initial(&creation)[0],
        entity::Initial::SequenceRoot { sequence: 0, .. }
    ));
    assert!(!fresh.is_complete(c.participant));
    assert!(fresh.complete(&independent, a, reply).is_err());
}

#[test]
fn failed_or_discarded_creation_preserves_both_allocators() {
    let (mut players, mut sequences) = setup();
    let owner = join(&mut players, 1, 10);
    let before = players.objects().snapshot();
    let missing = sequences.definition.channel;
    sequences.definition.channel.index = 0;
    assert!(sequences.start(&mut players, owner).is_err());
    assert_eq!(players.objects().snapshot(), before);
    assert!(sequences.entries.is_empty());
    sequences.definition.channel = missing;
    let mut staged_players = players.clone();
    let mut staged_sequences = sequences.clone();
    let discarded = staged_sequences
        .start(&mut staged_players, owner)
        .unwrap()
        .unwrap();
    let committed = sequences.start(&mut players, owner).unwrap().unwrap();
    assert_eq!(discarded, committed);
    assert_eq!(players.objects().snapshot().last(), Some(&committed));
    assert!(
        sequences
            .complete(
                &players,
                Owner {
                    connection: 2,
                    ..owner
                },
                Completed {
                    sequence: committed.id,
                    participant: owner.participant
                }
            )
            .is_err()
    );
}

#[test]
fn completion_rejects_truncation_extra_bytes_wrong_route_and_reserved_words() {
    let call = Completed {
        sequence: 299,
        participant: 260,
    };
    let wire = call.encode().unwrap();
    assert_eq!(wire.len(), 155);
    assert_eq!(Completed::decode(wire.span()), Ok(call));
    for cut in 0..155 {
        assert!(Completed::decode(wire.span().slice(0, cut).unwrap()).is_err());
    }
    let mut extra = wire.clone();
    extra.put(0, 1);
    assert!(Completed::decode(extra.span()).is_err());
    for bit in [0, 64, 107, 147] {
        let mut bytes = wire.bytes().to_vec();
        bytes[bit / 8] ^= 1 << (7 - bit % 8);
        assert!(
            Completed::decode(BitSpan::new(&bytes, 0, 155).unwrap()).is_err(),
            "bit{bit}"
        );
    }
    let mut bytes = wire.bytes().to_vec();
    bytes[19] |= 0x20;
    assert_eq!(
        Completed::decode(BitSpan::new(&bytes, 0, 155).unwrap()),
        Ok(call)
    );
    assert!(
        Completed {
            participant: 0,
            ..call
        }
        .encode()
        .is_err()
    );
    assert!(
        Completed {
            sequence: 8192,
            ..call
        }
        .encode()
        .is_err()
    );
}

#[test]
fn sequence_resource_limit_is_transactional() {
    let (mut players, mut sequences) = setup();
    let owner = join(&mut players, 1, 10);
    for id in 1000..1000 + MAX_INSTANCES as u16 {
        sequences.entries.insert(
            id,
            Instance {
                owner: Owner {
                    participant: id,
                    ..owner
                },
                ghost: id + 1000,
                phase: Phase::Running {
                    fallback_at: FALLBACK_MS,
                },
                streaming: None,
                garage_loaded: false,
                ready_at: None,
            },
        );
    }
    let before = players.objects().snapshot();
    assert_eq!(sequences.start(&mut players, owner), Err(Error::Bound));
    assert_eq!(players.objects().snapshot(), before);
}

#[test]
fn fallback_clock_capacity_repeats_and_discarded_outputs_preserve_phases() {
    let (mut players, mut sequences) = setup();
    sequences.poll(100, 0).unwrap();
    let a = join(&mut players, 1, 10);
    let first = sequences.start(&mut players, a).unwrap().unwrap();
    let call = Completed {
        sequence: first.id,
        participant: a.participant,
    };
    assert!(
        sequences
            .poll(100 + FALLBACK_MS - 1, 10)
            .unwrap()
            .is_empty()
    );
    assert!(sequences.poll(100 + FALLBACK_MS, 0).unwrap().is_empty());
    assert_eq!(sequences.complete(&players, a, call), Err(Error::Shape));
    let mut discarded = sequences.clone();
    assert_eq!(discarded.poll(100 + FALLBACK_MS, 10).unwrap().len(), 1);
    assert_eq!(sequences.complete(&players, a, call), Err(Error::Shape));
    assert_eq!(sequences.poll(100 + FALLBACK_MS, 10).unwrap().len(), 1);
    assert!(sequences.poll(100 + FALLBACK_MS, 10).unwrap().is_empty());
    assert!(sequences.poll(99, 10).is_err());
    assert!(sequences.complete(&players, a, call).unwrap());
    assert!(!sequences.complete(&players, a, call).unwrap());
    let b = join(&mut players, 2, 20);
    let second = sequences.start(&mut players, b).unwrap().unwrap();
    assert!(
        sequences
            .poll(100 + 2 * FALLBACK_MS - 1, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sequences.poll(100 + 2 * FALLBACK_MS, 10).unwrap()[0].sequence,
        second.id
    );
    let (mut players, mut overflow) = setup();
    let c = join(&mut players, 3, 30);
    overflow.poll(u64::MAX, 0).unwrap();
    let before = players.objects().snapshot();
    assert_eq!(overflow.start(&mut players, c), Err(Error::Bound));
    assert_eq!(players.objects().snapshot(), before);
    assert!(overflow.entries.is_empty());
}

fn streaming(players: &mut Players, owner: Owner) -> u16 {
    let content = Content::new(
        vec![
            entity::creation::Catalog::new(STREAMING_ASSET.bundle, &[(STREAMING_ASSET.type_id, 2)])
                .unwrap(),
        ],
        vec![(
            STREAMING_ASSET,
            entity::Profile::new(&[entity::Kind::Root]).unwrap(),
        )],
    )
    .unwrap();
    players
        .spawn_entities(
            vec![Creation {
                prefix: Prefix {
                    parent: None,
                    blueprint: 1,
                    sub_id: 1,
                    owner: None,
                    asset: STREAMING_ASSET,
                },
                body: entity::Body {
                    initial: Some(vec![entity::Initial::Root {
                        value: 1,
                        rpc: entity::Rpc {
                            selector: 0,
                            serial: Serial::new(1).unwrap(),
                        },
                        reference: owner.participant,
                    }]),
                    updates: vec![Some(entity::Update::Noop)],
                },
            }],
            &content,
        )
        .unwrap()[0]
        .id
}
fn reached(ghost: u16, event: u32, player: u8) -> crate::logic::Message {
    crate::logic::Message::Reached {
        event,
        target: crate::logic::EntityRef { ghost, entity: 1 },
        player,
    }
}

#[test]
fn readiness_requires_streaming_and_every_slot_and_does_not_reset_on_repeats() {
    for streaming_first in [false, true] {
        let (mut players, mut sequences) = setup();
        let owner = join(&mut players, 1, 10);
        let sequence = sequences.start(&mut players, owner).unwrap().unwrap().id;
        let ghost = streaming(&mut players, owner);
        sequences
            .bind_streaming(&players, owner, ghost, STREAMING_ASSET)
            .unwrap();
        sequences.poll(100, 0).unwrap();
        if streaming_first {
            assert!(
                sequences
                    .streaming_event(&reached(ghost, STREAMING_LOADED, 0))
                    .unwrap()
            );
        } else {
            sequences.garage_loaded(owner, true).unwrap();
        }
        assert!(sequences.poll(50_000, 1).unwrap().is_empty());
        if streaming_first {
            sequences.garage_loaded(owner, true).unwrap();
        } else {
            sequences
                .streaming_event(&reached(ghost, STREAMING_LOADED, 0))
                .unwrap();
        }
        assert!(
            sequences
                .poll(50_000 + FIRST_READY_MS - 1, 1)
                .unwrap()
                .is_empty()
        );
        sequences
            .streaming_event(&reached(ghost, STREAMING_UNLOADED, 0))
            .unwrap();
        sequences
            .streaming_event(&reached(ghost, STREAMING_LOADED, 0))
            .unwrap();
        sequences.garage_loaded(owner, true).unwrap();
        assert!(
            sequences
                .poll(50_000 + FIRST_READY_MS, 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            sequences.poll(50_000 + FIRST_READY_MS, 1).unwrap()[0].sequence,
            sequence
        );
        assert!(
            sequences
                .poll(50_000 + FIRST_READY_MS, 1)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn streaming_checks_current_asset_participant_local_slot_and_transactional_errors() {
    let (mut players, mut sequences) = setup();
    let a = join(&mut players, 1, 10);
    let b = join(&mut players, 2, 20);
    let first = sequences.start(&mut players, a).unwrap().unwrap().id;
    let second = sequences.start(&mut players, b).unwrap().unwrap().id;
    let ga = streaming(&mut players, a);
    let gb = streaming(&mut players, b);
    assert!(
        sequences
            .bind_streaming(&players, a, gb, STREAMING_ASSET)
            .is_err()
    );
    assert!(
        sequences
            .bind_streaming(&players, a, first, STREAMING_ASSET)
            .is_err()
    );
    sequences
        .bind_streaming(&players, a, ga, STREAMING_ASSET)
        .unwrap();
    sequences
        .bind_streaming(&players, b, gb, STREAMING_ASSET)
        .unwrap();
    sequences.garage_loaded(a, true).unwrap();
    sequences.garage_loaded(b, true).unwrap();
    assert_eq!(
        players.runtime_index(b.connection, b.persona, b.participant),
        Some(1)
    );
    assert!(
        sequences
            .streaming_event(&reached(gb, STREAMING_LOADED, 1))
            .is_err()
    );
    assert!(!sequences.streaming_event(&reached(ga, 123, 0)).unwrap());
    assert!(
        !sequences
            .streaming_event(&reached(8191, STREAMING_LOADED, 0))
            .unwrap()
    );
    let mut discarded = sequences.clone();
    discarded
        .streaming_event(&reached(ga, STREAMING_LOADED, 0))
        .unwrap();
    assert!(sequences.poll(FIRST_READY_MS, 2).unwrap().is_empty());
    sequences
        .streaming_event(&reached(gb, STREAMING_LOADED, 0))
        .unwrap();
    assert_eq!(
        sequences.poll(2 * FIRST_READY_MS, 2).unwrap()[0].sequence,
        second
    );
    assert_eq!(sequences.poll(FALLBACK_MS, 2).unwrap()[0].sequence, first);
}
