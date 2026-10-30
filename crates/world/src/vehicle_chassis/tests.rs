// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{
    bits::BitWriter,
    replication::{
        Update, entity,
        players::{Request, ai},
    },
};

struct Fixture {
    players: Players,
    owned: u16,
    foreign: u16,
    ai: ai::Ids,
    other_ai: ai::Ids,
    delegated: BTreeMap<u16, u64>,
}

#[test]
fn absent_tire_measurements_preserve_owned_slip_without_rewriting_the_delta() {
    let mut f = fixture();
    let pairs = [[23, 3], [24, 4], [91, 5], [92, 6]];
    let mut physics = f.players.vehicle_physics(1, 101, f.owned).unwrap();
    physics.controls.wheel_pairs = Some(pairs);
    f.players
        .set_vehicle_physics(1, 101, f.owned, physics.clone())
        .unwrap();
    physics.controls.wheel_pairs = None;
    physics.controls.values[8] = 7;
    let delta = f
        .players
        .set_vehicle_physics(1, 101, f.owned, physics)
        .unwrap()
        .unwrap();
    let Update::Vehicle { fields, .. } = delta.update else {
        panic!()
    };
    let Some(vehicle::Update::Chassis(chassis)) = &fields[1] else {
        panic!()
    };
    assert_eq!(chassis.physics.as_ref().unwrap().controls.wheel_pairs, None);
    let current = f.players.vehicle_physics(1, 101, f.owned).unwrap();
    assert_eq!(current.controls.wheel_pairs, Some(pairs));
    assert_eq!(current.controls.values[8], 7);
    let controls = crate::scoring::burnout::Controls::from_wire(&current.controls, None)
        .unwrap()
        .unwrap();
    assert_eq!(controls.throttle, 1.);
    assert!(controls.slip[2] > controls.slip[0]);
    let foreign = f.players.vehicle_physics(2, 202, f.foreign).unwrap();
    assert_ne!(foreign.controls.wheel_pairs, Some(pairs));
}

fn car() -> (vehicle::EntityCreation, entity::creation::Content) {
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
        vehicle::Kind::Chassis,
        vehicle::Kind::Index,
        vehicle::Kind::Bool,
        vehicle::Kind::Wheel,
        vehicle::Kind::Wheel,
    ])
    .unwrap();
    let content = Content::new(vec![Catalog::new(2, &[(30, 1)]).unwrap()], vec![])
        .unwrap()
        .with_vehicles(vec![(asset, profile.clone())])
        .unwrap();
    let rpcs = vehicle::components::fresh_rpcs(&profile);
    let custom = vehicle::Customization {
        value: 0,
        list_a: vec![],
        list_b: vec![],
        index: None,
        values: [0; 2],
        flag: false,
        tuning: [0; 32],
    };
    let root = vehicle::root::State::new(None)
        .unwrap()
        .initial(
            [0.; 3],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            [0.; 3],
            rpcs[0].clone().unwrap(),
        )
        .unwrap();
    let mut chassis =
        vehicle::chassis::State::new([10., 2., 3.], [0., 0., 0., 1.], 4, custom.clone()).unwrap();
    chassis.set_flags(false, (3., true)).unwrap();
    (
        vehicle::EntityCreation {
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
                    connection_id: 1,
                    fields: vec![
                        vehicle::Initial::Root(Box::new(root)),
                        vehicle::Initial::Chassis(Box::new(vehicle::ChassisInitial {
                            rpc: rpcs[1].clone().unwrap(),
                            vectors: [[None; 3]; 2],
                            custom,
                        })),
                        vehicle::Initial::Index {
                            rpc: rpcs[2].clone().unwrap(),
                            index: Some(2),
                            value: 4,
                        },
                        vehicle::Initial::Empty,
                        vehicle::Initial::Wheel {
                            rpc: rpcs[4].clone().unwrap(),
                            flag: false,
                        },
                        vehicle::Initial::Wheel {
                            rpc: rpcs[5].clone().unwrap(),
                            flag: false,
                        },
                    ],
                }),
                updates: vec![
                    Some(vehicle::Update::Root(None)),
                    Some(chassis.update([0.; 3], 15).unwrap()),
                    Some(vehicle::Update::Index {
                        index: Some(0),
                        value: Some(93),
                    }),
                    Some(vehicle::Update::Bool(Some(true))),
                    Some(vehicle::Update::Wheel(Some(false))),
                    Some(vehicle::Update::Wheel(Some(false))),
                ],
            },
        },
        content,
    )
}

