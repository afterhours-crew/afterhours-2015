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
                points: (0..20).map(|i| [i as f32 * 15., 0., 0.]).collect(),
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
fn host_motion_advances_and_stops_without_granting_client_authority() {
    let (mut p, mut world, c) = fixture();
    let ids = p
        .advance(&mut world, 1, 100, &c, 0, &[[0.; 3]])
        .unwrap()
        .spawned
        .unwrap();
    let controls = vehicle::chassis::Controls::new(4).unwrap();
    let stamp = vehicle::chassis::Motion {
        identity: 100f64.to_bits(),
        revision: 3000,
        position: [0.; 3],
        quaternion: [0., 0., 0., 1.],
        velocity: [0.; 3],
        angular_velocity: [0.; 3],
    }
    .encode([0.; 3], &controls)
    .unwrap();
    p.observe_clock(0, &stamp).unwrap();
    assert!(world.vehicle_physics(1, 100, ids.vehicle).is_err());
    assert!(
        world
            .set_vehicle_physics(1, 100, ids.vehicle, stamp.clone())
            .is_err()
    );
    assert!(
        world
            .set_ai_vehicle_physics(2, 100, 1, stamp.clone())
            .is_err()
    );
    let start = p.cars[&1].anchor;
    let output = p.advance(&mut world, 1, 100, &c, 100, &[[0.; 3]]).unwrap();
    assert_eq!(output.records.len(), 1);
    assert!(p.cars[&1].anchor[0] > start[0]);
    let before_repeat = world.objects().snapshot();
    assert!(
        p.advance(&mut world, 1, 100, &c, 100, &[[0.; 3]])
            .unwrap()
            .records
            .is_empty()
    );
    assert_eq!(world.objects().snapshot(), before_repeat);
    // Player is directly ahead: no motion can cross the clearance boundary.
    let stop = p.cars[&1].anchor;
    p.advance(&mut world, 1, 100, &c, 200, &[[stop[0] + 5., 0., 0.]])
        .unwrap();
    assert_eq!(p.cars[&1].anchor, stop);
    assert_eq!(p.cars[&1].speed, 0.);
    let mut invalid = stamp.clone();
    invalid.identity_words[0] = 0x7ff80000;
    assert!(p.observe_clock(201, &invalid).is_err());
    // A long pause retires stale simulated cars instead of rewinding them
    // after the client has extrapolated across the missing interval.
    let retired = p
        .advance(&mut world, 1, 100, &c, 10_000, &[[0.; 3]])
        .unwrap();
    assert!(retired.deleted.contains(&ids.vehicle));
    assert!(!p.cars.contains_key(&1));
    assert!(world.set_ai_vehicle_physics(1, 100, 1, stamp).is_err());
}

#[test]
fn delayed_publication_integrates_the_same_road_distance_as_regular_updates() {
    let (mut regular, mut world, mut content) = fixture();
    content.splines[0].segments[0].points = vec![[50., 0., 0.], [100., 0., 0.]];
    regular
        .advance(&mut world, 1, 100, &content, 0, &[[0.; 3]])
        .unwrap();
    let physics = vehicle::chassis::Motion {
        identity: 100f64.to_bits(),
        revision: 3000,
        position: [0.; 3],
        quaternion: [0., 0., 0., 1.],
        velocity: [0.; 3],
        angular_velocity: [0.; 3],
    }
    .encode([0.; 3], &vehicle::chassis::Controls::new(4).unwrap())
    .unwrap();
    regular.observe_clock(0, &physics).unwrap();
    let mut delayed = regular.clone();
    let mut delayed_world = world.clone();
    for now in (100..=1000).step_by(100) {
        regular
            .advance(&mut world, 1, 100, &content, now, &[[0.; 3]])
            .unwrap();
    }
    delayed
        .advance(&mut delayed_world, 1, 100, &content, 1000, &[[0.; 3]])
        .unwrap();
    assert!((regular.cars[&1].anchor[0] - 51.5).abs() < 0.001);
    assert_eq!(regular.cars[&1].anchor, delayed.cars[&1].anchor);
    assert_eq!(regular.cars[&1].speed, delayed.cars[&1].speed);
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
