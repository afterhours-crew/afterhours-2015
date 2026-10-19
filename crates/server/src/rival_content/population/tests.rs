// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::rivals::{Persona, Racer as Definition, Region, preset::Preset};
use nfs_protocol::world::rpc::Serial;
use nfs_world::content::bindings::name_key;
use nfs_world::{
    content::{Registration, Registrations, SubLevelNames},
    replication::{Initial, Update, sublevel},
    road_graph::{Direction, LaneInput},
};

/// A 6 km by 3 km loop of two-way roads whose south side runs along z = 2,
/// through the authored racer spawn. Zero width keeps the fixture's cars on
/// the centre line; the lane offset has its own test.
fn graph() -> Arc<Graph> {
    road_loop(0.)
}

fn road_loop(width: f32) -> Arc<Graph> {
    let nodes = vec![
        [-3000., 0., 2.],
        [3000., 0., 2.],
        [3000., 0., 3002.],
        [-3000., 0., 3002.],
    ];
    let lanes = (0..4)
        .map(|i| LaneInput {
            start: i,
            end: (i + 1) % 4,
            direction: Direction::Any,
            points: vec![nodes[i], nodes[(i + 1) % 4]],
            widths: vec![width],
        })
        .collect();
    Arc::new(Graph::new(nodes, lanes).unwrap())
}

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
        graph: Some(graph()),
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
    Input {
        now,
        focus,
        random: 12_345,
    }
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
    assert_eq!((p.count(), world.ai_count(), c.calls.len()), (1, 1, 4));
    assert!(matches!(
        c.calls[3],
        HostRpc::WorldActivation(nfs_world::world_activation::Notification {
            participant: Some(p),
            call: nfs_world::world_activation::Call::StreamedParticipant,
            ..
        }) if p == ids.participant
    ));
    // Official rivals are world cars, not garage displays: the root's
    // customization flag stays clear so the client keeps updating the car.
    let Initial::Vehicle { creation, .. } = &world.objects().get(ids.vehicle).unwrap().initial
    else {
        panic!()
    };
    let root = creation
        .fields
        .iter()
        .find_map(|f| match f {
            vehicle::Initial::Root(r) => Some(r),
            _ => None,
        })
        .unwrap();
    assert!(!root.fine_flag && !root.flag);
    let spawn_velocity = creation
        .fields
        .iter()
        .find_map(|f| match f {
            vehicle::Initial::Chassis(c) => {
                Some(c.vectors[1].map(|v| v.map_or(0., f32::from_bits)))
            }
            _ => None,
        })
        .unwrap();
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
    // Created holding control; the car's first client report releases it.
    assert!(actor.flag_a && actor.flag_b);
    assert_eq!((actor.word_a, actor.word_b), (11, 15));
    assert_rival_driving_mode(&c, ids.actor);
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
    // vector. It starts with the installed reset mode, without a player timer,
    // already moving at the wander speed along its heading (E981).
    assert_eq!(motion.position, [5., 1., 2.]);
    let speed = spawn_velocity.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((speed - WANDER_SPEED).abs() < 1e-3, "{speed}");
    // The chassis state carries the same velocity at its packed precision.
    for (state, spawn) in motion.velocity.iter().zip(spawn_velocity) {
        assert!((state - spawn).abs() <= 1. / 32., "{state} {spawn}");
    }
    assert!(motion.velocity.iter().any(|v| *v != 0.));
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
    assert_rival_driving_mode(&c, c.spawned.unwrap().actor);
}

fn assert_rival_driving_mode(changes: &Changes, actor: u16) {
    let record = changes
        .records
        .iter()
        .find(|r| r.id == actor && r.initial.is_some())
        .unwrap();
    // Decode the published creation as a client would. A roaming rival must
    // keep racer physics on both its first spawn and subsequent persona cycle.
    let encoded = record.encode().unwrap();
    let decoded = Record::decode(encoded.span(), None).unwrap();
    assert!(matches!(
        decoded.record.update,
        Update::Actor(ref update) if update.word_c == Some(2)
    ));
}

