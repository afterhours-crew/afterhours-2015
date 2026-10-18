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
            index: 0,
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

#[test]
fn spawn_goal_managers_and_retirement_are_owned_and_repeat_safe() {
    let (mut p, mut world, c) = fixture();
    let initial = world.objects().snapshot();
    let change = p.advance(&mut world, 1, 100, &c, 0, &[[0.; 3]]).unwrap();
    let ids = change.spawned.unwrap();
    assert_eq!(change.calls.len(), 2);
    assert_eq!(change.records.len(), 6);
    assert_eq!(p.count(), 1);
    assert!(matches!(
        p.advance(&mut world, 2, 100, &c, 1, &[]),
        Err(Error::UnknownObject)
    ));
    assert_eq!(p.count(), 1);
    assert!(world.ai_ids(2, 100, 1).is_none());
    assert_eq!(world.ai_ids(1, 100, 1), Some(ids));
    assert!(
        p.advance(&mut world, 1, 100, &c, 0, &[[0.; 3]])
            .unwrap()
            .records
            .is_empty()
    );
    let retired = p.advance(&mut world, 1, 100, &c, 1, &[]).unwrap();
    assert_eq!(retired.deleted.len(), 4);
    assert_eq!(retired.records.len(), 2);
    assert!(retired.calls.is_empty());
    assert_eq!(p.count(), 0);
    assert!(world.ai_ids(1, 100, 1).is_none());
    assert_eq!(world.objects().snapshot(), initial);
    assert!(
        p.advance(&mut world, 1, 100, &c, 1, &[])
            .unwrap()
            .deleted
            .is_empty()
    );
    assert!(matches!(
        p.advance(&mut world, 1, 100, &c, 0, &[]),
        Err(Error::Bound)
    ));
}

#[test]
fn late_manager_failure_rolls_back_car_ids_authority_and_clock() {
    let (mut p, mut world, mut c) = fixture();
    let before = world.objects().snapshot();
    c.managers.actors = 0;
    assert!(p.advance(&mut world, 1, 100, &c, 10, &[[0.; 3]]).is_err());
    assert_eq!(world.objects().snapshot(), before);
    assert_eq!(p.count(), 0);
    assert_eq!((p.next, p.due, p.clock), (1, 0, 0));
    assert_eq!(p.authority, vehicle::authority::Registry::default());
    c.managers.actors = 1;
    assert!(
        p.advance(&mut world, 1, 100, &c, 10, &[[0.; 3]])
            .unwrap()
            .spawned
            .is_some()
    );
}

#[test]
fn placement_respects_tags_floor_spacing_and_population_bound() {
    let (mut p, mut world, mut c) = fixture();
    assert!(p.select(&c, &[[0., 20., 0.]]).is_none());
    c.splines[0].tags = vec![[9; 16]];
    assert!(p.select(&c, &[[0.; 3]]).is_none());
    c.splines[0].tags.clear();
    for i in 0..30 {
        p.advance(&mut world, 1, 100, &c, i * 1000, &[[i as f32 * 5., 0., 0.]])
            .unwrap();
        assert!(p.count() <= MAX_CARS);
    }
    let points: Vec<_> = p.cars.values().map(|c| c.anchor).collect();
    for (i, point) in points.iter().enumerate() {
        for other in &points[i + 1..] {
            assert!(distance_squared(*point, *other) >= 900.);
        }
    }
    assert!(p.count() > 1);
    p.advance(&mut world, 1, 100, &c, 200_000, &[]).unwrap();
    assert_eq!(p.count(), 0);
    let basis = road_basis([0.; 3], [3., 1., 4.]).unwrap();
    vehicle::orientation::quaternion_from_basis(basis).unwrap();
    assert!(road_basis([0.; 3], [0.; 3]).is_none());
}

#[test]
fn nearby_traffic_survives_age_and_retirement_grace_resets_on_return() {
    let (mut population, mut world, mut content) = fixture();
    let ids = population
        .advance(&mut world, 1, 100, &content, 0, &[[0.; 3]])
        .unwrap()
        .spawned
        .unwrap();
    content.density = 0.; // Isolate retention from further population growth.
    let anchor = population.cars[&1].anchor;
    for now in [90_000, 180_000, 1_000_000] {
        let changes = population
            .advance(&mut world, 1, 100, &content, now, &[anchor])
            .unwrap();
        assert!(changes.deleted.is_empty());
        assert_eq!(world.ai_ids(1, 100, 1), Some(ids));
    }
    let far = [anchor[0] + 451., anchor[1], anchor[2]];
    for now in [1_000_100, 1_002_099] {
        assert!(
            population
                .advance(&mut world, 1, 100, &content, now, &[far])
                .unwrap()
                .deleted
                .is_empty()
        );
    }
    // A second nearby player protects the car and clears the earlier timer.
    assert!(
        population
            .advance(&mut world, 1, 100, &content, 1_002_100, &[far, anchor])
            .unwrap()
            .deleted
            .is_empty()
    );
    assert!(
        population
            .advance(&mut world, 1, 100, &content, 1_002_101, &[far])
            .unwrap()
            .deleted
            .is_empty()
    );
    assert!(
        population
            .advance(&mut world, 1, 100, &content, 1_004_100, &[far])
            .unwrap()
            .deleted
            .is_empty()
    );
    let retired = population
        .advance(&mut world, 1, 100, &content, 1_004_101, &[far])
        .unwrap();
    assert_eq!(retired.deleted.len(), 4);
    assert_eq!(population.count(), 0);
    assert!(world.ai_ids(1, 100, 1).is_none());
}

#[test]
fn spawning_respects_every_players_near_region() {
    let (population, _, content) = fixture();
    let focus = [[0.; 3], [600., 0., 0.], [1200., 0., 0.]];
    // Together these exclusion circles cover the complete installed segment.
    assert!(population.select(&content, &focus).is_none());
    let selected = population.select(&content, &[[0.; 3]]).unwrap();
    assert!(horizontal_distance_squared(selected.pose.position, [0.; 3]) > SPAWN_DISTANCE.powi(2));
    assert!(horizontal_distance_squared(selected.pose.position, [0.; 3]) < RETAIN_DISTANCE.powi(2));
}

#[test]
fn delegated_traffic_reports_are_isolated_retry_safe_and_retired() {
    use nfs_world::{
        frame::{Movement, MovementRecord},
        replication::vehicle::movement::Sample,
        vehicle_reports::Reports,
    };
    let (mut population, mut world, content) = fixture();
    let ids = population
        .advance(&mut world, 1, 100, &content, 0, &[[0.; 3]])
        .unwrap()
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
        assert_eq!(
            reports
                .receive_ai(&mut world, 1, 100, &movement, &cars, now)
                .unwrap()
                .len(),
            1
        );
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
    assert!(
        population
            .advance(&mut world, 1, 100, &content, 500, &[[0.; 3]])
            .unwrap()
            .records
            .is_empty()
    );
    assert_eq!(world.ai_vehicle_physics(1, 100, 1).unwrap(), retained);
    assert_eq!(
        population.cars[&1].anchor,
        retained.decode([0.; 3]).unwrap().position
    );
    let removed = population
        .advance(&mut world, 1, 100, &content, 501, &[])
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
