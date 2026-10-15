// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    bits::BitWriter,
    replication::{Update, entity, players::Request},
};
use nfs_protocol::world::rpc::Serial;

fn car(players: &mut Players, connection: u8, persona: u64) -> (u16, u16) {
    let player = players
        .create(
            connection,
            persona,
            Request {
                name: b"Nitrous test".to_vec(),
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
    (
        participant,
        spawn(players, connection, persona, participant),
    )
}

fn spawn(players: &mut Players, connection: u8, persona: u64, participant: u16) -> u16 {
    use entity::creation::{Asset, Catalog, Content, Prefix};
    let asset = Asset {
        bundle: 2,
        type_id: 30,
        local_index: 0,
    };
    let profile = vehicle::Profile::new(vec![
        vehicle::Kind::Root {
            property_owner: false,
        },
        vehicle::Kind::Bool,
        vehicle::Kind::Index,
    ])
    .unwrap();
    let content = Content::new(vec![Catalog::new(2, &[(30, 1)]).unwrap()], vec![])
        .unwrap()
        .with_vehicles(vec![(asset, profile.clone())])
        .unwrap();
    let rpcs = vehicle::components::fresh_rpcs(&profile);
    let root = vehicle::root::State::new(None)
        .unwrap()
        .initial(
            [0.; 3],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            [0.; 3],
            rpcs[0].clone().unwrap(),
        )
        .unwrap();
    let creation = vehicle::EntityCreation {
        prefix: Prefix {
            parent: None,
            blueprint: 0,
            sub_id: 1,
            owner: None,
            asset,
        },
        body: vehicle::Body {
            creation: Some(vehicle::Creation {
                setup: None,
                connection_id: u16::from(connection),
                fields: vec![
                    vehicle::Initial::Root(Box::new(root)),
                    vehicle::Initial::Empty,
                    vehicle::Initial::Index {
                        rpc: rpcs[2].clone().unwrap(),
                        index: Some(2),
                        value: 4,
                    },
                ],
            }),
            updates: vec![
                Some(vehicle::Update::Root(None)),
                Some(vehicle::Update::Bool(Some(true))),
                Some(vehicle::Update::Index {
                    index: Some(0),
                    value: Some(93),
                }),
            ],
        },
    };
    players
        .create_vehicle(connection, persona, participant, 10, creation, &content)
        .unwrap()
        .id
}

fn wire(refs: &[u16], selector: u16, method: u32, seconds: f32, padding: u8) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(selector.into(), 9)
        .put(method.into(), 32)
        .put(seconds.to_bits().into(), 32)
        .put(padding.into(), 7);
    let mut body = BitWriter::new();
    body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
    for id in refs {
        body.put((*id).into(), 13);
    }
    body.put(10, 9).put_span(payload.span());
    body
}

#[test]
fn nitrous_start_stop_repeats_preserve_fuel_equipment_and_other_components() {
    let mut players = Players::default();
    let (_, id) = car(&mut players, 1, 101);
    let before = players.objects().get(id).unwrap().clone();
    // Selector 1 belongs to serializer 2; do not confuse the two indices.
    for (method, active, changed) in [
        (0, true, true),
        (0, true, false),
        (1, false, true),
        (1, false, false),
        (0, true, true),
    ] {
        let call = wire(&[id], 1, method, 35.25, 127);
        let report = receive(&mut players, 1, 101, call.span(), |_| true)
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                report.active,
                report.client_seconds,
                report.record.is_some()
            ),
            (active, 35.25, changed)
        );
        if let Some(record) = report.record {
            record.encode().unwrap();
            let Update::Vehicle { fields, .. } = record.update else {
                panic!()
            };
            assert_eq!(
                fields,
                vec![
                    None,
                    None,
                    Some(vehicle::Update::Index {
                        index: Some(u8::from(active)),
                        value: None
                    })
                ]
            );
        }
        let after = players.objects().get(id).unwrap();
        assert_eq!(after.initial, before.initial);
        let (
            Update::Vehicle {
                fields: old,
                binding: a,
            },
            Update::Vehicle {
                fields: new,
                binding: b,
            },
        ) = (&before.current, &after.current)
        else {
            panic!()
        };
        assert_eq!(a, b);
        assert_eq!(old[..2], new[..2]);
        assert_eq!(
            new[2],
            Some(vehicle::Update::Index {
                index: Some(u8::from(active)),
                value: Some(93)
            })
        );
    }
}