#[test]
fn native_view_retention_uses_current_pose_and_strict_horizontal_distance() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    let ids = spawn(&mut p, &mut world, &mut registry, 1).spawned.unwrap();
    let motion = vehicle::chassis::Motion {
        identity: 100f64.to_bits(),
        revision: 2,
        position: [1000., 300., 0.],
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
    // The car stays within the 3D hand-back distance (720 m).
    let near = Focus {
        position: [1399., -300., 0.],
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
        (p.next, p.slots, p.cohort, p.clock, p.owner),
        (RIVAL_KEY_START, 1, 0, None, None)
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
    // The bundle scene follows the slot's participant and actor and precedes
    // the car.
    let index = changes
        .records
        .iter()
        .position(|r| matches!(r.initial, Some(Initial::SubLevel { .. })))
        .unwrap();
    assert_eq!((index, changes.records[index].id), (2, scene));
    assert!(matches!(
        changes.records[3].initial,
        Some(Initial::Vehicle { .. })
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
    (other.next, other.slots) = (1, 1_000);
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

/// A racer authored 1 km east of the focus, facing it.
fn far_fixture() -> (Population, Players, registrations::Registry) {
    let (mut p, world, registry, _) = fixture_mode(1, false);
    let racer = &mut Arc::make_mut(&mut p.content).data.racers[0];
    racer.position = [1000., 0., 2.];
    racer.basis = [[0., 0., 1.], [0., 1., 0.], [-1., 0., 0.]];
    (p, world, registry)
}

fn place(world: &mut Players, key: u64, position: [f32; 3], velocity: [f32; 3]) {
    let motion = vehicle::chassis::Motion {
        identity: 100f64.to_bits(),
        revision: 2,
        position,
        quaternion: [0., 0., 0., 1.],
        velocity,
        angular_velocity: [0.; 3],
    };
    let physics = motion
        .encode([0.; 3], &vehicle::chassis::Controls::new(4).unwrap())
        .unwrap();
    world.set_ai_vehicle_physics(1, 100, key, physics).unwrap();
}

#[test]
fn host_drives_rivals_and_hands_cars_over_within_790_m_and_back_beyond_800_m() {
    let (mut p, mut world, mut registry) = far_fixture();
    // The racer activates at its authored spawn, 1 km away: participant and
    // actor only, bound to the managers, without a car or reset tracking.
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(0, &[focus()]))
        .unwrap();
    let slot = c.activated.unwrap();
    assert!(c.spawned.is_none());
    assert_eq!(
        (p.count(), world.ai_count(), world.ai_slot_count()),
        (1, 0, 1)
    );
    assert_eq!((c.records.len(), c.calls.len()), (2, 3));
    assert!(
        !c.calls
            .iter()
            .any(|call| matches!(call, HostRpc::WorldActivation(_)))
    );
    assert!(c.messages.is_empty());
    assert!(p.delegated(&world, 1, 100).is_empty());
    assert_eq!(
        p.locations(&world, 1, 100).unwrap(),
        vec![(slot.participant, [1000., 0., 2.])]
    );
    // Another session sees nothing.
    assert!(p.locations(&world, 2, 100).unwrap().is_empty());

    // The host drives it west along the road at the wander speed.
    p.advance(&mut world, &mut registry, 1, 100, input(5_000, &[focus()]))
        .unwrap();
    let [(participant, moved)] = p.locations(&world, 1, 100).unwrap()[..] else {
        panic!()
    };
    assert_eq!(participant, slot.participant);
    assert!(
        (moved[0] - (1000. - 5. * WANDER_SPEED)).abs() < 0.01,
        "{moved:?}"
    );
    assert_eq!((moved[1], moved[2]), (0., 2.));

    // Within 790 m of the focus the client gets a car at the simulated pose,
    // moving west at the wander speed, under the same participant.
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(10_000, &[focus()]))
        .unwrap();
    assert!(c.activated.is_none());
    let first = c.spawned.unwrap();
    assert_eq!(
        (first.participant, first.actor),
        (slot.participant, slot.actor)
    );
    assert!(c.messages.iter().any(|m| matches!(m, Message::Names(_))));
    assert!(matches!(
        c.calls[..],
        [HostRpc::WorldActivation(nfs_world::world_activation::Notification {
            participant: Some(p),
            call: nfs_world::world_activation::Call::StreamedParticipant,
            ..
        })] if p == slot.participant
    ));
    let key = RIVAL_KEY_START;
    assert_eq!(
        p.delegated(&world, 1, 100),
        BTreeMap::from([(first.vehicle, key)])
    );
    let motion = world
        .ai_vehicle_physics(1, 100, key)
        .unwrap()
        .decode([0.; 3])
        .unwrap();
    let x = 1000. - 10. * WANDER_SPEED;
    // The fixture's zero suspension lifts the car by one up vector.
    assert!(
        (motion.position[0] - x).abs() < 0.01,
        "{:?}",
        motion.position
    );
    assert_eq!((motion.position[1], motion.position[2]), (1., 2.));
    assert!((motion.velocity[0] + WANDER_SPEED).abs() <= 1. / 32.);
    assert!(motion.velocity[1].abs() <= 1. / 32. && motion.velocity[2].abs() <= 1. / 32.);
    let goal = c
        .records
        .iter()
        .find_map(|r| match &r.initial {
            Some(Initial::WanderGoal(g)) => Some(g),
            _ => None,
        })
        .unwrap();
    assert_eq!(goal.flags, [false, false, true]);
    // While the client drives, the map follows the car and the host waits.
    place(&mut world, key, [500., 0., 2.], [-20., 0., 0.]);
    p.advance(&mut world, &mut registry, 1, 100, input(20_000, &[focus()]))
        .unwrap();
    assert_eq!(
        p.locations(&world, 1, 100).unwrap(),
        vec![(slot.participant, [500., 0., 2.])]
    );

    // At exactly 800 m the car stays; beyond it the client loses the car but
    // the participant, actor and region retention stay. The binding and goal
    // are cleared before the reset-volume leave.
    place(&mut world, key, [-800., 1., 0.], [-20., 0., 0.]);
    assert!(
        p.advance(&mut world, &mut registry, 1, 100, input(20_001, &[focus()]))
            .unwrap()
            .deleted
            .is_empty()
    );
    place(&mut world, key, [-850., 0., 2.], [-20., 0., 0.]);
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(20_002, &[focus()]))
        .unwrap();
    assert!(c.leave.is_empty());
    assert_eq!(c.deleted.len(), 2);
    assert!(c.deleted.contains(&first.vehicle));
    assert!(!c.deleted.contains(&slot.participant));
    assert!(matches!(
        c.calls[..],
        [HostRpc::WorldActivation(
            nfs_world::world_activation::Notification {
                call: nfs_world::world_activation::Call::StreamedParticipantLeave,
                ..
            }
        )]
    ));
    assert!(c.records.iter().any(|r| r.id == slot.participant));
    assert!(p.delegated(&world, 1, 100).is_empty());
    assert_eq!(
        (p.count(), world.ai_count(), world.ai_slot_count()),
        (1, 0, 1)
    );
    assert_eq!(world.ai_slot(1, 100, 1).unwrap().car, None);
    assert_eq!(p.authority, vehicle::authority::Registry::default());
    // The host continues west from the car's last reported pose.
    assert_eq!(
        p.locations(&world, 1, 100).unwrap(),
        vec![(slot.participant, [-850., 0., 2.])]
    );
    p.advance(&mut world, &mut registry, 1, 100, input(22_002, &[focus()]))
        .unwrap();
    let [(_, resumed)] = p.locations(&world, 1, 100).unwrap()[..] else {
        panic!()
    };
    assert!(
        (resumed[0] - (-850. - 2. * WANDER_SPEED)).abs() < 0.01,
        "{resumed:?}"
    );

    // A focus inside 790 m gets the next car under the same participant.
    let near = Focus {
        position: [-700., 1., 0.],
        ..focus()
    };
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(22_003, &[near]))
        .unwrap();
    let second = c.spawned.unwrap();
    assert_eq!(second.participant, slot.participant);
    assert_ne!(second.vehicle, first.vehicle);
    assert!(c.messages.is_empty());
    assert_eq!(
        p.delegated(&world, 1, 100),
        BTreeMap::from([(second.vehicle, key + 1)])
    );
    // Leaving every view and region retires the slot with its car.
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(22_004, &[]))
        .unwrap();
    assert_eq!(c.leave.len(), 2);
    for id in [slot.participant, slot.actor, second.vehicle] {
        assert!(c.deleted.contains(&id));
    }
    assert_eq!(
        (p.count(), world.ai_count(), world.ai_slot_count()),
        (0, 0, 0)
    );
}