fn fixture() -> Fixture {
    let mut players = Players::default();
    let (car, content) = car();
    let mut owned = Vec::new();
    for (connection, persona) in [(1, 101), (2, 202)] {
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
        owned.push(
            players
                .create_vehicle(connection, persona, participant, 10, car.clone(), &content)
                .unwrap()
                .id,
        );
    }
    let mut spawn = |key| {
        players
            .spawn_ai_vehicle(
                1,
                101,
                ai::Request {
                    key,
                    name: b"Rival".to_vec(),
                    flags: [true, false],
                    words: [0, 0],
                    mode: 0,
                },
                car.clone(),
                &content,
            )
            .unwrap()
            .unwrap()
            .ids
    };
    let ai = spawn(77);
    let other_ai = spawn(78);
    Fixture {
        players,
        owned: owned[0],
        foreign: owned[1],
        ai,
        other_ai,
        delegated: [(ai.vehicle, 77), (other_ai.vehicle, 78)].into(),
    }
}

fn wire(refs: &[u16], selector: u16, method: u32, value: u64, bits: usize) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(selector.into(), 9)
        .put(method.into(), 32)
        .put(value, bits)
        .put(63, if bits == 1 { 6 } else { 7 });
    let mut body = BitWriter::new();
    body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
    for id in refs {
        body.put((*id).into(), 13);
    }
    body.put((payload.len() / 8) as u64, 9)
        .put_span(payload.span());
    body
}

#[test]
fn burnout_override_is_scoped_to_owned_or_ai_lifetimes_and_preserves_other_fields() {
    let mut f = fixture();
    let foreign = f.players.objects().get(f.foreign).unwrap().clone();
    for id in [f.owned, f.ai.vehicle] {
        let before = f.players.objects().get(id).unwrap().clone();
        for (value, changed) in [
            (Some(0.75), true),
            (Some(0.75), false),
            (Some(2.), true),
            (None, true),
            (None, false),
        ] {
            let record = if id == f.owned {
                f.players.set_vehicle_burnout_tendency(1, 101, id, value)
            } else {
                f.players.set_ai_vehicle_burnout_tendency(1, 101, 77, value)
            }
            .unwrap();
            assert_eq!(record.is_some(), changed);
            if let Some(record) = record {
                record.encode().unwrap();
                let Update::Vehicle { fields, .. } = record.update else {
                    panic!()
                };
                assert!(fields[0].is_none() && fields[2..].iter().all(Option::is_none));
                assert_eq!(
                    fields[1],
                    Some(vehicle::Update::Chassis(Box::new(vehicle::ChassisUpdate {
                        physics: None,
                        flag: None,
                        custom: None,
                        pair: Some((value.unwrap_or(0.).to_bits(), value.is_some())),
                    })))
                );
            }
        }
        let mut cleared = before;
        let Update::Vehicle { fields, .. } = &mut cleared.current else {
            panic!()
        };
        let Some(vehicle::Update::Chassis(chassis)) = &mut fields[1] else {
            panic!()
        };
        // Fixture begins with a different active override. Native release
        // clears it; it does not restore that previous value.
        chassis.pair = Some((0, false));
        assert_eq!(f.players.objects().get(id), Some(&cleared));
    }
    assert_eq!(
        f.players
            .set_vehicle_burnout_tendency(1, 101, f.ai.vehicle, Some(1.)),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        f.players
            .set_vehicle_burnout_tendency(1, 101, f.foreign, Some(1.)),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        f.players
            .set_ai_vehicle_burnout_tendency(2, 202, 77, Some(1.)),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        f.players
            .set_ai_vehicle_burnout_tendency(1, 101, 79, Some(1.)),
        Err(Error::UnknownObject)
    );
    let before = f.players.objects().snapshot();
    assert_eq!(
        f.players
            .set_vehicle_burnout_tendency(1, 101, f.owned, Some(f32::NAN)),
        Err(Error::Bound)
    );
    assert_eq!(f.players.objects().snapshot(), before);
    assert_eq!(f.players.objects().get(f.foreign), Some(&foreign));
}

