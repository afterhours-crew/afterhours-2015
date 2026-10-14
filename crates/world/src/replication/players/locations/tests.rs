// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
fn player(world: &mut Players, c: u8, p: u64) -> (u16, u16) {
    let player = world
        .create(
            c,
            p,
            Request {
                name: format!("Driver{c}").into_bytes(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = world.join(c, p, player).unwrap().unwrap().id;
    world.create_actor(c, p, participant).unwrap();
    (player, participant)
}
fn car(world: &mut Players, c: u8, p: u64, participant: u16, item: u64) -> u16 {
    use entity::creation::{Asset, Catalog, Content, Prefix};
    let asset = Asset {
        bundle: 2,
        type_id: 30,
        local_index: 0,
    };
    let content = Content::new(vec![Catalog::new(2, &[(30, 1)]).unwrap()], vec![])
        .unwrap()
        .with_vehicles(vec![(
            asset,
            vehicle::Profile::new(vec![
                vehicle::Kind::Root {
                    property_owner: false,
                },
                vehicle::Kind::Chassis,
            ])
            .unwrap(),
        )])
        .unwrap();
    let root = vehicle::root::State::new(None)
        .unwrap()
        .initial(
            [0.; 3],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            [0.; 3],
            entity::Rpc {
                selector: 0,
                serial: nfs_protocol::world::rpc::Serial::new(1).unwrap(),
            },
        )
        .unwrap();
    let custom = vehicle::Customization {
        value: 0,
        list_a: vec![],
        list_b: vec![],
        index: None,
        values: [0; 2],
        flag: false,
        tuning: [0; 32],
    };
    let chassis =
        vehicle::chassis::State::new([40., 2., -80.], [0., 0., 0., 1.], 3, custom.clone()).unwrap();
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
                connection_id: u16::from(c),
                fields: vec![
                    vehicle::Initial::Root(Box::new(root)),
                    vehicle::Initial::Chassis(Box::new(vehicle::ChassisInitial {
                        rpc: entity::Rpc {
                            selector: 1,
                            serial: nfs_protocol::world::rpc::Serial::new(2).unwrap(),
                        },
                        vectors: [[None; 3]; 2],
                        custom,
                    })),
                ],
            }),
            updates: vec![
                Some(vehicle::Update::Root(None)),
                Some(chassis.update([0.; 3], 11).unwrap()),
            ],
        },
    };
    world
        .create_vehicle(c, p, participant, item, creation, &content)
        .unwrap()
        .id
}

fn entries(record: &Record) -> Vec<Entry> {
    let Update::ParticipantLocationMap(map) = &record.update else {
        panic!()
    };
    map.groups
        .iter()
        .flatten()
        .flat_map(|g| g.entries.clone())
        .collect()
}