#[test]
fn hand_over_distances_include_height() {
    let (mut p, mut world, mut registry) = far_fixture();
    p.advance(&mut world, &mut registry, 1, 100, input(0, &[focus()]))
        .unwrap()
        .activated
        .unwrap();
    // 700 m away horizontally but 922 m in 3D: no car yet. The racer stays
    // active through its region until its cohort is evaluated again.
    let above = Focus {
        position: [300., 600., 2.],
        ..focus()
    };
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(1, &[above]))
        .unwrap();
    assert!(c.spawned.is_none() && c.deleted.is_empty());
    assert_eq!((p.count(), world.ai_count()), (1, 0));
    let level = Focus {
        position: [300., 0., 2.],
        ..focus()
    };
    let car = p
        .advance(&mut world, &mut registry, 1, 100, input(2, &[level]))
        .unwrap()
        .spawned
        .unwrap();
    // A car 700 m away horizontally and 832 m in 3D is removed, as by the
    // native AI director; the racer keeps its participant.
    place(
        &mut world,
        RIVAL_KEY_START,
        [-700., 450., 2.],
        [-20., 0., 0.],
    );
    let c = p
        .advance(&mut world, &mut registry, 1, 100, input(3, &[focus()]))
        .unwrap();
    assert!(c.deleted.contains(&car.vehicle));
    assert!(!c.deleted.contains(&car.participant));
    assert_eq!(
        (p.count(), world.ai_count(), world.ai_slot_count()),
        (1, 0, 1)
    );
}

