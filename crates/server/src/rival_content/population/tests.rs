// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::rivals::{Persona, Racer as Definition, Region, preset::Preset};
use nfs_protocol::world::rpc::Serial;
use nfs_world::content::bindings::name_key;
use nfs_world::{
    content::{Registration, Registrations, SubLevelNames},
    replication::{Initial, sublevel},
};

pub(crate) fn fixture(
    connection: u8,
) -> (Population, Players, registrations::Registry, Vec<Message>) {
    fixture_mode(connection, true)
}
fn fixture_mode(
    connection: u8,
    preloaded: bool,
) -> (Population, Players, registrations::Registry, Vec<Message>) {
    let (json, classes, items) = crate::vehicle_content::tests::fixture();
    let vehicles = vehicle_content::Content::from_json(&json, &classes).unwrap();
    let names = ["constructed/gameplay", "car/test_bundle"];
    let keys: Vec<_> = names
        .iter()
        .map(|s| name_key(s.as_bytes()).unwrap())
        .collect();
    let messages = vec![
        Message::Names(SubLevelNames {
            word: 1,
            entries: names
                .iter()
                .take(if preloaded { 2 } else { 1 })
                .enumerate()
                .map(|(i, s)| (i as u16 + 1, s.as_bytes().to_vec()))
                .collect(),
        }),
        Message::Registrations(Registrations {
            header: 4,
            entries: (1..=if preloaded { 2 } else { 1 })
                .map(|i| Registration {
                    handle: i,
                    next: i,
                    region: 0,
                    word: u32::from(i + 3),
                    byte: 0,
                    flag: true,
                    enum3: 3,
                    word2: 0,
                    bit: true,
                    enum2: 1,
                    word3: 0,
                    text: vec![],
                })
                .collect(),
        }),
    ];
    let bindings = Bindings::from_messages(&messages).unwrap();
    let data = nfs_content::rivals::Content {
        personality_count: 17,
        managers: ai::Managers {
            scene: keys[0],
            actors: 1,
            participants: 2,
        },
        personas: (0..2)
            .map(|i| Persona {
                name: format!("ID_SYNTHETIC_RACER_{i}"),
                provider_index: 11 + i,
                personality_index: 15 + i,
                preset: Preset {
                    vehicle: [1; 16],
                    color: None,
                    style: None,
                    additional: vec![],
                },
                driving: vehicle::customization::SpawnOverrides {
                    performance: 0.25,
                    grip: Some(4),
                },
            })
            .collect(),
        racers: vec![Definition {
            guid: [7; 16],
            event_type: 0,
            personas: vec![0, 1],
            regions: vec![
                Region::new(
                    vec![
                        [-10., 0., -10.],
                        [10., 0., -10.],
                        [10., 0., 10.],
                        [-10., 0., 10.],
                    ],
                    20.,
                )
                .unwrap(),
            ],
            secondary_regions: vec![],
            position: [5., 0., 2.],
            basis: [[0., 0., -1.], [0., 1., 0.], [1., 0., 0.]],
        }],
    };
    let content = Arc::new(Content {
        data,
        vehicles,
        presets: vec![Arc::new(items.clone()), Arc::new(items)],
    });
    let population = Population::new(&bindings, content).unwrap();
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
        .take(if preloaded { 2 } else { 1 })
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
    world
        .create_scenes(if preloaded { &[1, 2] } else { &[1] }, creations, &scenes)
        .unwrap();
    (
        population,
        world,
        registrations::Registry::from_messages(&messages).unwrap(),
        messages,
    )
}
fn focus() -> Focus {
    Focus {
        id: 1,
        position: [0., 1., 0.],
    }
}
fn input(now: u64, focus: &[Focus]) -> Input<'_> {
    Input { now, focus }
}
pub(crate) fn spawn(
    p: &mut Population,
    w: &mut Players,
    r: &mut registrations::Registry,
    connection: u8,
) -> Changes {
    p.advance(w, r, connection, 100, input(0, &[focus()]))
        .unwrap()
}

