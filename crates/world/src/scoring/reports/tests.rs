// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    bits::BitWriter,
    replication::{
        entity::{
            Body, Kind as Field, Profile, Rpc, Update,
            creation::{Asset, Catalog, Content, Creation, Prefix},
        },
        players::Request,
    },
};
use nfs_protocol::world::rpc::Serial;

fn setup(players: &mut Players, connection: u8, persona: u64, kind: Kind) -> Binding {
    let player = players
        .create(
            connection,
            persona,
            Request {
                name: b"Query test".to_vec(),
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
    let asset = Asset {
        bundle: 3,
        type_id: 12,
        local_index: 0,
    };
    let content = Content::new(
        vec![Catalog::new(3, &[(12, 1)]).unwrap()],
        vec![(
            asset,
            Profile::new(&[Field::Root, Field::FloatProperty, Field::Rpc]).unwrap(),
        )],
    )
    .unwrap();
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: player,
            sub_id: 1,
            owner: None,
            asset,
        },
        body: Body {
            initial: Some(vec![
                entity::Initial::Root {
                    value: 1,
                    rpc: Rpc {
                        selector: 0,
                        serial: Serial::new(1).unwrap(),
                    },
                    reference: participant,
                },
                entity::Initial::FloatProperty(Some(0)),
                entity::Initial::Rpc(Rpc {
                    selector: 1,
                    serial: Serial::new(2).unwrap(),
                }),
            ]),
            updates: vec![
                Some(Update::Noop),
                Some(Update::FloatProperty(Some(0))),
                Some(Update::Noop),
            ],
        },
    };
    let id = players.spawn_entities(vec![creation], &content).unwrap()[0].id;
    Binding::new(players, id, 2, kind).unwrap()
}
fn wire(refs: &[u16], selector: u16, method: u32, words: &[u32]) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    for &word in words {
        payload.put(word.into(), 32);
    }
    payload.put(0x7f, 7);
    let mut body = BitWriter::new();
    body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
    for &id in refs {
        body.put(id.into(), 13);
    }
    body.put(payload.span().len().div_ceil(8) as u64, 9)
        .put_span(payload.span());
    body
}
#[test]
fn near_miss_four_properties_precede_its_event_and_weight_is_not_an_award() {
    let mut players = Players::default();
    let near = setup(&mut players, 1, 100, Kind::NearMiss);
    let collision = setup(&mut players, 2, 200, Kind::AccumulatedCollisions);
    let before = players.objects().get(near.entity).unwrap().clone();
    let values = [40.5f32, 70.25, -0.75, 0.125];
    let body = wire(&[near.entity], 1, 0, &values.map(f32::to_bits));
    let report = near
        .receive(&players, 1, 100, body.span(), |_| true)
        .unwrap()
        .unwrap();
    assert_eq!(report, Report::NearMiss(values));
    assert_eq!(
        report.publication(),
        Publication {
            properties: vec![
                (0xad59_4576, Value::Float(40.5)),
                (0x445f_266b, Value::Float(70.25)),
                (0xd70a_7f0c, Value::Float(-0.75)),
                (0xd887_31d6, Value::Float(0.125)),
            ],
            event: 1_552_797_432
        }
    );
    for weight in [0i32, 13, i32::MAX, i32::MIN] {
        let body = wire(&[collision.entity], 1, 0, &[weight as u32]);
        let report = collision
            .receive(&players, 2, 200, body.span(), |_| true)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.publication(),
            Publication {
                properties: vec![(1_977_761_241, Value::Integer(weight))],
                event: 1_190_559_846,
            }
        );
    }
    assert_eq!(players.objects().get(near.entity), Some(&before));
}
#[test]
fn ownership_readiness_and_retirement_are_checked_on_every_receive() {
    let mut players = Players::default();
    let first = setup(&mut players, 1, 100, Kind::NearMiss);
    let second = setup(&mut players, 2, 200, Kind::NearMiss);
    let body = wire(&[first.entity], 1, 0, &[0; 4]);
    for (connection, persona) in [(1, 200), (2, 100), (2, 200), (0, 100)] {
        assert_eq!(
            first.receive(&players, connection, persona, body.span(), |_| true),
            Err(Error::UnknownObject)
        );
    }
    assert_eq!(
        first.receive(&players, 1, 100, body.span(), |_| false),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        second.receive(&players, 2, 200, body.span(), |_| true),
        Ok(None)
    );
    assert!(Binding::new(&players, first.entity, 1, Kind::NearMiss).is_err());
    assert!(Binding::new(&players, first.entity, 0, Kind::NearMiss).is_err());
    players.remove_entities(&[first.entity]).unwrap();
    assert_eq!(
        first.receive(&players, 1, 100, body.span(), |_| true),
        Err(Error::UnknownObject)
    );
}
#[test]
fn exact_report_shape_rejects_truncation_extra_data_references_and_nonfinite_values() {
    let mut players = Players::default();
    let binding = setup(&mut players, 1, 100, Kind::NearMiss);
    let body = wire(&[binding.entity], 1, 0, &[0; 4]);
    for length in 0..body.span().len() {
        assert!(
            binding
                .receive(
                    &players,
                    1,
                    100,
                    body.span().slice(0, length).unwrap(),
                    |_| true
                )
                .is_err()
        );
    }
    for words in [
        vec![0; 3],
        vec![0; 5],
        vec![f32::NAN.to_bits(); 4],
        vec![f32::INFINITY.to_bits(); 4],
    ] {
        let body = wire(&[binding.entity], 1, 0, &words);
        assert_eq!(
            binding.receive(&players, 1, 100, body.span(), |_| true),
            Err(Error::Shape)
        );
    }
    let mut body = wire(&[binding.entity], 1, 0, &[0; 4]);
    body.put(0, 1);
    assert_eq!(
        binding.receive(&players, 1, 100, body.span(), |_| true),
        Err(Error::Shape)
    );
    let body = wire(&[binding.entity, binding.participant], 1, 0, &[0; 4]);
    assert_eq!(
        binding.receive(&players, 1, 100, body.span(), |_| true),
        Err(Error::Shape)
    );
    for (selector, method) in [(0, 0), (2, 0), (1, 1), (1, u32::MAX)] {
        let body = wire(&[binding.entity], selector, method, &[0; 4]);
        assert_eq!(
            binding.receive(&players, 1, 100, body.span(), |_| true),
            Ok(None)
        );
    }
}