#[test]
fn nitrous_rejects_bad_arguments_and_foreign_or_unready_cars_atomically() {
    let mut players = Players::default();
    let (_, id) = car(&mut players, 1, 101);
    let (_, other) = car(&mut players, 2, 202);
    let before = players.objects().snapshot();
    for seconds in [-1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            receive(
                &mut players,
                1,
                101,
                wire(&[id], 1, 0, seconds, 0).span(),
                |_| true
            ),
            Err(Error::Shape)
        );
    }
    for refs in [vec![id, other], vec![id, id]] {
        assert_eq!(
            receive(
                &mut players,
                1,
                101,
                wire(&refs, 1, 0, 1., 0).span(),
                |_| true
            ),
            Err(Error::Shape)
        );
    }
    for (connection, persona, target) in [(2, 101, id), (1, 202, id), (1, 101, other)] {
        assert_eq!(
            receive(
                &mut players,
                connection,
                persona,
                wire(&[target], 1, 0, 1., 0).span(),
                |_| true
            ),
            Err(Error::UnknownObject)
        );
    }
    assert_eq!(
        receive(
            &mut players,
            1,
            101,
            wire(&[id], 1, 0, 1., 0).span(),
            |_| false
        ),
        Err(Error::UnknownObject)
    );
    let body = wire(&[id], 1, 0, 1., 0);
    for len in 0..body.len() {
        assert!(
            receive(
                &mut players,
                1,
                101,
                body.span().slice(0, len).unwrap(),
                |_| true
            )
            .is_err()
        );
    }
    let mut extra = body.clone();
    extra.put(0, 1);
    assert_eq!(
        receive(&mut players, 1, 101, extra.span(), |_| true),
        Err(Error::Shape)
    );
    for (selector, method) in [(0, 0), (2, 0), (1, 2), (1, u32::MAX)] {
        assert_eq!(
            receive(
                &mut players,
                1,
                101,
                wire(&[id], selector, method, 1., 0).span(),
                |_| true
            ),
            Ok(None)
        );
    }
    assert_eq!(players.objects().snapshot(), before);
}

#[test]
fn nitrous_lifetimes_and_connections_do_not_share_mode() {
    let mut players = Players::default();
    let (participant, old) = car(&mut players, 1, 101);
    let (_, other) = car(&mut players, 2, 202);
    let other_before = players.objects().get(other).unwrap().clone();
    let fresh = players.clone();
    assert!(
        receive(
            &mut players,
            1,
            101,
            wire(&[old], 1, 0, 1., 0).span(),
            |_| true
        )
        .unwrap()
        .unwrap()
        .record
        .is_some()
    );
    assert_eq!(players.objects().get(other), Some(&other_before));
    assert_ne!(players.objects().get(old), fresh.objects().get(old));
    players.remove_vehicle(1, 101, participant, 10).unwrap();
    let new = spawn(&mut players, 1, 101, participant);
    let before = players.objects().get(new).unwrap().clone();
    assert_ne!(old, new);
    assert_eq!(
        receive(
            &mut players,
            1,
            101,
            wire(&[old], 1, 1, 2., 0).span(),
            |_| true
        ),
        Ok(None)
    );
    assert_eq!(players.objects().get(new), Some(&before));
    let Initial::Vehicle { creation, .. } = &before.initial else {
        panic!()
    };
    assert!(
        matches!(creation.fields[2], vehicle::Initial::Index { ref rpc, .. } if rpc.serial == Serial::new(1).unwrap())
    );
    assert!(
        receive(
            &mut players,
            1,
            101,
            wire(&[new], 1, 0, 3., 0).span(),
            |_| true
        )
        .unwrap()
        .unwrap()
        .record
        .is_some()
    );
}