#[test]
fn customization_updates_only_owned_chassis_and_preserves_motion_and_identity() {
    let mut world = Players::default();
    let (player, participant) = player(&mut world, 1, 101);
    let car = car(&mut world, 1, 101, participant, 10);
    assert_eq!(
        world.player_vehicle_item(1, 101, player, car),
        Some((participant, 10))
    );
    assert_eq!(world.player_vehicle_item(2, 101, player, car), None);
    let before = world.objects().get(car).unwrap().clone();
    let Update::Vehicle { fields, .. } = &before.current else {
        panic!()
    };
    let Some(vehicle::Update::Chassis(chassis)) = &fields[1] else {
        panic!()
    };
    let mut custom = vehicle::Customization {
        value: 0,
        list_a: vec![],
        list_b: vec![],
        index: None,
        values: [0; 2],
        flag: false,
        tuning: [0; 32],
    };
    custom.values[0] = 2;
    assert!(
        world
            .set_vehicle_customization(2, 101, car, custom.clone())
            .is_err()
    );
    assert_eq!(world.objects().get(car).unwrap(), &before);
    let record = world
        .set_vehicle_customization(1, 101, car, custom.clone())
        .unwrap()
        .unwrap();
    let Update::Vehicle { fields, .. } = record.update else {
        panic!()
    };
    assert!(fields[0].is_none());
    let Some(vehicle::Update::Chassis(delta)) = &fields[1] else {
        panic!()
    };
    assert_eq!(delta.custom, Some(custom.clone()));
    assert!(delta.physics.is_none() && delta.flag.is_none() && delta.pair.is_none());
    let after = world.objects().get(car).unwrap();
    assert_eq!(after.initial, before.initial);
    let Update::Vehicle { fields, .. } = &after.current else {
        panic!()
    };
    let Some(vehicle::Update::Chassis(current)) = &fields[1] else {
        panic!()
    };
    assert_eq!(current.physics, chassis.physics);
    assert_eq!(current.flag, chassis.flag);
    assert_eq!(current.pair, chassis.pair);
    assert!(
        world
            .set_vehicle_customization(1, 101, car, custom)
            .unwrap()
            .is_none()
    );
}
#[test]
fn marker_moves_clears_and_rebinds_to_three_car_lifetimes() {
    let mut world = Players::default();
    let (_, participant) = player(&mut world, 1, 101);
    assert_eq!(
        world
            .sync_location_map(1, 101, &BTreeMap::new(), [0.; 3])
            .unwrap(),
        None
    );
    let mut map_id = None;
    for lifetime in 0..3 {
        let car = car(&mut world, 1, 101, participant, 10);
        let cars = BTreeMap::from([(participant, car)]);
        let published = world
            .sync_location_map(1, 101, &cars, [0.; 3])
            .unwrap()
            .unwrap();
        assert_eq!(published.initial.is_some(), lifetime == 0);
        assert_eq!(*map_id.get_or_insert(published.id), published.id);
        assert_eq!(
            entries(&published),
            vec![Entry {
                participant,
                x: Coordinate::from_world(40.).unwrap(),
                z: Coordinate::from_world(-80.).unwrap()
            }]
        );
        assert!(
            world
                .sync_location_map(1, 101, &cars, [0.; 3])
                .unwrap()
                .is_none()
        );
        let mut physics = world.vehicle_physics(1, 101, car).unwrap();
        physics.position = vehicle::Vector::from_position([100., 2., -180.], [0.; 3], 7).unwrap();
        world.set_vehicle_physics(1, 101, car, physics).unwrap();
        let moved = world
            .sync_location_map(1, 101, &cars, [0.; 3])
            .unwrap()
            .unwrap();
        assert_eq!(entries(&moved)[0].x, Coordinate::from_world(100.).unwrap());
        let empty = world
            .sync_location_map(1, 101, &BTreeMap::new(), [0.; 3])
            .unwrap()
            .unwrap();
        assert_eq!(empty.id, published.id);
        assert!(entries(&empty).is_empty());
        world.remove_vehicle(1, 101, participant, 10).unwrap();
        assert_eq!(
            world.sync_location_map(1, 101, &cars, [0.; 3]),
            Err(Error::UnknownObject)
        );
    }
}
#[test]
fn ownership_bad_pairs_and_map_references_fail_without_mutation() {
    let mut world = Players::default();
    let (a, ap) = player(&mut world, 1, 101);
    let (_, bp) = player(&mut world, 2, 202);
    let ac = car(&mut world, 1, 101, ap, 10);
    let bc = car(&mut world, 2, 202, bp, 20);
    let first = world
        .sync_location_map(1, 101, &BTreeMap::from([(ap, ac)]), [0.; 3])
        .unwrap()
        .unwrap();
    let second = world
        .sync_location_map(2, 202, &BTreeMap::from([(bp, bc)]), [0.; 3])
        .unwrap()
        .unwrap();
    assert_ne!(first.id, second.id);
    let before = world.objects.snapshot();
    for (c, p, pairs) in [
        (1, 101, vec![(bp, bc)]),
        (2, 202, vec![(ap, ac)]),
        (1, 202, vec![(ap, ac)]),
        (1, 101, vec![(ap, bc)]),
        (1, 101, vec![(ap, ac), (bp, bc)]),
    ] {
        assert_eq!(
            world.sync_location_map(c, p, &pairs.into_iter().collect(), [0.; 3]),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);
    }
    let invalid = Update::ParticipantLocationMap(location_map::Update {
        groups: Some(vec![Group {
            value: 255,
            entries: vec![Entry {
                participant: a,
                x: Coordinate::from_raw(0).unwrap(),
                z: Coordinate::from_raw(0).unwrap(),
            }],
        }]),
    });
    assert_eq!(
        world.objects.change(first.id, invalid),
        Err(Error::UnknownObject)
    );
    assert_eq!(world.objects.remove(ap), Err(Error::UnknownObject));
    assert_eq!(world.objects.snapshot(), before);
    world
        .sync_location_map(1, 101, &BTreeMap::new(), [0.; 3])
        .unwrap();
    assert_eq!(world.objects.get(second.id).unwrap().current, second.update);
}

fn service(world: &mut Players, key: u32) -> (crate::world_activation::Site, u16) {
    use sublevel::{Content, Kind, Profile, ordinary};
    let content = Content::new(vec![(
        key,
        Profile::new(false, &[Kind::Noop, Kind::RpcFourReferences]).unwrap(),
    )])
    .unwrap();
    let level = u16::try_from(key).unwrap();
    let creation = ordinary::creation(
        level,
        key,
        None,
        nfs_protocol::world::rpc::Serial::new(0).unwrap(),
        &ordinary::unpopulated(content.profile(key).unwrap()).unwrap(),
        &content,
    )
    .unwrap();
    let scene = world
        .create_scenes(&[level], vec![creation], &content)
        .unwrap()[0]
        .id;
    (
        crate::world_activation::Site {
            scene_key: key,
            serializer: 1,
        },
        scene,
    )
}