#[test]
fn late_hand_over_error_rolls_back_registrations_and_the_bundle_scene() {
    let (mut p, mut world, mut registry) = far_fixture();
    p.advance(&mut world, &mut registry, 1, 100, input(0, &[focus()]))
        .unwrap();
    // The car's blueprint lookup fails after its bundle and meshes registered.
    Arc::make_mut(&mut p.content).data.managers.scene = 99;
    let (before, meshes, population) = (world.objects().snapshot(), registry.clone(), p.clone());
    assert!(
        p.advance(&mut world, &mut registry, 1, 100, input(10_000, &[focus()]))
            .is_err()
    );
    assert_eq!(world.objects().snapshot(), before);
    assert_eq!(registry, meshes);
    assert_eq!(
        (p.next, p.slots, p.clock, &p.racers),
        (
            population.next,
            population.slots,
            population.clock,
            &population.racers
        )
    );
    assert_eq!(p.authority, vehicle::authority::Registry::default());
}

#[test]
fn racers_without_a_nearby_road_stay_inactive() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    Arc::make_mut(&mut p.content).data.racers[0].position = [5., 0., 500.];
    for now in 0..20 {
        let c = p
            .advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
        assert!(c.activated.is_none() && c.records.is_empty());
    }
    assert_eq!((p.count(), world.ai_slot_count()), (0, 0));
}

#[test]
fn cars_follow_the_road_slope() {
    let rise = 0.1f32;
    let length = (1. + rise * rise).sqrt();
    let [right, up, forward] = road_basis([1., rise, 0.]).unwrap();
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6);
    assert!(close(forward, [1. / length, rise / length, 0.]));
    assert!(close(up, [-rise / length, 1. / length, 0.]));
    assert!(close(right, [0., 0., -1.]));
    assert_eq!(road_basis([0., 1., 0.]), None);
    assert_eq!(road_basis([0.; 3]), None);
}