#[test]
fn horn_changes_only_its_chassis_field_for_owned_and_delegated_cars() {
    let mut f = fixture();
    let foreign = f.players.objects().get(f.foreign).unwrap().clone();
    for id in [f.owned, f.ai.vehicle] {
        let before = f.players.objects().get(id).unwrap().clone();
        for (sounding, changed) in [(true, true), (true, false), (false, true), (false, false)] {
            let report = receive(
                &mut f.players,
                1,
                101,
                wire(&[id], 1, 0, u64::from(sounding), 1).span(),
                |car| car == f.owned,
                &f.delegated,
            )
            .unwrap()
            .unwrap();
            let Report::Horn { record, .. } = report else {
                panic!()
            };
            assert_eq!(record.is_some(), changed);
            if let Some(record) = record {
                record.encode().unwrap();
                let Update::Vehicle { fields, .. } = record.update else {
                    panic!()
                };
                assert!(fields[0].is_none() && fields[2..].iter().all(Option::is_none));
                assert_eq!(
                    fields[1],
                    Some(vehicle::Update::Chassis(Box::new(vehicle::ChassisUpdate {
                        physics: None,
                        flag: Some(sounding),
                        custom: None,
                        pair: None,
                    })))
                );
            }
        }
        assert_eq!(f.players.objects().get(id), Some(&before));
    }
    assert_eq!(f.players.objects().get(f.foreign), Some(&foreign));
    assert!(!f.players.owned_vehicles(1, 101).contains(&f.ai.vehicle));
    assert_eq!(
        f.players.set_vehicle_horn(1, 101, f.ai.vehicle, true),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        f.players
            .set_vehicle_nitrous_mode(1, 101, f.ai.vehicle, true),
        Err(Error::UnknownObject)
    );
}

#[test]
fn callbacks_reject_foreign_lifetimes_truncation_and_malformed_shapes_atomically() {
    let mut f = fixture();
    let before = f.players.objects().snapshot();
    let valid = wire(&[f.ai.vehicle], 1, 0, 1, 1);
    for bits in 0..valid.len() {
        assert!(
            receive(
                &mut f.players,
                1,
                101,
                valid.span().slice(0, bits).unwrap(),
                |_| false,
                &f.delegated
            )
            .is_err()
        );
    }
    for (connection, persona) in [(2, 101), (1, 202), (2, 202)] {
        assert_eq!(
            receive(
                &mut f.players,
                connection,
                persona,
                valid.span(),
                |_| true,
                &f.delegated
            ),
            Err(Error::UnknownObject)
        );
    }
    for map in [
        BTreeMap::new(),
        [(f.ai.vehicle, 78)].into(),
        [(f.ai.vehicle, 999)].into(),
    ] {
        assert_eq!(
            receive(&mut f.players, 1, 101, valid.span(), |_| true, &map),
            Err(Error::UnknownObject)
        );
    }
    assert_eq!(
        receive(
            &mut f.players,
            1,
            101,
            wire(&[f.foreign], 1, 0, 1, 1).span(),
            |_| true,
            &f.delegated
        ),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        receive(
            &mut f.players,
            1,
            101,
            wire(&[f.owned], 1, 0, 1, 1).span(),
            |_| false,
            &f.delegated
        ),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        receive(
            &mut f.players,
            1,
            101,
            wire(&[f.ai.vehicle, f.owned], 1, 0, 1, 1).span(),
            |_| true,
            &f.delegated
        ),
        Err(Error::Shape)
    );
    let mut extra = valid.clone();
    extra.put(0, 1);
    assert_eq!(
        receive(&mut f.players, 1, 101, extra.span(), |_| true, &f.delegated),
        Err(Error::Shape)
    );
    for (selector, method) in [(0, 0), (1, 1), (2, 0), (1, 3)] {
        assert_eq!(
            receive(
                &mut f.players,
                1,
                101,
                wire(&[f.owned], selector, method, 1, 1).span(),
                |_| true,
                &f.delegated
            ),
            Ok(None)
        );
    }
    assert_eq!(f.players.objects().snapshot(), before);
    f.players.remove_ai_vehicle(1, 101, 77).unwrap();
    let retired = f.players.objects().snapshot();
    assert_eq!(
        receive(&mut f.players, 1, 101, valid.span(), |_| true, &f.delegated),
        Ok(None)
    );
    assert_eq!(f.players.objects().snapshot(), retired);
}

