// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::traffic::{Persona, Segment, Spline};
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    content::{Message, Registration, Registrations, SubLevelNames},
    replication::sublevel,
};

pub(crate) fn fixture() -> (Population, Players, Content) {
    fixture_with(1)
}
pub(crate) fn fixture_with(connection: u8) -> (Population, Players, Content) {
    let d = super::super::tests::definition();
    let names = ["constructed/gameplay", &d.bundle];
    let keys: Vec<_> = names
        .iter()
        .map(|name| name_key(name.as_bytes()).unwrap())
        .collect();
    let bindings = Bindings::from_messages(&[
        Message::Names(SubLevelNames {
            word: 0,
            entries: names
                .iter()
                .enumerate()
                .map(|(i, s)| (i as u16 + 1, s.as_bytes().to_vec()))
                .collect(),
        }),
        Message::Registrations(Registrations {
            header: 0,
            entries: (1..=2)
                .map(|i| Registration {
                    handle: i,
                    next: i,
                    region: 0,
                    word: u32::from(i + 3),
                    byte: 0,
                    flag: false,
                    enum3: 0,
                    word2: 0,
                    bit: false,
                    enum2: 0,
                    word3: 0,
                    text: vec![],
                })
                .collect(),
        }),
    ])
    .unwrap();
    let content = Content {
        managers: ai::Managers {
            alive: ai::State {
                scene: keys[0],
                serializer: 3,
            },
            reset_volumes: ai::State {
                scene: keys[0],
                serializer: 4,
            },
            scene: keys[0],
            actors: 1,
            participants: 2,
        },
        spline_class: 89,
        density: 1.,
        personas: vec![Persona {
            name: "Constructed traffic".into(),
            index: 0,
            vehicle: d.bundle.clone(),
            tags: vec![[3; 16]],
        }],
        vehicles: BTreeMap::from([(d.bundle.clone(), d)]),
        splines: vec![Spline {
            scene: keys[0],
            index: 0,
            catalog_count: 1,
            density: 1,
            tags: vec![[3; 16]],
            segments: vec![Segment {
                speed: 15.,
                points: (0..80).map(|i| [i as f32 * 15., 0., 0.]).collect(),
                curve: nfs_content::traffic::Curve::new(
                    [[0.; 3], [395., 0., 0.], [790., 0., 0.], [1185., 0., 0.]],
                    (1..80).map(|i| i as f32 / 79.).collect(),
                    (1..80).map(|i| i as f32 * 15.).collect(),
                )
                .unwrap(),
            }],
        }],
    };
    let population = Population::new(bindings, &content).unwrap();
    let mut world = Players::default();
    let player = world
        .create(
            connection,
            100,
            nfs_world::replication::players::Request {
                name: b"Owner".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap();
    world.join(connection, 100, player.id).unwrap();
    let profiles = [
        sublevel::Profile::new(
            false,
            &[
                sublevel::Kind::Noop,
                sublevel::Kind::RpcPairs,
                sublevel::Kind::RpcReferences,
                sublevel::Kind::RpcReferences,
                sublevel::Kind::Rpc,
            ],
        )
        .unwrap(),
        sublevel::Profile::new(false, &[sublevel::Kind::Noop]).unwrap(),
    ];
    let scenes =
        sublevel::Content::new(keys.iter().copied().zip(profiles.iter().cloned()).collect())
            .unwrap();
    let creations = keys
        .iter()
        .zip(&profiles)
        .enumerate()
        .map(|(i, (&key, profile))| {
            sublevel::ordinary::creation(
                i as u16 + 1,
                key,
                None,
                Serial::new(0).unwrap(),
                &sublevel::ordinary::unpopulated(profile).unwrap(),
                &scenes,
            )
            .unwrap()
        })
        .collect();
    world.create_scenes(&[1, 2], creations, &scenes).unwrap();
    (population, world, content)
}

pub(crate) fn focus() -> Focus {
    Focus {
        id: 1,
        position: [600., 0., 0.],
        velocity: [0.; 3],
        forward: [1., 0., 0.],
    }
}
fn input(now: u64, focus: &[Focus]) -> Input<'_> {
    Input {
        now,
        focus,
        random: [0; 2],
    }
}
pub(crate) fn spawn(
    p: &mut Population,
    world: &mut Players,
    c: &Content,
    connection: u8,
) -> Changes {
    p.advance(world, connection, 100, c, input(0, &[focus()]))
        .unwrap();
    p.advance(world, connection, 100, c, input(1001, &[focus()]))
        .unwrap()
}

#[test]
fn initial_delay_owned_creation_and_garage_retirement() {
    let (mut p, mut world, c) = fixture();
    let initial = world.objects().snapshot();
    for now in [0, 1000] {
        assert!(
            p.advance(&mut world, 1, 100, &c, input(now, &[focus()]))
                .unwrap()
                .spawned
                .is_none()
        );
    }
    let change = p
        .advance(&mut world, 1, 100, &c, input(1001, &[focus()]))
        .unwrap();
    let ids = change.spawned.unwrap();
    // Manager add, actor binding, Alive entry, then reset-volume tracking.
    assert_eq!(change.calls.len(), 4);
    assert!(matches!(
        change.calls[3],
        HostRpc::WorldActivation(nfs_world::world_activation::Notification {
            participant: Some(p),
            call: nfs_world::world_activation::Call::StreamedParticipant,
            ..
        }) if p == ids.participant
    ));
    // The actor is created holding control until the car's first report.
    assert!(change.records.iter().any(|r| r.id == ids.actor
        && matches!(&r.initial, Some(nfs_world::replication::Initial::Actor(a)) if a.flag_a && a.flag_b)));
    assert_eq!(change.records.len(), 6);
    assert_eq!(p.count(), 1);
    assert_eq!(p.cars[&1].born, 1001);
    assert!(matches!(
        p.advance(&mut world, 2, 100, &c, input(1002, &[])),
        Err(Error::UnknownObject)
    ));
    assert_eq!(world.ai_ids(1, 100, 1), Some(ids));
    let retired = p.advance(&mut world, 1, 100, &c, input(1002, &[])).unwrap();
    assert_eq!((retired.records.len(), retired.deleted.len()), (2, 4));
    assert_eq!(world.objects().snapshot(), initial);
    assert_eq!(p.road_counts[&0], 0);
    assert!(
        p.advance(&mut world, 1, 100, &c, input(1002, &[]))
            .unwrap()
            .deleted
            .is_empty()
    );
    assert!(matches!(
        p.advance(&mut world, 1, 100, &c, input(1001, &[])),
        Err(Error::Bound)
    ));
}

#[test]
fn late_failure_rolls_back_timers_persona_counts_ids_and_authority() {
    let (mut p, mut world, mut c) = fixture();
    p.advance(&mut world, 1, 100, &c, input(0, &[focus()]))
        .unwrap();
    let before = world.objects().snapshot();
    let timers = p.timers.clone();
    c.managers.actors = 0;
    assert!(
        p.advance(&mut world, 1, 100, &c, input(1001, &[focus()]))
            .is_err()
    );
    assert_eq!(world.objects().snapshot(), before);
    assert_eq!((p.next, p.clock, p.persona_cursor), (1, 0, 0));
    assert_eq!(p.timers, timers);
    assert!(p.cars.is_empty() && p.road_counts.is_empty());
    assert_eq!(p.authority, vehicle::authority::Registry::default());
    c.managers.actors = 1;
    assert!(
        p.advance(&mut world, 1, 100, &c, input(1001, &[focus()]))
            .unwrap()
            .spawned
            .is_some()
    );
}

#[test]
fn retirement_uses_total_age_and_motion_regions_not_an_outside_timer() {
    let (mut p, mut world, c) = fixture();
    let ids = spawn(&mut p, &mut world, &c, 1).spawned.unwrap();
    let anchor = p.cars[&1].anchor;
    let far = Focus {
        position: [anchor[0] - 500., 0., 0.],
        ..focus()
    };
    assert!(
        p.advance(&mut world, 1, 100, &c, input(3001, &[far]))
            .unwrap()
            .deleted
            .is_empty()
    );
    // Prediction protects a car 500m ahead of the actual player.
    let moving = Focus {
        velocity: [100., 0., 0.],
        ..far
    };
    for now in [3002, 90_000, 180_000] {
        assert!(
            !p.advance(&mut world, 1, 100, &c, input(now, &[moving]))
                .unwrap()
                .deleted
                .contains(&ids.vehicle)
        );
        assert_eq!(world.ai_ids(1, 100, 1), Some(ids));
    }
    // Stopping removes that protection immediately for an old car.
    let retired = p
        .advance(&mut world, 1, 100, &c, input(180_001, &[far]))
        .unwrap();
    assert!(retired.deleted.contains(&ids.vehicle));
    assert!(world.ai_ids(1, 100, 1).is_none());
}

#[test]
fn focus_identity_is_stable_and_invalid_updates_are_atomic() {
    let (mut p, mut world, c) = fixture();
    let other = Focus {
        id: 2,
        position: [5000., 0., 0.],
        ..focus()
    };
    p.advance(&mut world, 1, 100, &c, input(0, &[focus(), other]))
        .unwrap();
    p.advance(&mut world, 1, 100, &c, input(500, &[other, focus()]))
        .unwrap();
    assert_eq!(p.timers[&1].elapsed, 0.5);
    assert_eq!(p.timers[&2].elapsed, 0.5);
    let before = p.timers.clone();
    for bad in [
        vec![focus(), focus()],
        vec![Focus {
            velocity: [f32::INFINITY, 0., 0.],
            ..focus()
        }],
        vec![Focus {
            forward: [0.; 3],
            ..focus()
        }],
    ] {
        assert!(p.advance(&mut world, 1, 100, &c, input(600, &bad)).is_err());
        assert_eq!(p.timers, before);
        assert_eq!(p.clock, 500);
    }
    p.advance(&mut world, 1, 100, &c, input(1001, &[focus(), other]))
        .unwrap();
    assert_eq!(p.count(), 1);
    assert_eq!(p.timers[&2].elapsed, 1.001);
}

#[test]
fn road_goal_uses_its_own_scene_and_catalog_not_population_order() {
    let (p, mut world, mut content) = fixture();
    let vehicle_scene =
        name_key(content.vehicles.values().next().unwrap().bundle.as_bytes()).unwrap();
    content.splines[0].scene = vehicle_scene;
    content.splines[0].index = 3;
    content.splines[0].catalog_count = 7;
    let mut p = Population::new(p.bindings, &content).unwrap();
    let change = spawn(&mut p, &mut world, &content, 1);
    let record = change
        .records
        .iter()
        .find(|r| {
            matches!(
                r.initial,
                Some(nfs_world::replication::Initial::TrafficGoal(_))
            )
        })
        .unwrap();
    let Some(nfs_world::replication::Initial::TrafficGoal(goal)) = &record.initial else {
        panic!()
    };
    assert_eq!(
        goal.asset,
        Asset {
            bundle: 2,
            type_id: content.spline_class,
            local_index: 3
        }
    );
    assert_eq!(
        goal.catalog,
        Catalog::new(2, &[(content.spline_class, 7)]).unwrap()
    );
    let catalogs = traffic::Content::new(vec![goal.catalog.clone()]).unwrap();
    let wire = record.encode().unwrap();
    assert_eq!(
        Record::decode_traffic(wire.span(), None, Some(&catalogs))
            .unwrap()
            .record,
        *record
    );
}

#[test]
fn delegated_traffic_reports_are_isolated_retry_safe_and_retired() {
    use nfs_world::{
        frame::{Movement, MovementRecord},
        replication::vehicle::movement::Sample,
        vehicle_reports::Reports,
    };
    let (mut population, mut world, content) = fixture();
    let ids = spawn(&mut population, &mut world, &content, 1)
        .spawned
        .unwrap();
    let cars = population.delegated(&world, 1, 100);
    assert_eq!(cars, BTreeMap::from([(ids.vehicle, 1)]));
    assert!(population.delegated(&world, 2, 100).is_empty());
    assert!(world.owned_vehicles(1, 100).is_empty());
    assert!(world.vehicle_physics(1, 100, ids.vehicle).is_err());
    let mut reports = Reports::default();
    assert_eq!(
        world.ai_vehicle_physics(1, 100, 1).unwrap().controls.value3,
        0
    );
    for (index, now) in [(0, 0), (1, 100)] {
        let physics = vehicle::chassis::Motion {
            identity: (100. + index as f64 * 0.1).to_bits(),
            revision: 3000 + index * 3,
            position: [70. + index as f32, 1., 2.],
            quaternion: [0., 0., 0., 1.],
            velocity: [5., 0., 0.],
            angular_velocity: [0.; 3],
        }
        .encode([0.; 3], &vehicle::chassis::Controls::new(4).unwrap())
        .unwrap();
        let (header, body) = Sample {
            physics: physics.clone(),
            flag: false,
        }
        .encode()
        .unwrap();
        let movement = Movement {
            header: Some(header.span()),
            records: vec![MovementRecord {
                id: ids.vehicle,
                component: 0,
                data: body.span(),
            }],
        };
        let before = world.objects().snapshot();
        assert!(
            reports
                .receive_ai(&mut world, 2, 100, &movement, &cars, now)
                .is_err()
        );
        let wrong = BTreeMap::from([(ids.vehicle + 1, 1)]);
        assert!(
            reports
                .receive_ai(&mut world, 1, 100, &movement, &wrong, now)
                .is_err()
        );
        assert_eq!(world.objects().snapshot(), before);
        // The first report echoes the car and releases its held actor control;
        // later reports only echo the car.
        let first = reports
            .receive_ai(&mut world, 1, 100, &movement, &cars, now)
            .unwrap();
        assert_eq!(first.len(), if index == 0 { 2 } else { 1 });
        assert_eq!(first[0].id, ids.vehicle);
        if index == 0 {
            assert_eq!(first[1].id, ids.actor);
            assert!(matches!(
                first[1].update,
                nfs_world::replication::Update::Actor(nfs_world::replication::ActorUpdate {
                    flag_b: Some(false),
                    ..
                })
            ));
        }
        let accepted = world.ai_vehicle_physics(1, 100, 1).unwrap();
        assert_eq!(accepted, physics);
        assert_eq!(accepted.controls.value3, 4);
        assert!(
            reports
                .receive_ai(&mut world, 1, 100, &movement, &cars, now)
                .unwrap()
                .is_empty()
        );
        let mut invalid = movement.clone();
        invalid.records.push(invalid.records[0].clone());
        let before = world.objects().snapshot();
        assert!(
            reports
                .receive_ai(&mut world, 1, 100, &invalid, &cars, now)
                .is_err()
        );
        assert_eq!(world.objects().snapshot(), before);
    }
    let retained = world.ai_vehicle_physics(1, 100, 1).unwrap();
    let changes = population
        .advance(&mut world, 1, 100, &content, input(1501, &[focus()]))
        .unwrap();
    assert!(!changes.deleted.contains(&ids.vehicle));
    assert_eq!(world.ai_vehicle_physics(1, 100, 1).unwrap(), retained);
    assert_eq!(
        population.cars[&1].anchor,
        retained.decode([0.; 3]).unwrap().position
    );
    let removed = population
        .advance(&mut world, 1, 100, &content, input(1502, &[]))
        .unwrap();
    assert!(removed.deleted.contains(&ids.vehicle));
    assert!(population.delegated(&world, 1, 100).is_empty());
    let (header, body) = Sample {
        physics: retained,
        flag: false,
    }
    .encode()
    .unwrap();
    let movement = Movement {
        header: Some(header.span()),
        records: vec![MovementRecord {
            id: ids.vehicle,
            component: 0,
            data: body.span(),
        }],
    };
    assert!(
        reports
            .receive_ai(&mut world, 1, 100, &movement, &cars, 502)
            .is_err()
    );
    assert!(
        reports
            .receive_ai(&mut world, 1, 100, &movement, &BTreeMap::new(), 502)
            .unwrap()
            .is_empty()
    );
}
