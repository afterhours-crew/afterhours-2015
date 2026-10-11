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