#[test]
fn drift_exit_delivers_a_finite_event_without_inventing_replies_or_vehicle_mutation() {
    let mut f = fixture();
    let before = f.players.objects().snapshot();
    for id in [f.owned, f.ai.vehicle] {
        for value in [0f32, 8.75, -2.] {
            assert_eq!(
                receive(
                    &mut f.players,
                    1,
                    101,
                    wire(&[id], 1, 2, value.to_bits().into(), 32).span(),
                    |car| car == f.owned,
                    &f.delegated
                ),
                Ok(Some(Report::DriftExit { vehicle: id, value }))
            );
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                receive(
                    &mut f.players,
                    1,
                    101,
                    wire(&[id], 1, 2, value.to_bits().into(), 32).span(),
                    |_| true,
                    &f.delegated
                ),
                Err(Error::Shape)
            );
        }
    }
    assert_eq!(f.players.objects().snapshot(), before);
}

#[test]
fn rival_nitrous_uses_exact_delegated_lifetime_and_preserves_fuel_and_other_components() {
    let mut f = fixture();
    let id = f.ai.vehicle;
    let before = f.players.objects().get(id).unwrap().clone();
    let unrelated = f.players.objects().get(f.other_ai.vehicle).unwrap().clone();
    for (method, changed) in [(0, true), (0, false), (1, true), (1, false)] {
        let report = crate::vehicle_nitrous::receive_with_ai(
            &mut f.players,
            1,
            101,
            wire(&[id], 2, method, 10f32.to_bits().into(), 32).span(),
            |_| false,
            &f.delegated,
        )
        .unwrap()
        .unwrap();
        assert_eq!(report.record.is_some(), changed);
        if let Some(record) = report.record {
            let Update::Vehicle { fields, .. } = record.update else {
                panic!()
            };
            assert_eq!(
                fields,
                vec![
                    None,
                    None,
                    Some(vehicle::Update::Index {
                        index: Some(u8::from(method == 0)),
                        value: None
                    }),
                    None,
                    None,
                    None
                ]
            );
        }
    }
    assert_eq!(f.players.objects().get(id), Some(&before));
    assert_eq!(
        f.players.objects().get(f.other_ai.vehicle),
        Some(&unrelated)
    );
    let snapshot = f.players.objects().snapshot();
    for map in [BTreeMap::new(), [(id, 78)].into(), [(id, 999)].into()] {
        assert_eq!(
            crate::vehicle_nitrous::receive_with_ai(
                &mut f.players,
                1,
                101,
                wire(&[id], 2, 0, 10f32.to_bits().into(), 32).span(),
                |_| true,
                &map
            ),
            Err(Error::UnknownObject)
        );
    }
    for (connection, persona) in [(2, 101), (1, 202), (2, 202)] {
        assert_eq!(
            crate::vehicle_nitrous::receive_with_ai(
                &mut f.players,
                connection,
                persona,
                wire(&[id], 2, 0, 10f32.to_bits().into(), 32).span(),
                |_| true,
                &f.delegated
            ),
            Err(Error::UnknownObject)
        );
    }
    for seconds in [-1f32, f32::NAN, f32::INFINITY] {
        assert_eq!(
            crate::vehicle_nitrous::receive_with_ai(
                &mut f.players,
                1,
                101,
                wire(&[id], 2, 0, seconds.to_bits().into(), 32).span(),
                |_| true,
                &f.delegated
            ),
            Err(Error::Shape)
        );
    }
    assert_eq!(f.players.objects().snapshot(), snapshot);
    f.players.remove_ai_vehicle(1, 101, 77).unwrap();
    assert_eq!(
        f.players.set_ai_vehicle_nitrous_mode(1, 101, 77, true),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        crate::vehicle_nitrous::receive_with_ai(
            &mut f.players,
            1,
            101,
            wire(&[id], 2, 0, 10f32.to_bits().into(), 32).span(),
            |_| true,
            &f.delegated
        ),
        Ok(None)
    );
}