#[test]
fn cars_are_handed_over_in_the_driving_lane() {
    let (mut p, mut world, mut registry) = far_fixture();
    p.graph = road_loop(10.);
    p.advance(&mut world, &mut registry, 1, 100, input(0, &[focus()]))
        .unwrap();
    p.advance(&mut world, &mut registry, 1, 100, input(10_000, &[focus()]))
        .unwrap()
        .spawned
        .unwrap();
    let motion = world
        .ai_vehicle_physics(1, 100, RIVAL_KEY_START)
        .unwrap()
        .decode([0.; 3])
        .unwrap();
    // Driving west (forward -x), up x forward is +z: the car starts a quarter
    // of the 10 m road on the other side of the centre line (z = 2).
    assert!(
        (motion.position[0] - (1000. - 10. * WANDER_SPEED)).abs() < 0.01,
        "{:?}",
        motion.position
    );
    assert!(
        (motion.position[2] - (2. - 2.5)).abs() < 0.01,
        "{:?}",
        motion.position
    );
    // The rival map follows the car.
    let [(_, marker)] = p.locations(&world, 1, 100).unwrap()[..] else {
        panic!()
    };
    assert!((marker[2] + 0.5).abs() < 0.01, "{marker:?}");
}

#[test]
fn new_bundle_scenes_name_the_root_scene() {
    use nfs_world::replication::sublevel::{Blueprint, Rpc, root};
    let (mut p, mut world, mut registry, _) = fixture_mode(1, false);
    let records = world
        .create_scenes(
            &[0],
            vec![
                root::creation(
                    Rpc {
                        selector: 0,
                        serial: Serial::new(0).unwrap(),
                    },
                    [None; root::REFERENCES],
                )
                .unwrap(),
            ],
            &root::content().unwrap(),
        )
        .unwrap();
    let root_ghost = records[0].id;
    assert_eq!(world.objects().scene(root::CONTENT_KEY), Some(root_ghost));
    let changes = spawn(&mut p, &mut world, &mut registry, 1);
    let prefix = changes
        .records
        .iter()
        .find_map(|r| match &r.initial {
            Some(Initial::SubLevel { prefix, .. }) => Some(prefix.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        prefix.blueprint,
        Some(Blueprint {
            ghost: root_ghost,
            sub_id: 1
        })
    );
    // Published records stay encodable with the reference.
    for record in &changes.records {
        record.encode().unwrap();
    }
}

#[test]
fn rival_cars_carry_the_official_audio_state() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    let ids = spawn(&mut p, &mut world, &mut registry, 1).spawned.unwrap();
    let object = format!("{:?}", world.objects().get(ids.vehicle).unwrap());
    assert!(
        object.contains("head: [0, 0, 0], guid: [0, 0, 0, 0], tail: [0, 2, 0, 0, 0, 0]"),
        "{object}"
    );
}

#[test]
fn rival_limit_caps_active_racers_and_zero_turns_them_off() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    let data = Arc::make_mut(&mut p.content);
    let template = data.data.racers[0].clone();
    data.data.racers = (0..6)
        .map(|i| {
            let mut r = template.clone();
            r.guid = [i + 1; 16];
            r
        })
        .collect();
    p.racers = vec![Racer::default(); 6];
    let (mut capped, mut off) = (p.clone().with_limit(Some(2)), p.with_limit(Some(0)));
    let (mut other, mut meshes) = (world.clone(), registry.clone());
    for now in 0..30 {
        capped
            .advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
        off.advance(&mut other, &mut meshes, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    assert_eq!((capped.count(), world.ai_slot_count()), (2, 2));
    assert_eq!(
        (off.count(), other.ai_slot_count(), other.ai_count()),
        (0, 0, 0)
    );
}

/// Racers like the installed ones: one per spawn, each with its own small
/// region around that spawn, so only a racer spawned at the player is one of
/// the game's own there.
fn spread(p: &mut Population, spawns: &[[f32; 3]]) {
    let data = Arc::make_mut(&mut p.content);
    let template = data.data.racers[0].clone();
    data.data.racers = spawns
        .iter()
        .enumerate()
        .map(|(i, &[x, y, z])| {
            let mut r = template.clone();
            r.guid = [i as u8 + 1; 16];
            r.position = [x, y, z];
            r.regions = vec![
                Region::new(
                    vec![
                        [x - 10., y, z - 10.],
                        [x + 10., y, z - 10.],
                        [x + 10., y, z + 10.],
                        [x - 10., y, z + 10.],
                    ],
                    20.,
                )
                .unwrap(),
            ];
            r
        })
        .collect();
    p.racers = vec![Racer::default(); spawns.len()];
}

fn at(x: f32, z: f32) -> Focus {
    Focus {
        id: 1,
        position: [x, 1., z],
    }
}

fn active(p: &Population) -> Vec<usize> {
    (0..p.racers.len())
        .filter(|&i| p.racers[i].active.is_some())
        .collect()
}

/// Advance `ticks` milliseconds around one player.
fn drive(
    p: &mut Population,
    world: &mut Players,
    registry: &mut registrations::Registry,
    now: &mut u64,
    player: Focus,
    ticks: u64,
) -> Vec<Changes> {
    (0..ticks)
        .map(|_| {
            let c = p
                .advance(world, registry, 1, 100, input(*now, &[player]))
                .unwrap();
            *now += 1;
            c
        })
        .collect()
}

#[test]
fn a_chosen_count_adds_the_nearest_installed_racers_once_each() {
    let (mut p, world, registry, _) = fixture(1);
    spread(
        &mut p,
        &[
            [5., 0., 2.],
            [3000., 0., 2.],
            [1000., 0., 2.],
            [-1500., 0., 2.],
            [2000., 0., 2.],
            // 4.2 km away: beyond FILL_IN.
            [-3000., 0., 3002.],
            // The nearest spawn, but with no road near it.
            [100., 0., 500.],
        ],
    );
    let run = |limit: Option<u16>| {
        let mut p = p.clone().with_limit(limit);
        let (mut world, mut registry, mut now) = (world.clone(), registry.clone(), 0);
        drive(&mut p, &mut world, &mut registry, &mut now, focus(), 30);
        assert_eq!(world.ai_slot_count(), p.count());
        (active(&p), p.racers[6].stranded)
    };
    // The game's own number here is one: only the first racer's region holds
    // the player, and native rules never try the stranded spawn.
    assert_eq!(run(None), (vec![0], false));
    assert_eq!(run(Some(1)), (vec![0], false));
    // Added nearest first (1 km, 1.5 km, 2 km), after the game's own racer;
    // the stranded spawn is tried once and then skipped.
    assert_eq!(run(Some(4)), (vec![0, 2, 3, 4], true));
    // More than the installed racers near the player allow: every racer
    // within 3.5 km once, never a second instance of one.
    assert_eq!(run(Some(15)), (vec![0, 1, 2, 3, 4], true));
    assert_eq!(run(Some(0)), (vec![], false));
}

#[test]
fn a_chosen_count_keeps_racers_to_4500_m_and_refills_near_the_player() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    spread(
        &mut p,
        &[[1000., 0., 2.], [-1500., 0., 2.], [-2500., 0., 3002.]],
    );
    let mut p = p.with_limit(Some(2));
    let mut now = 0;
    drive(&mut p, &mut world, &mut registry, &mut now, at(0., 0.), 5);
    // The third racer's spawn is 3.9 km away: not added.
    assert_eq!(active(&p), [0, 1]);
    // 4.27 km from the first racer: still kept, so nothing is added.
    drive(
        &mut p,
        &mut world,
        &mut registry,
        &mut now,
        at(-3000., 1500.),
        5,
    );
    assert_eq!(active(&p), [0, 1]);
    // 5 km from the first racer: retired, and the racer whose spawn is now
    // 500 m away takes its place and gets a car.
    let changes = drive(
        &mut p,
        &mut world,
        &mut registry,
        &mut now,
        at(-3000., 3002.),
        5,
    );
    assert_eq!(active(&p), [1, 2]);
    assert_eq!(changes.iter().filter(|c| c.activated.is_some()).count(), 1);
    assert_eq!(changes.iter().filter(|c| c.spawned.is_some()).count(), 1);
    assert!(p.racers[2].active.as_ref().unwrap().car.is_some());
    assert_eq!((world.ai_slot_count(), world.ai_count()), (2, 1));
    // Without players every racer is retired, as natively.
    p.advance(&mut world, &mut registry, 1, 100, input(now, &[]))
        .unwrap();
    assert_eq!(
        (p.count(), world.ai_slot_count(), world.ai_count()),
        (0, 0, 0)
    );
}

#[test]
fn the_games_own_racers_take_the_place_of_added_ones_at_the_count() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    spread(&mut p, &[[1000., 0., 2.], [-2000., 0., 2.]]);
    let mut p = p.with_limit(Some(1));
    let mut now = 0;
    drive(&mut p, &mut world, &mut registry, &mut now, at(0., 0.), 5);
    assert_eq!(active(&p), [0]);
    // In the second racer's region the first, 3 km away and without a car,
    // is kept only by the count; the region racer replaces it.
    drive(
        &mut p,
        &mut world,
        &mut registry,
        &mut now,
        at(-2000., 0.),
        12,
    );
    assert_eq!(active(&p), [1]);
    assert_eq!(world.ai_slot_count(), 1);
    // A racer with a car (driven by the client) is never replaced.
    let (mut p, mut world, mut registry, _) = fixture(1);
    spread(&mut p, &[[500., 0., 2.], [-200., 0., 2.]]);
    let mut p = p.with_limit(Some(1));
    let mut now = 0;
    drive(&mut p, &mut world, &mut registry, &mut now, at(0., 0.), 3);
    assert_eq!(active(&p), [1]);
    assert!(p.racers[1].active.as_ref().unwrap().car.is_some());
    // The first racer's region holds the player now, but the second has its
    // car and is within the hand-back distance.
    drive(
        &mut p,
        &mut world,
        &mut registry,
        &mut now,
        at(500., 0.),
        12,
    );
    assert_eq!(active(&p), [1]);
}