fn service_refs(world: &Players, scene: u16) -> [Option<u16>; 4] {
    let Update::SubLevel { fields, .. } = &world.objects.get(scene).unwrap().current else {
        panic!()
    };
    let Some(sublevel::Update::FourReferences(refs)) = &fields[1] else {
        panic!()
    };
    *refs
}

#[test]
fn service_binds_after_map_creation_and_rebinds_across_garage_lifetimes() {
    let mut world = Players::default();
    let (_, participant) = player(&mut world, 1, 101);
    let (site, scene) = service(&mut world, 501);
    let mut map_id = None;
    for lifetime in 0..3 {
        let car_id = car(&mut world, 1, 101, participant, 10);
        let cars = BTreeMap::from([(participant, car_id)]);
        let batch = world
            .sync_location_service(1, 101, &cars, [0.; 3], site)
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].initial.is_some(), lifetime == 0);
        assert_eq!(*map_id.get_or_insert(batch[0].id), batch[0].id);
        assert_eq!(batch[1].id, scene);
        assert_eq!(
            service_refs(&world, scene),
            [Some(0), Some(0), Some(0), map_id]
        );
        // Unchanged cars produce neither another map nor a redundant binding.
        assert!(
            world
                .sync_location_service(1, 101, &cars, [0.; 3], site)
                .unwrap()
                .is_empty()
        );
        let mut physics = world.vehicle_physics(1, 101, car_id).unwrap();
        physics.position = vehicle::Vector::from_position([160., 2., -200.], [0.; 3], 7).unwrap();
        world.set_vehicle_physics(1, 101, car_id, physics).unwrap();
        let moved = world
            .sync_location_service(1, 101, &cars, [0.; 3], site)
            .unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(
            entries(&moved[0])[0].x,
            Coordinate::from_world(160.).unwrap()
        );
        let cleared = world
            .sync_location_service(1, 101, &BTreeMap::new(), [0.; 3], site)
            .unwrap();
        assert_eq!(cleared.len(), 2);
        assert!(entries(&cleared[0]).is_empty());
        assert_eq!(service_refs(&world, scene), [Some(0); 4]);
        world.remove_vehicle(1, 101, participant, 10).unwrap();
        assert!(
            world
                .sync_location_service(1, 101, &BTreeMap::new(), [0.; 3], site)
                .unwrap()
                .is_empty()
        );
    }
    // A fresh connection owns a fresh world; no retired map reference survives.
    let mut reconnect = Players::default();
    let (_, scene) = service(&mut reconnect, 501);
    assert_eq!(service_refs(&reconnect, scene), [Some(0); 4]);
}

#[test]
fn service_rejects_foreign_binding_and_bad_sites_without_partial_map_mutation() {
    let mut world = Players::default();
    let (_, a) = player(&mut world, 1, 101);
    let (_, b) = player(&mut world, 2, 202);
    let ac = car(&mut world, 1, 101, a, 10);
    let bc = car(&mut world, 2, 202, b, 20);
    let (site, scene) = service(&mut world, 501);
    let (other_site, other_scene) = service(&mut world, 502);
    let a_cars = BTreeMap::from([(a, ac)]);
    let b_cars = BTreeMap::from([(b, bc)]);
    let empty = world.objects.snapshot();
    for bad in [
        crate::world_activation::Site {
            scene_key: 999,
            ..site
        },
        crate::world_activation::Site {
            serializer: 0,
            ..site
        },
        crate::world_activation::Site {
            serializer: usize::MAX,
            ..site
        },
    ] {
        assert!(
            world
                .sync_location_service(1, 101, &a_cars, [0.; 3], bad)
                .is_err()
        );
        assert_eq!(world.objects.snapshot(), empty);
        assert!(world.location_maps.is_empty());
    }
    world
        .sync_location_service(1, 101, &a_cars, [0.; 3], site)
        .unwrap();
    let first_refs = service_refs(&world, scene);
    let before = world.objects.snapshot();
    for cars in [&a_cars, &b_cars, &BTreeMap::new()] {
        assert_eq!(
            world.sync_location_service(2, 202, cars, [0.; 3], site),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);
    }
    world
        .sync_location_service(2, 202, &b_cars, [0.; 3], other_site)
        .unwrap();
    assert_ne!(first_refs[3], service_refs(&world, other_scene)[3]);
    world
        .sync_location_service(2, 202, &BTreeMap::new(), [0.; 3], other_site)
        .unwrap();
    assert_eq!(service_refs(&world, scene), first_refs);
}