#[test]
fn wheel_requests_affect_only_the_selected_tire_and_revoke_with_delegation() {
    use crate::vehicle_wheels::receive;
    let mut f = fixture();
    let request = |id, selector, method| wire(&[id], selector, method, 1, 1);
    for id in [f.owned, f.ai.vehicle] {
        let before = f.players.objects().get(id).unwrap().clone();
        for selector in [3, 4] {
            for (method, changed) in [(0, true), (0, false), (1, true), (1, false)] {
                let report = receive(
                    &mut f.players,
                    1,
                    101,
                    request(id, selector, method).span(),
                    |car| car == f.owned,
                    &f.delegated,
                )
                .unwrap()
                .unwrap();
                assert_eq!(report.vehicle, id);
                assert_eq!(report.component, usize::from(selector) + 1);
                assert_eq!(report.burst, method == 0);
                assert_eq!(report.record.is_some(), changed);
                if let Some(record) = report.record {
                    record.encode().unwrap();
                    let Update::Vehicle { fields, .. } = record.update else {
                        panic!()
                    };
                    assert_eq!(fields.iter().flatten().count(), 1);
                    assert_eq!(
                        fields[report.component],
                        Some(vehicle::Update::Wheel(Some(method == 0)))
                    );
                }
            }
        }
        assert_eq!(f.players.objects().get(id), Some(&before));
    }
    let before = f.players.objects().snapshot();
    let valid = request(f.ai.vehicle, 3, 0);
    for bits in 0..valid.len() {
        assert!(
            receive(
                &mut f.players,
                1,
                101,
                valid.span().slice(0, bits).unwrap(),
                |_| true,
                &f.delegated
            )
            .is_err()
        );
    }
    for (connection, persona) in [(2, 101), (1, 202), (2, 202)] {
        assert_eq!(
            receive(
                &mut f.players,
                connection,
                persona,
                valid.span(),
                |_| true,
                &f.delegated
            ),
            Err(Error::UnknownObject)
        );
    }
    for map in [
        BTreeMap::new(),
        [(f.ai.vehicle, 78)].into(),
        [(f.ai.vehicle, 999)].into(),
    ] {
        assert_eq!(
            receive(&mut f.players, 1, 101, valid.span(), |_| true, &map),
            Err(Error::UnknownObject)
        );
    }
    for (id, ready) in [(f.owned, false), (f.foreign, true)] {
        assert_eq!(
            receive(
                &mut f.players,
                1,
                101,
                request(id, 3, 0).span(),
                |_| ready,
                &f.delegated
            ),
            Err(Error::UnknownObject)
        );
    }
    assert_eq!(
        receive(
            &mut f.players,
            1,
            101,
            wire(&[f.ai.vehicle], 3, 0, 0, 32).span(),
            |_| true,
            &f.delegated
        ),
        Err(Error::Shape)
    );
    assert_eq!(
        receive(
            &mut f.players,
            1,
            101,
            wire(&[f.ai.vehicle, f.owned], 3, 0, 0, 1).span(),
            |_| true,
            &f.delegated
        ),
        Err(Error::Shape)
    );
    let mut extra = valid.clone();
    extra.put(0, 1);
    assert_eq!(
        receive(&mut f.players, 1, 101, extra.span(), |_| true, &f.delegated),
        Err(Error::Shape)
    );
    for (selector, method) in [(1, 0), (3, 2), (5, 0)] {
        assert_eq!(
            receive(
                &mut f.players,
                1,
                101,
                request(f.ai.vehicle, selector, method).span(),
                |_| true,
                &f.delegated
            ),
            Ok(None)
        );
    }
    assert_eq!(
        f.players
            .set_vehicle_tire_burst(1, 101, f.ai.vehicle, 4, true),
        Err(Error::UnknownObject)
    );
    for component in [0, 1, 2, 3, 6, usize::MAX] {
        assert_eq!(
            f.players
                .set_ai_vehicle_tire_burst(1, 101, 77, component, true),
            Err(Error::TypeMismatch)
        );
    }
    assert_eq!(f.players.objects().snapshot(), before);
    f.players.remove_ai_vehicle(1, 101, 77).unwrap();
    assert_eq!(
        receive(&mut f.players, 1, 101, valid.span(), |_| true, &f.delegated),
        Ok(None)
    );
    assert_eq!(
        f.players.set_ai_vehicle_tire_burst(1, 101, 77, 4, true),
        Err(Error::UnknownObject)
    );
}