#[test]
fn chosen_rivals_still_get_cars_when_traffic_holds_its_whole_share() {
    let (mut p, mut world, mut registry, _) = fixture(1);
    let template = p.content.data.racers[0].clone();
    let racers = |count: usize| {
        (0..count)
            .map(|i| {
                let mut r = template.clone();
                r.guid = [i as u8 + 1; 16];
                r
            })
            .collect::<Vec<_>>()
    };
    // Another population holds every AI car ambient traffic may use.
    let share = crate::traffic_content::population::MAX_CARS;
    let mut traffic = p.clone();
    (traffic.next, traffic.slots) = (1, 1_000);
    Arc::make_mut(&mut traffic.content).data.racers = racers(share);
    traffic.racers = vec![Racer::default(); share];
    Arc::make_mut(&mut p.content).data.racers = racers(20);
    p.racers = vec![Racer::default(); 20];
    let mut p = p.with_limit(Some(crate::ai_settings::MAX_RIVALS));
    for now in 0..share as u64 + 10 {
        traffic
            .advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    assert_eq!(world.ai_count(), share);
    for now in 0..30 {
        p.advance(&mut world, &mut registry, 1, 100, input(now, &[focus()]))
            .unwrap();
    }
    let cars = p
        .racers
        .iter()
        .filter(|r| r.active.as_ref().is_some_and(|a| a.car.is_some()))
        .count();
    assert_eq!((p.count(), cars, world.ai_count()), (15, 15, ai::MAX_AI));
}