#[test]
fn authored_pose_personality_wander_and_mesh_namespace_are_published() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    // Simulate a player appearance registration before the rival. Its identity
    // must remain stable and the rival must use the very same mesh namespace.
    let previous = registry.prepare(&["player/other_mesh"]).unwrap().bindings[0].level;
    let c = spawn(&mut p, &mut world, &mut registry, 1);
    let ids = c.spawned.unwrap();
    assert_eq!((p.count(), world.ai_count(), c.calls.len()), (1, 1, 2));
    assert_eq!(
        p.delegated(&world, 1, 100),
        BTreeMap::from([(ids.vehicle, RIVAL_KEY_START)])
    );
    assert!(p.delegated(&world, 2, 100).is_empty());
    assert!(world.owned_vehicles(1, 100).is_empty());
    assert!(c.messages.iter().any(|m| matches!(m, Message::Names(_))));
    assert_eq!(registry.mesh_level("player/other_mesh").unwrap(), previous);
    assert!(registry.mesh_level("wheel/test").unwrap() > previous);
    let actor = c
        .records
        .iter()
        .find_map(|r| {
            if let Some(Initial::Actor(a)) = &r.initial {
                Some(a)
            } else {
                None
            }
        })
        .unwrap();
    assert!(actor.flag_a && !actor.flag_b);
    assert_eq!((actor.word_a, actor.word_b), (11, 15));
    let goal = c
        .records
        .iter()
        .find_map(|r| {
            if let Some(Initial::WanderGoal(g)) = &r.initial {
                Some(g)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(goal.flags, [false, false, true]);
    let motion = world
        .ai_vehicle_physics(1, 100, RIVAL_KEY_START)
        .unwrap()
        .decode([0.; 3])
        .unwrap();
    // The shared native spawner lifts this zero-suspension fixture by one up
    // vector. It starts with the installed reset mode, without a player timer.
    assert_eq!(motion.position, [5., 1., 2.]);
    assert_eq!(motion.velocity, [0.; 3]);
    let controls = &world
        .ai_vehicle_physics(1, 100, RIVAL_KEY_START)
        .unwrap()
        .controls;
    assert_eq!((controls.value3, controls.value6), (0, 0));
    assert!(
        p.advance(&mut world, &mut registry, 1, 100, input(0, &[focus()]))
            .unwrap()
            .records
            .is_empty()
    );
    let cleanup = p
        .advance(&mut world, &mut registry, 1, 100, input(1, &[]))
        .unwrap();
    assert_eq!(
        (
            cleanup.records.len(),
            cleanup.deleted.len(),
            world.ai_count()
        ),
        (2, 4, 0)
    );
    assert_eq!(p.authority, vehicle::authority::Registry::default());
    assert!(p.delegated(&world, 1, 100).is_empty());
    for now in 2..10 {
        p.advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(10, &[focus()]))
        .unwrap();
    assert!(c.spawned.is_some());
    let actor = c
        .records
        .iter()
        .find_map(|r| {
            if let Some(Initial::Actor(a)) = &r.initial {
                Some(a)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!((actor.word_a, actor.word_b), (12, 16));
}

#[test]
fn native_view_retention_uses_current_pose_and_strict_horizontal_distance() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    let ids = spawn(&mut p, &mut world, &mut registry, 1).spawned.unwrap();
    let motion = vehicle::chassis::Motion {
        identity: 100f64.to_bits(),
        revision: 2,
        position: [1000., 800., 0.],
        quaternion: [0., 0., 0., 1.],
        velocity: [10., 0., 0.],
        angular_velocity: [0.; 3],
    };
    let physics = motion
        .encode([0.; 3], &vehicle::chassis::Controls::new(4).unwrap())
        .unwrap();
    world
        .set_ai_vehicle_physics(1, 100, RIVAL_KEY_START, physics.clone())
        .unwrap();
    // Outside the authored region and far above its height, but within view of
    // the *reported* vehicle. The authored spawn location is irrelevant here.
    let near = Focus {
        position: [1399., -800., 0.],
        ..focus()
    };
    for now in 1..=20 {
        assert!(
            p.advance(&mut world, &mut registry, 1, 100, input(now, &[near]))
                .unwrap()
                .deleted
                .is_empty()
        );
    }
    assert_eq!(
        world.ai_vehicle_physics(1, 100, RIVAL_KEY_START).unwrap(),
        physics
    );
    let edge = Focus {
        position: [1400., 0., 0.],
        ..focus()
    };
    for now in 21..=29 {
        p.advance(&mut world, &mut registry, 1, 100, input(now, &[edge]))
            .unwrap();
    }
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(30, &[edge]))
        .unwrap();
    assert!(c.deleted.contains(&ids.vehicle));
    assert_eq!(p.count(), 0);
}

#[test]
fn late_error_rolls_back_meshes_objects_cursors_and_authority() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    Arc::make_mut(&mut p.content).data.managers.actors = 0;
    let before = world.objects().snapshot();
    let meshes = registry.clone();
    assert!(
        p.advance(&mut world, &mut registry, 1, 100, input(0, &[focus()]))
            .is_err()
    );
    assert_eq!(world.objects().snapshot(), before);
    assert_eq!(registry, meshes);
    assert_eq!(
        (p.next, p.cohort, p.clock, p.owner),
        (RIVAL_KEY_START, 0, None, None)
    );
    assert_eq!(p.racers, vec![Racer::default()]);
    assert_eq!(p.authority, vehicle::authority::Registry::default());
    Arc::make_mut(&mut p.content).data.managers.actors = 1;
    assert!(
        spawn(&mut p, &mut world, &mut registry, 1)
            .spawned
            .is_some()
    );
    let before = world.objects().snapshot();
    for (connection, now, focus) in [
        (2, 1, vec![]),
        (1, 1, vec![focus(), focus()]),
        (
            1,
            1,
            vec![Focus {
                id: 1,
                position: [f32::NAN; 3],
            }],
        ),
    ] {
        assert!(
            p.advance(
                &mut world,
                &mut registry,
                connection,
                100,
                input(now, &focus)
            )
            .is_err()
        );
        assert_eq!(world.objects().snapshot(), before);
    }
}

#[test]
fn empty_focus_retires_all_cohorts_and_a_fresh_session_has_no_rivals() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    let data = Arc::make_mut(&mut p.content);
    data.data.racers = (0..12)
        .map(|i| {
            let mut r = data.data.racers[0].clone();
            r.guid = [i + 1; 16];
            r
        })
        .collect();
    p.racers = vec![Racer::default(); 12];
    for now in 0..12 {
        p.advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    assert_eq!(p.count(), 12);
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(12, &[]))
        .unwrap();
    assert_eq!((p.count(), world.ai_count(), c.deleted.len()), (0, 0, 48));
    let (fresh, other, _, _) = fixture(1);
    assert_eq!((fresh.count(), other.ai_count()), (0, 0));
}

#[test]
fn first_rival_registers_a_missing_vehicle_bundle_and_reuses_it_after_retirement() {
    let (mut p, mut world, mut registry, _) = fixture_mode(1, false);
    let key = name_key(b"car/test_bundle").unwrap();
    assert!(world.objects().scene(key).is_none());
    let changes = spawn(&mut p, &mut world, &mut registry, 1);
    let scene = world.objects().scene(key).unwrap();
    assert_eq!(changes.records[0].id, scene);
    assert!(matches!(
        changes.records[0].initial,
        Some(Initial::SubLevel { .. })
    ));
    assert!(changes.messages.iter().any(
        |m| matches!(m,Message::Names(n) if n.entries.iter().any(|(_,s)|s==b"car/test_bundle"))
    ));
    p.advance(&mut world, &mut registry, 1, 100, input(1, &[]))
        .unwrap();
    for now in 2..10 {
        p.advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    let next = p
        .advance(&mut world, &mut registry, 1, 100, input(10, &[focus()]))
        .unwrap();
    assert!(next.spawned.is_some());
    assert!(next.messages.is_empty());
    assert_eq!(world.objects().scene(key), Some(scene));
    assert!(
        !next
            .records
            .iter()
            .any(|r| matches!(r.initial, Some(Initial::SubLevel { .. })))
    );
}

#[test]
fn all_populations_share_the_world_budget_and_resume_after_a_slot_is_freed() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    // An independently owned AI lifetime occupies one world-budget slot.
    let mut other = p.clone();
    other.next = 1;
    spawn(&mut other, &mut world, &mut registry, 1);
    let data = Arc::make_mut(&mut p.content);
    let template = data.data.racers[0].clone();
    data.data.racers = (0..128)
        .map(|i| {
            let mut r = template.clone();
            r.guid = [i + 1; 16];
            r
        })
        .collect();
    p.racers = vec![Racer::default(); 128];
    for now in 0..150 {
        p.advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    assert_eq!((p.count(), other.count(), world.ai_count()), (127, 1, 128));
    other
        .advance(&mut world, &mut registry, 1, 100, input(150, &[]))
        .unwrap();
    assert!(
        p.advance(&mut world, &mut registry, 1, 100, input(150, &[focus()]))
            .unwrap()
            .spawned
            .is_some()
    );
    assert_eq!((p.count(), world.ai_count()), (128, 128));
}
