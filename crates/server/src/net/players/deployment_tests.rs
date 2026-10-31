// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{
    content::{Batch, WorldContent},
    scene_roles::SceneRoles,
};
use nfs_world::{
    content::{LoadLevel, RegistrationDefinition, SubLevelNames, bindings::name_key},
    replication::{
        entity::creation::Asset,
        sublevel::{Content, Kind, Profile, root},
    },
};

fn content() -> WorldContent {
    content_with(None)
}

#[test]
fn seen_map_binding_requires_a_registered_scene_and_a_plain_rpc_serializer() {
    let mut content = content_with(Some(&[Kind::Noop, Kind::Rpc, Kind::RpcBool]));
    let scene = content.roles.as_ref().unwrap().spawn_points.unwrap();
    content.roles.as_mut().unwrap().seen_points = Some(crate::scene_roles::SceneRpc {
        scene,
        serializer: 1,
    });
    content.validate_runtime().unwrap();
    for (scene, serializer) in [(scene, 0), (scene, 2), (scene, 999), (9_999_999, 1)] {
        let mut wrong = content.clone();
        wrong.roles.as_mut().unwrap().seen_points =
            Some(crate::scene_roles::SceneRpc { scene, serializer });
        assert!(wrong.validate_runtime().is_err());
    }
}

#[test]
fn garage_livery_cache_update_uses_owned_scene_and_reliable_reply_queue() {
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::{bits::BitWriter, replication::sublevel};
    let (mut listener, _) = binding_refresh_listener();
    let mut kinds = vec![Kind::Noop; 9];
    kinds[8] = Kind::Rpc;
    let scenes = Content::new(vec![(101, Profile::new(false, &kinds).unwrap())]).unwrap();
    let creation = sublevel::ordinary::creation(
        1,
        101,
        None,
        Serial::new(7).unwrap(),
        &sublevel::ordinary::unpopulated(scenes.profile(101).unwrap()).unwrap(),
        &scenes,
    )
    .unwrap();
    listener
        .players
        .create_scenes(&[1], vec![creation], &scenes)
        .unwrap();
    let scene = listener.players.objects().scene(101).unwrap();
    let mut roles = content().roles.unwrap();
    roles.garage = 101;
    listener.roles = Some(roles);
    let key = b"BakedLivery_0X0123456789ABCDEF";
    let mut payload = BitWriter::new();
    payload
        .put(0, 9)
        .put(0, 32)
        .put(101, 32)
        .put(0, 32)
        .put(key.len() as u64, 10);
    for byte in key {
        payload.put(u64::from(*byte), 8);
    }
    payload.put(5, 5);
    let mut rpc = BitWriter::new();
    rpc.put(0, 32)
        .put(0, 32)
        .put(1, 8)
        .put(scene.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    let send = |listener: &mut PlayerListener, sequence| {
        let wire = frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages: vec![frame::Message::Opaque {
                        index: 47,
                        body: rpc.span(),
                    }],
                }],
            )
            .build()
            .unwrap();
        listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        })
    };
    assert!(send(&mut listener, 1));
    assert_eq!(listener.unsupported_rpcs, 0);
    assert_eq!(listener.pending_rpcs.len(), 1);
    assert!(send(&mut listener, 1));
    assert_eq!(
        listener.pending_rpcs.len(),
        1,
        "same reliable message is not applied twice"
    );
    assert!(send(&mut listener, 2));
    assert_eq!(
        listener.pending_rpcs.len(),
        2,
        "a later update invalidates again"
    );
    let notification = listener.pending_rpcs[0];
    listener
        .pending_rpcs
        .resize(nfs_world::session::MAX_RPC_QUEUE, notification);
    assert!(!send(&mut listener, 3));
    assert_eq!(
        listener.pending_rpcs.len(),
        nfs_world::session::MAX_RPC_QUEUE
    );
    listener.pending_rpcs.clear();
    assert!(
        send(&mut listener, 3),
        "capacity refusal must preserve the input for retry"
    );
    assert_eq!(listener.pending_rpcs.len(), 1);
}

#[test]
fn owned_driving_timer_signals_are_consumed_by_the_configured_lifetime() {
    use crate::garage_logic::{GarageLogic, Which};
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::{
        logic::{EntityRef, Message},
        replication::sublevel,
    };
    let (mut listener, participant) = binding_refresh_listener();
    let scenes = Content::new(vec![(101, sublevel::gameplay::profile().unwrap())]).unwrap();
    let creation = sublevel::ordinary::creation(
        1,
        101,
        None,
        Serial::new(0).unwrap(),
        &sublevel::ordinary::unpopulated(scenes.profile(101).unwrap()).unwrap(),
        &scenes,
    )
    .unwrap();
    listener
        .players
        .create_scenes(&[1], vec![creation], &scenes)
        .unwrap();
    let mut policy = crate::garage_logic::tests::content_json();
    let definition = policy["spawn"]["entities"][3].clone();
    let role = definition["capture_id"].as_u64().unwrap() as u16;
    let empty = serde_json::json!({"scene_updates":[],"entities":[],"events":[]});
    policy["spawn"] = empty.clone();
    policy["spawn"]["entities"] = serde_json::json!([definition]);
    policy["entry"] = empty.clone();
    policy["vehicles"] = empty;
    policy["driving_timer"] = serde_json::json!({"role":role,"start":{"entity":2,"event":37},"stop":{"entity":2,"event":38}});
    for (path, value) in [
        ("/driving_timer/role", serde_json::json!(999)),
        ("/driving_timer/start/entity", serde_json::json!(3)),
        ("/driving_timer/stop/event", serde_json::json!(37)),
        ("/driving_timer/stop/event", serde_json::json!(0x200_0000)),
    ] {
        let mut invalid = policy.clone();
        *invalid.pointer_mut(path).unwrap() = value;
        assert!(GarageLogic::from_json(&invalid).is_err(), "{path}");
    }
    let logic = GarageLogic::from_json(&policy).unwrap();
    let mut ghosts = BTreeMap::new();
    logic
        .produce(
            Which::Spawn,
            &mut listener.players,
            participant,
            &mut ghosts,
        )
        .unwrap();
    let ghost = ghosts[&role];
    listener.logic_ghosts.insert(participant, ghosts);
    listener.logic = Some(std::sync::Arc::new(logic));
    let message = Message::Reached {
        target: EntityRef { ghost, entity: 2 },
        event: 37,
        player: 0,
    };
    let logic = listener.logic.as_ref().unwrap();
    assert!(
        logic
            .driving_timer_owner(
                &listener.players,
                &listener.logic_ghosts,
                HOST_SELECTOR as u8,
                listener.persona,
                &message
            )
            .is_ok()
    );
    for (connection, persona, message) in [
        (HOST_SELECTOR as u8, listener.persona + 1, message.clone()),
        (HOST_SELECTOR as u8 + 1, listener.persona, message.clone()),
        (
            HOST_SELECTOR as u8,
            listener.persona,
            Message::Reached {
                target: EntityRef { ghost, entity: 2 },
                event: 37,
                player: 1,
            },
        ),
        (
            HOST_SELECTOR as u8,
            listener.persona,
            Message::Reached {
                target: EntityRef {
                    ghost: ghost + 1,
                    entity: 2,
                },
                event: 37,
                player: 0,
            },
        ),
    ] {
        assert!(
            logic
                .driving_timer_owner(
                    &listener.players,
                    &listener.logic_ghosts,
                    connection,
                    persona,
                    &message
                )
                .is_err()
        );
    }
    for (sequence, event) in [(1, 38), (2, 37), (3, 38), (4, 38)] {
        let body = Message::Reached {
            target: EntityRef { ghost, entity: 2 },
            event,
            player: 0,
        }
        .encode()
        .unwrap();
        let wire = frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages: vec![frame::Message::Opaque {
                        index: 82,
                        body: body.span(),
                    }],
                }],
            )
            .build()
            .unwrap();
        listener.world_ms = u64::from(sequence) * 1000;
        assert!(listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span()
        }));
    }
    assert_eq!(listener.unsupported_logic, 0);
    let state = listener.driving_time.state(participant).unwrap();
    assert_eq!(state.completed, 0, "no owned world vehicle in this fixture");
    assert!(state.last.is_none());
}

#[test]
fn nitrous_acknowledgements_commit_with_the_reliable_queue_and_roll_back_when_full() {
    use crate::vehicle_content::population::{Owner, tests as population_tests};
    use nfs_world::{
        bits::BitWriter,
        replication::{Initial, Update, vehicle},
    };
    let persona = 0x100000002;
    let (mut population, mut players, vehicles, layout, items, _) = population_tests::fixture();
    let player = players
        .create(
            HOST_SELECTOR as u8,
            persona,
            Request {
                name: b"Nitrous driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(HOST_SELECTOR as u8, persona, participant)
        .unwrap();
    let layout = population_tests::world_layout(&layout);
    let inventory = population_tests::inventory(items);
    let owner = || Owner {
        connection: HOST_SELECTOR as u8,
        persona,
        participant,
    };
    population
        .spawn(
            &mut players,
            owner(),
            inventory.clone(),
            &vehicles,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let section = population
        .spawn_world(&mut players, owner(), &inventory, &vehicles, &layout)
        .unwrap();
    let car = section.records[0].id;
    let Initial::Vehicle { creation, .. } = &players.objects().get(car).unwrap().initial else {
        panic!()
    };
    let selector = creation
        .fields
        .iter()
        .find_map(|field| match field {
            vehicle::Initial::Index { rpc, .. } => Some(rpc.selector),
            _ => None,
        })
        .unwrap();
    players.sync_player_views().unwrap();
    players.sync_player_manager().unwrap();
    let mut listener = PlayerListener::new(persona);
    listener.players = players;
    listener.world_cars.insert(car, (participant, true));
    let make_frame = |sequence, method: u32| {
        let mut payload = BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(method.into(), 32)
            .put(12.5f32.to_bits().into(), 32)
            .put(127, 7);
        let mut rpc = BitWriter::new();
        rpc.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(car.into(), 13)
            .put(10, 9)
            .put_span(payload.span());
        frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages: vec![frame::Message::Opaque {
                        index: 47,
                        body: rpc.span(),
                    }],
                }],
            )
            .build()
            .unwrap()
    };
    let first = make_frame(0, 0);
    let before = listener.players.objects().snapshot();
    listener.pending.resize(
        MAX_PENDING,
        Section {
            float_bits: None,
            flag: false,
            deleted: section.deleted,
            setup: None,
            records: section.records,
        },
    );
    assert!(!listener.frame(Queued {
        advertised: None,
        frame: 0,
        data: first.span()
    }));
    assert_eq!(listener.players.objects().snapshot(), before);
    assert_eq!(listener.pending.len(), MAX_PENDING);
    listener.pending.clear();
    for (sequence, method) in [(0, 0), (1, 1), (2, 0), (3, 1)] {
        let wire = make_frame(sequence, method);
        assert!(listener.frame(Queued {
            advertised: None,
            frame: sequence.into(),
            data: wire.span()
        }));
        assert_eq!(listener.pending.len(), 1);
        let section = listener.pending.pop_front().unwrap();
        nfs_world::session::replication_frame(&section, &[], 0).unwrap();
        let record = section.records.iter().find(|r| r.id == car).unwrap();
        let Update::Vehicle { fields, .. } = &record.update else {
            panic!()
        };
        assert_eq!(
            fields.iter().flatten().collect::<Vec<_>>(),
            vec![&vehicle::Update::Index {
                index: Some(u8::from(method == 0)),
                value: None,
            }]
        );
        let after = listener.players.objects().snapshot();
        // Reliable delivery repeats do not execute a transition twice.
        assert!(listener.frame(Queued {
            advertised: None,
            frame: sequence.into(),
            data: wire.span()
        }));
        assert!(listener.pending.is_empty());
        assert_eq!(listener.players.objects().snapshot(), after);
    }
    assert_eq!(listener.unsupported_rpcs, 0);
}

#[test]
fn sustained_driving_keeps_physics_and_minimap_in_one_bounded_frame() {
    use crate::vehicle_content::population::{Owner, tests as population_tests};
    use nfs_world::{
        bits::BitWriter,
        replication::{
            Update,
            sublevel::ordinary,
            vehicle::{Vector, movement::Sample},
        },
        world_activation::{Catalog, Site},
    };
    let persona = 0x100000003;
    let (mut population, mut players, vehicles, layout, items, _) = population_tests::fixture();
    let player = players
        .create(
            HOST_SELECTOR as u8,
            persona,
            Request {
                name: b"Moving driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(HOST_SELECTOR as u8, persona, participant)
        .unwrap();
    let layout = population_tests::world_layout(&layout);
    let owner = || Owner {
        connection: HOST_SELECTOR as u8,
        persona,
        participant,
    };
    let inventory = population_tests::inventory(items);
    population
        .spawn(
            &mut players,
            owner(),
            inventory.clone(),
            &vehicles,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let car = population
        .spawn_world(&mut players, owner(), &inventory, &vehicles, &layout)
        .unwrap()
        .records[0]
        .id;
    let key = 99;
    let profiles = Content::new(vec![(
        key,
        Profile::new(
            false,
            &[
                Kind::Noop,
                Kind::RpcBool,
                Kind::Rpc,
                Kind::RpcFourReferences,
            ],
        )
        .unwrap(),
    )])
    .unwrap();
    let scene = ordinary::creation(
        99,
        key,
        None,
        nfs_protocol::world::rpc::Serial::new(0).unwrap(),
        &ordinary::unpopulated(profiles.profile(key).unwrap()).unwrap(),
        &profiles,
    )
    .unwrap();
    players
        .create_scenes(&[99], vec![scene], &profiles)
        .unwrap();
    let mut roles = content().roles.unwrap();
    roles.free_roam = Some(Catalog {
        volumes: vec![],
        presence: Site {
            scene_key: key,
            serializer: 1,
        },
        streamed: Site {
            scene_key: key,
            serializer: 2,
        },
        location: Site {
            scene_key: key,
            serializer: 3,
        },
    });
    // Complete the initial view and map creation before steady movement.
    players.sync_player_views().unwrap();
    players.sync_player_manager().unwrap();
    let map = players
        .sync_location_service(
            HOST_SELECTOR as u8,
            persona,
            &BTreeMap::from([(participant, car)]),
            [0.; 3],
            roles.free_roam.as_ref().unwrap().location,
        )
        .unwrap()[0]
        .id;
    let mut sample = Sample {
        physics: players
            .vehicle_physics(HOST_SELECTOR as u8, persona, car)
            .unwrap(),
        flag: true,
    };
    let mut listener = PlayerListener::new(persona);
    listener.players = players;
    listener.roles = Some(roles);
    listener.world_cars.insert(car, (participant, false));
    // At ten accepted reports per second, a second frame for the minimap
    // would grow the queue even with a conservative ten-frame/second drain.
    let mut previous = None;
    for step in 0..600u32 {
        // A missing optional map binding must neither reject valid movement
        // nor partially change the map. Repeated failure stays diagnosable;
        // restoring the service must recover without reconnecting.
        let map_missing = (100..150).contains(&step);
        listener
            .roles
            .as_mut()
            .unwrap()
            .free_roam
            .as_mut()
            .unwrap()
            .location
            .serializer = if map_missing { 9999 } else { 3 };
        let map_before = listener.players.objects().get(map).unwrap().current.clone();
        listener.world_ms = u64::from(step) * 100;
        let x = step as f32 * 8.;
        sample.physics.identity_words = [(5f64.to_bits() >> 32) as u32, 0, step + 150];
        sample.physics.position = Vector::from_position([x, 2., 40.], [0.; 3], 7).unwrap();
        let (header, body) = sample.encode().unwrap();
        let mut wire = BitWriter::new();
        wire.put(1 << frame::MOVEMENT, 6)
            .put_span(header.span())
            .put(1, 6)
            .put(u64::from(car), 16)
            .put(0, 4)
            .put(body.len() as u64, 16)
            .put_span(body.span());
        wire.align();
        if step == 300 {
            // A full queue must roll back both position and echo admission;
            // retrying the same sample after capacity returns publishes it.
            listener
                .pending
                .resize(MAX_PENDING, previous.clone().unwrap());
            let before = listener.players.objects().snapshot();
            assert!(!listener.frame(Queued {
                advertised: None,
                frame: step as u16,
                data: wire.span(),
            }));
            assert_eq!(listener.players.objects().snapshot(), before);
            assert_eq!(listener.pending.len(), MAX_PENDING);
            listener.pending.clear();
        }
        assert!(listener.frame(Queued {
            advertised: None,
            frame: step as u16,
            data: wire.span()
        }));
        assert_eq!(
            listener.pending.len(),
            1,
            "report {step} must not amplify queued frames"
        );
        let section = listener.pending.pop_front().unwrap();
        assert!(
            nfs_world::session::replication_frame(&section, &[], 0)
                .unwrap()
                .len()
                <= nfs_world::application::OUTBOUND_FRAME_BITS
        );
        assert!(section.records.iter().any(|r| r.id == car));
        if map_missing {
            assert!(!section.records.iter().any(|r| r.id == map));
            assert_eq!(
                listener.players.objects().get(map).unwrap().current,
                map_before
            );
        } else if step > 0 {
            assert!(section.records.iter().any(|r| r.id == map));
            let Update::ParticipantLocationMap(location) =
                &listener.players.objects().get(map).unwrap().current
            else {
                panic!()
            };
            assert_eq!(
                location.groups.as_ref().unwrap()[0].entries[0].x,
                nfs_world::replication::location_map::Coordinate::from_world(x).unwrap()
            );
        }
        assert_eq!(listener.refused, usize::from(step >= 300));
        previous = Some(section);
    }
    assert_eq!(listener.optional_map_failures, [50, 0, 50]);
    // Clearing world ownership still sends map removal and service unbinding
    // without relying on another movement report.
    listener.world_cars.clear();
    let empty = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(0),
                messages: vec![frame::Message::CreatePlayer {
                    name: b"Moving driver".to_vec(),
                    flag: false,
                    slot: 0,
                }],
            }],
        )
        .build()
        .unwrap();
    assert!(listener.frame(Queued {
        advertised: None,
        frame: 600,
        data: empty.span()
    }));
    assert_eq!(listener.pending.len(), 1);
    let cleared = listener.pending.pop_front().unwrap();
    assert_eq!(cleared.records.len(), 2);
    assert_eq!(cleared.records[0].id, map);
    assert!(!cleared.records.iter().any(|r| r.id == car));
}

#[test]
fn spawn_presentation_waits_for_capacity_with_poll_and_rejects_foreign_owner() {
    use nfs_world::participants::{Call, HostRpc, Notification};
    let content = content_with(Some(&SPAWN_LAYOUT));
    let key = content.roles.as_ref().unwrap().gameplay;
    let mut listener = PlayerListener::new(101);
    listener.initialize_world(&content).unwrap();
    let player = listener
        .players
        .create(
            HOST_SELECTOR as u8,
            101,
            Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = listener
        .players
        .join(HOST_SELECTOR as u8, 101, player)
        .unwrap()
        .unwrap()
        .id;
    listener.participants.begin(participant).unwrap();
    let mut v = crate::garage_logic::tests::presentation_json();
    v["world_spawn"]["events"][0]["target"] = serde_json::json!({"scene_content_key":key});
    let logic = std::sync::Arc::new(crate::garage_logic::GarageLogic::from_json(&v).unwrap());
    assert!(
        PlayerListener::presentation_events(
            Some(&logic),
            crate::garage_logic::Which::WorldSpawn,
            &listener.players,
            &listener.logic_ghosts,
            202,
            participant
        )
        .is_err()
    );
    listener.logic = Some(logic);
    let notification = Notification {
        endpoint: listener.spawn_points.bindings().unwrap().assign,
        participant,
        call: Call::Assign(1),
    };
    listener.pending_spawns.push_back((100, notification));
    // The teleport waits for the assignment to leave in an earlier tick; a
    // participant with no scheduled assignment teleports when due.
    let assign = listener.spawn_points.bindings().unwrap().assign;
    let teleport = move |participant: u16, vehicle: u16| nfs_world::teleport::Command {
        endpoint: assign,
        participant,
        vehicle,
        position: [1.0, 2.0, 3.0],
        speed: 0.,
        basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };
    listener
        .pending_teleports
        .push_back((50, teleport(participant, 300)));
    assert_eq!(listener.due_teleport(100), None);
    let filler = HostRpc::Participant(notification);
    listener
        .pending_rpcs
        .resize(nfs_world::session::MAX_RPC_QUEUE - 1, filler);
    let before = listener.level_poll.clone();
    listener.advance_level_poll(100).unwrap();
    assert_eq!(listener.level_poll, before);
    assert_eq!(listener.pending_spawns.len(), 1);
    assert_eq!(
        listener.pending_rpcs.len(),
        nfs_world::session::MAX_RPC_QUEUE - 1
    );
    listener.pending_rpcs.clear();
    listener.advance_level_poll(100).unwrap();
    assert!(matches!(listener.pending_rpcs[0], HostRpc::LevelPoll(_)));
    assert_eq!(listener.pending_rpcs[1], filler);
    // Spawn presentation waits for the zone report.
    assert_eq!(listener.pending_rpcs.len(), 2);
    assert!(listener.pending_spawns.is_empty());
    assert_eq!(listener.due_teleport(100), None);
    assert_eq!(
        listener.due_teleport(101),
        None,
        "pending assignment must reach the host queue first"
    );
    listener.pending_rpcs.clear();
    assert_eq!(
        listener.due_teleport(101),
        None,
        "bootstrap content and scenes also precede the command"
    );
    listener.pending_content.clear();
    listener.pending.clear();
    listener.pending_entries.clear();
    assert_eq!(listener.due_teleport(101), Some(teleport(participant, 300)));
    listener.teleport_sent();
    listener
        .pending_teleports
        .push_back((200, teleport(participant + 1, 301)));
    assert_eq!(listener.due_teleport(199), None);
    assert_eq!(
        listener.due_teleport(200),
        Some(teleport(participant + 1, 301))
    );
    listener.teleport_sent();
    listener.pending_rpcs.clear();
    listener.advance_level_poll(101).unwrap();
    assert!(listener.pending_rpcs.is_empty());
}

#[test]
fn installed_scenes_fill_version_five_without_masking_invalid_overrides() {
    let content = content();
    let catalog = nfs_content::scenes::Catalog {
        level: String::from_utf8(content.level.level.clone()).unwrap(),
        profiles: content.scene_profiles.clone().unwrap(),
        launchers: content.launchers.clone().unwrap(),
    };
    let mut document = content.to_json();
    document["version"] = serde_json::json!(5);
    document.as_object_mut().unwrap().remove("scene_profiles");
    document.as_object_mut().unwrap().remove("launchers");
    assert!(WorldContent::from_json(&document).is_err());
    let loaded = WorldContent::from_json_with_scenes(&document, Some(&catalog)).unwrap();
    assert_eq!(loaded, content);
    loaded.validate_runtime().unwrap();
    let mut invalid = document.clone();
    invalid["scene_profiles"] = serde_json::json!(null);
    assert!(WorldContent::from_json_with_scenes(&invalid, Some(&catalog)).is_err());
    let mut invalid = document.clone();
    invalid["launchers"] = serde_json::json!([{"content_key":1,"serializers":[999]}]);
    assert!(WorldContent::from_json_with_scenes(&invalid, Some(&catalog)).is_err());
    let mut other = catalog.clone();
    other.level = "different/level".into();
    assert!(WorldContent::from_json_with_scenes(&document, Some(&other)).is_err());
    // Existing complete v4 and explicit v5 documents remain authoritative.
    let mut complete = content.to_json();
    assert_eq!(
        WorldContent::from_json_with_scenes(&complete, Some(&other)).unwrap(),
        content
    );
    complete["version"] = serde_json::json!(5);
    assert_eq!(
        WorldContent::from_json_with_scenes(&complete, Some(&other)).unwrap(),
        content
    );
}
/// `spawn` adds a fifteenth scene with this field layout and names it as the
/// SpawnPoints role.
fn content_with(spawn: Option<&[Kind]>) -> WorldContent {
    let count = 14 + usize::from(spawn.is_some());
    let names: Vec<_> = (0..count)
        .map(|i| format!("constructed/scene-{i}").into_bytes())
        .collect();
    let keys: Vec<_> = names.iter().map(|name| name_key(name).unwrap()).collect();
    let roles = SceneRoles {
        level: b"constructed/level".to_vec(),
        gameplay: keys[10],
        startup: keys[11],
        garage: keys[12],
        progression: keys[13],
        traffic: keys[..10].try_into().unwrap(),
        customization_timer: Asset {
            bundle: 1,
            type_id: 2,
            local_index: 0,
        },
        spawn_points: spawn.map(|_| keys[14]),
        free_roam: None,
        bank: None,
        seen_points: None,
        streaming_gate: Asset {
            bundle: 2,
            type_id: 3,
            local_index: 0,
        },
    };
    let mut profiles: Vec<_> = root::content()
        .unwrap()
        .profiles()
        .map(|(key, p)| (key, p.clone()))
        .collect();
    for (i, key) in keys.iter().copied().enumerate() {
        let mut kinds = vec![Kind::Noop];
        if i == 14 {
            kinds = spawn.unwrap().to_vec();
        } else if i == 10 {
            kinds.resize(52, Kind::Noop);
            kinds[1] = Kind::RpcReferences;
            kinds[8..13].fill(Kind::Rpc);
            // Level root poll endpoint (field 51).
            kinds[51] = Kind::Rpc;
        } else if i == 11 {
            kinds.resize(115, Kind::Noop);
            for index in [6, 11, 7, 8, 25, 21, 18, 19, 114, 95, 90, 89, 86, 87] {
                kinds[index] = Kind::RpcReferences;
            }
        }
        profiles.push((key, Profile::new(false, &kinds).unwrap()));
    }
    let profiles = Content::new(profiles).unwrap();
    let launchers = Some(nfs_world::launchers::Catalog::new(vec![], &profiles).unwrap());
    WorldContent {
        level: LoadLevel {
            level: roles.level.clone(),
            attributes: vec![],
            word: 0,
            text: vec![],
            flags: [false; 3],
            entries: vec![],
            final_word: 0,
        },
        roles: Some(roles),
        scene_profiles: Some(profiles),
        launchers,
        batches: vec![
            Batch::Names(SubLevelNames {
                word: 0,
                entries: names
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| (i as u16, name))
                    .collect(),
            }),
            Batch::Registrations {
                header: 0,
                definitions: (0..count)
                    .map(|_| RegistrationDefinition {
                        region: 0,
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
            },
        ],
    }
}

#[test]
fn explicit_scene_roles_bind_atomically_and_reject_incomplete_or_changed_content() {
    let content = content();
    content.validate_runtime().unwrap();
    assert_eq!(
        WorldContent::from_json(&content.to_json()).unwrap(),
        content
    );
    let mut listener = PlayerListener::new(101);
    let mut missing = content.clone();
    missing.roles = None;
    assert!(missing.validate_runtime().is_err());
    assert!(listener.initialize_world(&missing).is_err());
    assert!(listener.players.objects().is_empty());
    assert!(listener.roles.is_none());
    let mut invalid = content.clone();
    invalid.roles.as_mut().unwrap().gameplay = 12345;
    assert!(invalid.validate_runtime().is_err());
    assert!(listener.initialize_world(&invalid).is_err());
    assert!(listener.players.objects().is_empty());
    assert!(listener.pending.is_empty());
    listener.initialize_world(&content).unwrap();
    assert_eq!(listener.players.objects().len(), 16);
    assert!(matches!(
        listener.players.objects().get(16).unwrap().initial,
        nfs_world::replication::Initial::PlayerManager
    ));
    assert_eq!(listener.roles, content.roles);
    let before = listener.players.objects().snapshot();
    let queued = listener.pending.len();
    assert!(listener.initialize_world(&content).is_err());
    assert_eq!(listener.players.objects().snapshot(), before);
    assert_eq!(listener.pending.len(), queued);
    let mut other = PlayerListener::new(202);
    other.initialize_world(&content).unwrap();
    assert_eq!(other.players.objects().snapshot(), before);
}

const SPAWN_LAYOUT: [Kind; 7] = [
    Kind::Noop,
    Kind::Rpc,
    Kind::RpcBool,
    Kind::Rpc,
    Kind::Rpc,
    Kind::Rpc,
    Kind::Rpc,
];

#[test]
fn optional_spawn_points_role_binds_its_scene_or_stays_unsupported() {
    let content = content_with(Some(&SPAWN_LAYOUT));
    content.validate_runtime().unwrap();
    assert_eq!(
        WorldContent::from_json(&content.to_json()).unwrap(),
        content
    );
    let key = content.roles.as_ref().unwrap().spawn_points.unwrap();
    let mut listener = PlayerListener::new(101);
    listener.initialize_world(&content).unwrap();
    let scene = listener.players.objects().scene(key).unwrap();
    let b = listener.spawn_points.bindings().unwrap();
    assert_eq!((b.assign.scene, b.occupied.scene), (scene, scene));
    assert_ne!(b.assign.selector, b.occupied.selector);
    let mut absent = PlayerListener::new(101);
    absent
        .initialize_world(&super::deployment_tests::content())
        .unwrap();
    assert!(absent.spawn_points.bindings().is_none());
    // A role naming a scene of another shape leaves spawns unsupported.
    let mut other = PlayerListener::new(101);
    other
        .initialize_world(&content_with(Some(&[Kind::Noop, Kind::Rpc])))
        .unwrap();
    assert!(other.spawn_points.bindings().is_none());
    // The role must name a scene that the content creates.
    let mut unknown = content;
    unknown.roles.as_mut().unwrap().spawn_points = Some(12345);
    assert!(unknown.validate_runtime().is_err());
}

#[test]
fn glass_reports_on_a_world_car_assign_one_spawn() {
    use nfs_world::logic::{EntityRef, Message};
    let mut listener = PlayerListener::new(101);
    listener
        .initialize_world(&content_with(Some(&SPAWN_LAYOUT)))
        .unwrap();
    let event = |event, ghost| Message::Reached {
        event,
        target: EntityRef { ghost, entity: 2 },
        player: 0,
    };
    let mut spawn = listener.spawn_points.clone();
    let mut cars = BTreeMap::from([(300, (40, false))]);
    for other in [event(12_540_227, 300), event(21_578_436, 301)] {
        assert_eq!(
            PlayerListener::world_car_ready(&other, &mut cars, &mut spawn),
            Ok(None)
        );
    }
    let mut unbound = nfs_world::spawn_points::SpawnPoints::default();
    assert_eq!(
        PlayerListener::world_car_ready(&event(21_578_436, 300), &mut cars, &mut unbound),
        Ok(Some(None))
    );
    assert_eq!(cars[&300], (40, false));
    let Ok(Some(Some(first))) =
        PlayerListener::world_car_ready(&event(21_578_436, 300), &mut cars, &mut spawn)
    else {
        panic!("assignment expected");
    };
    assert_eq!(
        (first.participant, first.call),
        (40, nfs_world::participants::Call::Assign(1))
    );
    assert_eq!(first.endpoint, spawn.bindings().unwrap().assign);
    // The assignment waits until due, then leaves in order (E765 regression:
    // an assignment 145 ms after exit was never requested).
    let mut pending = std::collections::VecDeque::from([(1_000, first), (1_500, first)]);
    assert!(PlayerListener::release_spawns(&mut pending, 999).is_empty());
    assert_eq!(
        PlayerListener::release_spawns(&mut pending, 1_200),
        vec![first]
    );
    assert_eq!(pending.len(), 1);
    assert_eq!(
        PlayerListener::release_spawns(&mut pending, 2_000),
        vec![first]
    );
    assert!(pending.is_empty());
    // The three reports are glass signals, which the glass model answers
    // before the assignment (regression: E751 never reached the assignment).
    for report in [28_404_286, 14_703_462, 21_578_436] {
        assert!(crate::glass::Glass::recognizes(&event(report, 300)));
    }
    for repeat in [14_703_462, 28_404_286, 21_578_436] {
        assert_eq!(
            PlayerListener::world_car_ready(&event(repeat, 300), &mut cars, &mut spawn),
            Ok(Some(None))
        );
    }
    assert_eq!(spawn.assigned(40), Some(1));
}

#[test]
fn leaving_free_roam_clears_the_spawn_assignment_once() {
    use nfs_world::participants::{Call, HostRpc};
    let mut listener = PlayerListener::new(101);
    listener
        .initialize_world(&content_with(Some(&SPAWN_LAYOUT)))
        .unwrap();
    // No assignment: nothing to clear (and no reserve needed).
    assert!(listener.spawn_clear(40).unwrap().is_empty());
    let assign = listener.spawn_points.assign(40, 1).unwrap();
    let rpcs = listener.spawn_clear(40).unwrap();
    assert_eq!(rpcs.len(), 1);
    let HostRpc::Participant(n) = rpcs[0] else {
        panic!("participant notification expected");
    };
    assert_eq!(
        (n.endpoint, n.participant, n.call),
        (assign.endpoint, 40, Call::Leave)
    );
    // The clear is idempotent at the model level and the counter continues.
    assert_eq!(listener.spawn_points.next_value(40), 2);
    // The lifetime ended: the intro transfer car is assigned again (career
    // start: 1 at 38618, Leave at 298117, 2 at 563021 ms).
    assert!(!listener.spawn_points.subscribed(40));
    let mut unbound = PlayerListener::new(101);
    unbound.initialize_world(&content_with(None)).unwrap();
    assert!(unbound.spawn_clear(40).unwrap().is_empty());
}

#[test]
fn level_poll_binds_the_level_root_and_waits_for_a_participant() {
    let content = content();
    let gameplay = content.roles.as_ref().unwrap().gameplay;
    let mut listener = PlayerListener::new(101);
    listener.initialize_world(&content).unwrap();
    let scene = listener.players.objects().scene(gameplay).unwrap();
    assert_eq!(listener.level_poll.endpoint().unwrap().scene, scene);
    let queued = listener.pending_rpcs.len();
    listener.advance_level_poll(10_000).unwrap();
    assert_eq!(listener.pending_rpcs.len(), queued);
}

#[test]
fn the_world_car_is_registered_when_garage_logic_records_follow_it() {
    use nfs_world::replication::Record;
    let content = content();
    let mut listener = PlayerListener::new(101);
    listener.initialize_world(&content).unwrap();
    let scenes: Vec<Record> = listener.pending[0]
        .records
        .iter()
        .take(2)
        .cloned()
        .collect();
    let car = scenes[0].id;
    let section = Section {
        float_bits: None,
        flag: false,
        deleted: vec![],
        setup: None,
        records: scenes,
    };
    let mut cars = BTreeMap::new();
    PlayerListener::register_world_car(&mut cars, &[40], &section).unwrap();
    assert_eq!(cars.get(&car), Some(&(40, false)));
    // Several or no exiting participants register nothing.
    let mut none = BTreeMap::new();
    PlayerListener::register_world_car(&mut none, &[40, 41], &section).unwrap();
    PlayerListener::register_world_car(&mut none, &[], &section).unwrap();
    assert!(none.is_empty());
    let mut full: BTreeMap<u16, (u16, bool)> = (1..=128).map(|g| (g + 1000, (1, false))).collect();
    assert_eq!(
        PlayerListener::register_world_car(&mut full, &[40], &section),
        Err(replication::Error::Bound)
    );
}

#[test]
fn the_garage_car_is_deleted_once_the_client_reports_the_world_car() {
    use nfs_world::logic::{EntityRef, Message};
    let report = |ghost| Message::Reached {
        event: 28_404_286,
        target: EntityRef { ghost, entity: 2 },
        player: 0,
    };
    let mut deferred = BTreeMap::from([(205, vec![203])]);
    assert!(PlayerListener::garage_car_deletion(&report(204), &mut deferred).is_none());
    let section = PlayerListener::garage_car_deletion(&report(205), &mut deferred).unwrap();
    assert_eq!(section.deleted, vec![203]);
    assert!(section.records.is_empty() && section.setup.is_none());
    // The later reports of the same car delete nothing more.
    assert!(PlayerListener::garage_car_deletion(&report(205), &mut deferred).is_none());
    assert!(deferred.is_empty());
}

#[test]
fn the_exit_teleport_targets_the_teleport_state_with_the_configured_destination() {
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::participants::{Endpoint, ExitBindings};
    let at = |selector| Endpoint {
        scene: 10,
        selector,
        serial: Serial::new(0).unwrap(),
    };
    let exit = ExitBindings {
        exit_request: at(96),
        garage_exit_request: at(8),
        state_94: at(93),
        state_4: at(3),
        state_3: at(2),
        state_77: at(76),
        state_83: at(82),
        state_78: at(77),
        ready_request: at(80),
        state_79: at(78),
        state_2: at(1),
        garage_calls: [at(0), at(3), at(4), at(5), at(6)],
    };
    let mut bits = [0u32; 16];
    for j in [0, 5, 10] {
        bits[j] = 1f32.to_bits();
    }
    bits[12] = 12.5f32.to_bits();
    let mut v = crate::vehicle_content::layout::tests::fixture();
    v["exit_teleport"] = serde_json::json!({"transform_bits": bits});
    let layout = crate::vehicle_content::layout::Layout::from_json(&v).unwrap();
    let command = PlayerListener::exit_teleport(Some(exit), Some(&layout), 178, 205).unwrap();
    assert_eq!(command.endpoint, exit.state_77);
    assert_eq!((command.participant, command.vehicle), (178, 205));
    assert_eq!(command.position, [12.5, 0., 0.]);
    assert!(command.encode().is_ok());
    // Without exit bindings or a configured destination there is no teleport.
    assert!(PlayerListener::exit_teleport(None, Some(&layout), 178, 205).is_none());
    let plain = crate::vehicle_content::layout::Layout::from_json(
        &crate::vehicle_content::layout::tests::fixture(),
    )
    .unwrap();
    assert!(PlayerListener::exit_teleport(Some(exit), Some(&plain), 178, 205).is_none());
    assert!(PlayerListener::exit_teleport(Some(exit), None, 178, 205).is_none());
}

#[test]
fn the_arrive_frame_carries_the_arrive_logic_deletions() {
    use super::sequences::arrival_section;
    assert_eq!(arrival_section(Vec::new(), Vec::new()), Ok(None));
    // Deletions alone (no arrive sequence configured) still get a section.
    let section = arrival_section(Vec::new(), vec![421, 422])
        .unwrap()
        .unwrap();
    assert_eq!(section.deleted, vec![421, 422]);
    assert!(section.records.is_empty() && section.setup.is_none() && !section.flag);
}

#[test]
fn movement_grant_waits_for_world_car_report_and_keeps_garage_authority() {
    use nfs_world::logic::{EntityRef, Message};
    let mut cars = BTreeMap::from([(300, (40, false)), (301, (41, false))]);
    let owned = BTreeSet::from([299, 300]);
    assert_eq!(
        PlayerListener::ready_movement(owned.clone(), &cars),
        BTreeSet::from([299])
    );
    let mut spawn = nfs_world::spawn_points::SpawnPoints::default();
    let mut listener = PlayerListener::new(101);
    listener
        .initialize_world(&content_with(Some(&SPAWN_LAYOUT)))
        .unwrap();
    // Readiness cannot grant a car when no SpawnPoints endpoint is configured.
    let report = Message::Reached {
        event: 28_404_286,
        target: EntityRef {
            ghost: 300,
            entity: 2,
        },
        player: 0,
    };
    assert_eq!(
        PlayerListener::world_car_ready(&report, &mut cars, &mut spawn),
        Ok(Some(None))
    );
    assert_eq!(
        PlayerListener::ready_movement(owned.clone(), &cars),
        BTreeSet::from([299])
    );
    // The existing readiness transition marks precisely the reported car.
    spawn = listener.spawn_points.clone();
    assert!(
        PlayerListener::world_car_ready(&report, &mut cars, &mut spawn)
            .unwrap()
            .is_some_and(|v| v.is_some())
    );
    assert_eq!(
        PlayerListener::ready_movement(owned, &cars),
        BTreeSet::from([299, 300])
    );
    assert!(!PlayerListener::ready_movement(BTreeSet::from([301]), &cars).contains(&301));
}

#[test]
fn ready_binding_has_a_complete_encodable_replication_section() {
    use nfs_world::replication::{
        Record, Update as Delta,
        player_manager::{Entry, Update},
    };
    let mut update = Update::default();
    update.entries[0] = Some(Some(Entry::driving(2, 300)));
    let mut record = Record {
        id: 16,
        initial: None,
        update: Delta::PlayerManager(update),
    };
    let sections = PlayerListener::binding_sections(vec![record.clone()]).unwrap();
    assert_eq!(sections.len(), 1);
    assert!(sections[0].setup.is_some());
    assert_eq!(sections[0].records, vec![record.clone()]);
    assert!(sections[0].frame().is_ok());
    // A bad record is rejected while readiness state is still transactional.
    record.id = 0;
    assert!(PlayerListener::binding_sections(vec![record]).is_err());
}

#[test]
fn generated_trigger_is_enabled_and_bound_without_changing_boolean_defaults() {
    use nfs_world::replication::{Initial, sublevel};
    let content = content_with(Some(&[Kind::Noop, Kind::RpcTriggerVolume, Kind::RpcBool]));
    let mut listener = PlayerListener::new(101);
    listener.initialize_world(&content).unwrap();
    let key = content.roles.as_ref().unwrap().spawn_points.unwrap();
    let scene = listener.pending.iter().flat_map(|s| &s.records).find(|r|
        matches!(&r.initial, Some(Initial::SubLevel { prefix, .. }) if prefix.content_key == key)).unwrap();
    let Some(Initial::SubLevel { fields, .. }) = &scene.initial else {
        panic!()
    };
    assert!(matches!(
        fields[1],
        sublevel::Initial::RpcBool { value: true, .. }
    ));
    assert!(matches!(
        fields[2],
        sublevel::Initial::RpcBool { value: false, .. }
    ));
    let before = listener.trigger_volumes.clone();
    assert_eq!(
        listener.trigger_volumes.bind(std::slice::from_ref(scene)),
        Err(replication::Error::DuplicateObject)
    );
    assert_eq!(listener.trigger_volumes, before);
}

fn binding_refresh_listener() -> (PlayerListener, u16) {
    let mut listener = PlayerListener::new(101);
    let player = listener
        .players
        .create(
            HOST_SELECTOR as u8,
            101,
            nfs_world::replication::players::Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = listener
        .players
        .join(HOST_SELECTOR as u8, 101, player)
        .unwrap()
        .unwrap()
        .id;
    listener.players.enable_player_manager().unwrap().unwrap();
    (listener, participant)
}

#[test]
fn recreated_customization_timer_accepts_new_lifetime_without_reusing_old_measurement() {
    use crate::{
        customization_timer as timer,
        garage_logic::{GarageLogic, Which},
    };
    use nfs_world::logic::{EntityRef, Message};
    let (mut listener, participant) = binding_refresh_listener();
    let scenes = Content::new(vec![(
        101,
        nfs_world::replication::sublevel::gameplay::profile().unwrap(),
    )])
    .unwrap();
    let creation = nfs_world::replication::sublevel::ordinary::creation(
        1,
        101,
        None,
        nfs_protocol::world::rpc::Serial::new(0).unwrap(),
        &nfs_world::replication::sublevel::ordinary::unpopulated(scenes.profile(101).unwrap())
            .unwrap(),
        &scenes,
    )
    .unwrap();
    listener
        .players
        .create_scenes(&[1], vec![creation], &scenes)
        .unwrap();
    let mut policy = crate::garage_logic::tests::content_json();
    let entity = policy["spawn"]["entities"][3].clone();
    let empty = serde_json::json!({"scene_updates":[],"entities":[],"events":[]});
    policy["spawn"] = empty.clone();
    policy["spawn"]["entities"] = serde_json::json!([entity.clone()]);
    policy["entry"] = empty.clone();
    policy["vehicles"] = empty.clone();
    policy["exit"] = empty.clone();
    policy["exit"]["delete_capture_ids"] = serde_json::json!([295]);
    policy["returning"] = empty.clone();
    policy["returning"]["entities"] = serde_json::json!([entity]);
    policy["returned"] = empty;
    let logic = GarageLogic::from_json(&policy).unwrap();
    let mut roles = content().roles.unwrap();
    roles.customization_timer = Asset {
        bundle: 4,
        type_id: 10,
        local_index: 3,
    };
    listener.roles = Some(roles);
    let mut ghosts = BTreeMap::new();
    logic
        .produce(
            Which::Spawn,
            &mut listener.players,
            participant,
            &mut ghosts,
        )
        .unwrap();
    let next_sequence = std::cell::Cell::new(0u8);
    let deliver = |listener: &mut PlayerListener, ghost, event| {
        let sequence = next_sequence.get();
        let body = Message::Reached {
            event,
            target: EntityRef { ghost, entity: 2 },
            player: 0,
        }
        .encode()
        .unwrap();
        let mut wire = nfs_world::bits::BitWriter::new();
        wire.put(1 << frame::MESSAGES, 6)
            .put(1, 4)
            .put(0, 1)
            .put(1, 1)
            .put(0, 3)
            .put(sequence.into(), 7)
            .put(82, 7)
            .put_span(body.span())
            .put(0, 1)
            .align();
        let accepted = listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        });
        if accepted {
            next_sequence.set((sequence + 1) & 127);
        }
        listener.pending.clear();
        listener.pending_rpcs.clear();
        accepted
    };
    for cycle in 0..3 {
        let current = ghosts[&295];
        listener.logic_ghosts.insert(participant, ghosts.clone());
        listener.world_ms = cycle * 10_000;
        assert!(
            deliver(&mut listener, current, timer::STOP),
            "cycle {cycle}: {:?}",
            listener.last_error
        );
        assert_eq!(
            listener.customization.state(participant).unwrap().completed,
            0
        );
        assert!(deliver(&mut listener, current, timer::START));
        listener.world_ms += 6_000;
        assert!(deliver(&mut listener, current, timer::STOP));
        assert_eq!(
            listener.customization.state(participant).unwrap().completed,
            1
        );
        logic
            .produce(Which::Exit, &mut listener.players, participant, &mut ghosts)
            .unwrap();
        logic
            .produce(
                Which::Returning,
                &mut listener.players,
                participant,
                &mut ghosts,
            )
            .unwrap();
        listener.logic_ghosts.insert(participant, ghosts.clone());
        assert_ne!(ghosts[&295], current);
        // The replaced timer's late stop names no owned timer: consumed and
        // logged, never refused (a refused reliable message is resent forever,
        // E1252). It measures nothing, and the committed frame drops the old
        // lifetime's measurement with its entity.
        assert!(deliver(&mut listener, current, timer::STOP));
        assert!(listener.customization.state(participant).is_none());
    }
    // A live timer the garage logic did not record (the intro's final garage
    // presentation creates it, E1252) is owned through its root participant
    // and measures like any other.
    listener.logic_ghosts.remove(&participant);
    let current = ghosts[&295];
    listener.world_ms = 40_000;
    assert!(deliver(&mut listener, current, timer::STOP));
    assert!(deliver(&mut listener, current, timer::START));
    listener.world_ms += 6_000;
    assert!(deliver(&mut listener, current, timer::STOP));
    assert_eq!(
        listener.customization.state(participant).unwrap().completed,
        1
    );
}

#[test]
fn completion_binding_refresh_waits_for_deadline_and_preserves_presentation_order_once() {
    let (mut listener, participant) = binding_refresh_listener();
    let expected = listener
        .players
        .vehicle_binding_snapshot(HOST_SELECTOR as u8, 101, participant)
        .unwrap();
    let before = listener.players.objects().snapshot();
    let presentation = Section {
        float_bits: None,
        flag: false,
        deleted: vec![321],
        setup: None,
        records: vec![],
    };
    listener
        .pending_entries
        .push_back((presentation.clone(), vec![]));
    listener
        .pending_binding_refreshes
        .push_back((100, participant));
    listener.advance_sequences(99).unwrap();
    assert_eq!(listener.pending_entries.len(), 1);
    assert_eq!(listener.pending_binding_refreshes.len(), 1);
    listener.advance_sequences(100).unwrap();
    assert_eq!(
        listener.pending_entries.pop_front().unwrap().0,
        presentation
    );
    let published = listener.pending_entries.pop_front().unwrap();
    assert_eq!(published.0.records, expected);
    assert!(published.1.is_empty());
    assert!(listener.pending_binding_refreshes.is_empty());
    listener.advance_sequences(200).unwrap();
    assert!(listener.pending_entries.is_empty());
    assert_eq!(listener.players.objects().snapshot(), before);
}

#[test]
fn completion_binding_refresh_retains_due_work_under_backpressure_and_is_session_local() {
    let (mut listener, participant) = binding_refresh_listener();
    let (mut other, _) = binding_refresh_listener();
    let occupied = Section {
        float_bits: None,
        flag: false,
        deleted: vec![],
        setup: None,
        records: vec![],
    };
    listener
        .pending_entries
        .resize(MAX_PENDING, (occupied, vec![]));
    listener
        .pending_binding_refreshes
        .push_back((100, participant));
    listener.advance_sequences(100).unwrap();
    assert_eq!(listener.pending_entries.len(), MAX_PENDING);
    assert_eq!(listener.pending_binding_refreshes.len(), 1);
    other.advance_sequences(100).unwrap();
    assert!(other.pending_entries.is_empty());
    assert!(other.pending_binding_refreshes.is_empty());
    listener.pending_entries.pop_front();
    listener.advance_sequences(101).unwrap();
    assert_eq!(listener.pending_entries.len(), MAX_PENDING);
    assert_eq!(listener.pending_entries.back().unwrap().0.records.len(), 2);
    assert!(listener.pending_binding_refreshes.is_empty());
    assert!(listener.advance_sequences(100).is_err());
    assert_eq!(listener.pending_entries.len(), MAX_PENDING);
}

#[test]
fn player_view_admission_rolls_back_when_full_and_retries_once_per_session() {
    let mut listener = PlayerListener::new(101);
    listener.initialize_world(&content()).unwrap();
    let wire = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(0),
                messages: vec![frame::Message::CreatePlayer {
                    name: b"Driver".to_vec(),
                    flag: false,
                    slot: 0,
                }],
            }],
        )
        .build()
        .unwrap();
    let deliver = |listener: &mut PlayerListener| {
        listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        })
    };
    let initial = listener.players.objects().snapshot();
    let filler = listener.pending.front().unwrap().clone();
    listener.pending.resize(MAX_PENDING, filler);
    assert!(!deliver(&mut listener));
    assert_eq!(listener.players.objects().snapshot(), initial);
    assert_eq!(listener.pending.len(), MAX_PENDING);
    assert!(listener.pending_rpcs.is_empty());
    listener.pending.clear();
    assert!(deliver(&mut listener));
    let records: Vec<_> = listener.pending.iter().flat_map(|s| &s.records).collect();
    let player_at = records
        .iter()
        .position(|r| matches!(r.initial, Some(replication::Initial::Player(_))))
        .unwrap();
    let view_at = records
        .iter()
        .position(|r| matches!(r.initial, Some(replication::Initial::PlayerView(_))))
        .unwrap();
    assert!(player_at < view_at);
    let view = records[view_at];
    let player = records[player_at].id;
    assert_eq!(
        listener
            .players
            .owned_player_view(HOST_SELECTOR as u8, 101, player),
        Some(view.id)
    );
    for section in &listener.pending {
        assert!(section.encode().unwrap().len() <= nfs_world::application::OUTBOUND_FRAME_BITS);
    }
    let accepted = listener.players.objects().snapshot();
    listener.pending.clear();
    listener.pending_rpcs.clear();
    assert!(deliver(&mut listener));
    assert_eq!(listener.players.objects().snapshot(), accepted);
    assert!(listener.pending.is_empty());
    let mut reconnected = PlayerListener::new(202);
    reconnected.initialize_world(&content()).unwrap();
    assert!(deliver(&mut reconnected));
    let new_player = reconnected
        .players
        .objects()
        .iter()
        .find_map(|(id, o)| matches!(o.initial, replication::Initial::Player(_)).then_some(id))
        .unwrap();
    assert!(
        reconnected
            .players
            .owned_player_view(HOST_SELECTOR as u8, 202, new_player)
            .is_some()
    );
    assert!(
        reconnected
            .players
            .owned_player_view(HOST_SELECTOR as u8, 101, new_player)
            .is_none()
    );
}

/// E838: the host retired a participant's world membership when it queued
/// the unsubscription, but the client reported a volume crossing before that
/// notification arrived. The frame was refused and its reliable report was
/// resent in every later frame, so every later frame was refused too.
#[test]
fn late_world_reports_from_a_retired_participant_do_not_refuse_the_frame() {
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::{
        bits::BitWriter,
        replication::sublevel::{gameplay, ordinary},
        world_activation::{Activation, Catalog, Site},
    };
    let profiles = Content::new(vec![(501, gameplay::profile().unwrap())]).unwrap();
    let creation = ordinary::creation(
        1,
        501,
        None,
        Serial::new(0).unwrap(),
        &ordinary::unpopulated(profiles.profile(501).unwrap()).unwrap(),
        &profiles,
    )
    .unwrap();
    let mut listener = PlayerListener::new(101);
    let records = listener
        .players
        .create_scenes(&[1], vec![creation], &profiles)
        .unwrap();
    listener.players.enable_player_manager().unwrap().unwrap();
    let site = |serializer| Site {
        scene_key: 501,
        serializer,
    };
    listener.world_activation = Some(
        Activation::bind(
            &records,
            &Catalog {
                volumes: vec![site(5)],
                presence: site(19),
                streamed: site(47),
                location: site(28),
            },
        )
        .unwrap(),
    );
    let player = listener
        .players
        .create(
            HOST_SELECTOR as u8,
            101,
            nfs_world::replication::players::Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = listener
        .players
        .join(HOST_SELECTOR as u8, 101, player)
        .unwrap()
        .unwrap()
        .id;
    let objects = listener.players.objects();
    let (scene, volume) = objects.scene_endpoint(501, 5).unwrap();
    let (_, location) = objects.scene_endpoint(501, 28).unwrap();
    let next_sequence = std::cell::Cell::new(0u8);
    let report = |participant: u16, selector: u16, method: u32, clock: Option<f64>| {
        let sequence = next_sequence.get();
        next_sequence.set((sequence + 1) & 127);
        let mut payload = BitWriter::new();
        payload.put(selector.into(), 9).put(method.into(), 32);
        if let Some(clock) = clock {
            payload.put(clock.to_bits(), 64).put(0, 32);
        }
        payload.put(0, 7);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(scene.into(), 13)
            .put(participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        let mut wire = BitWriter::new();
        wire.put(1 << frame::MESSAGES, 6)
            .put(1, 4)
            .put(0, 1)
            .put(1, 1)
            .put(0, 3)
            .put(sequence.into(), 7)
            .put(47, 7)
            .put_span(body.span())
            .put(0, 1)
            .align();
        wire
    };
    let deliver = |listener: &mut PlayerListener, wire: &BitWriter| {
        let accepted = listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        });
        listener.pending.clear();
        listener.pending_rpcs.clear();
        accepted
    };
    let players = listener.players.clone();
    let owns = |id| players.owns_participant(HOST_SELECTOR as u8, 101, id);
    let activation = listener.world_activation.as_mut().unwrap();
    assert_eq!(
        activation.set(participant, true, 16, owns).unwrap().len(),
        3
    );
    assert!(deliver(
        &mut listener,
        &report(participant, volume, 2, Some(1.))
    ));
    // The return retires membership; the unsubscription is still queued.
    let activation = listener.world_activation.as_mut().unwrap();
    assert_eq!(
        activation.set(participant, false, 16, owns).unwrap().len(),
        3
    );
    for wire in [
        report(participant, volume, 3, Some(2.)),
        report(participant, location, 0, None),
        report(participant, volume, 2, Some(3.)),
    ] {
        assert!(deliver(&mut listener, &wire), "{:?}", listener.last_error);
    }
    assert_eq!((listener.refused, listener.last_error), (0, None));
    // Ownership is still enforced: an unknown participant refuses the frame.
    assert!(!deliver(
        &mut listener,
        &report(participant + 1, volume, 3, Some(4.))
    ));
    assert_eq!(
        (listener.refused, listener.last_error),
        (1, Some(replication::Error::UnknownObject))
    );
}

#[test]
fn held_glass_replies_release_one_frame_after_the_spawn_report_within_capacity() {
    use nfs_world::logic::{EntityRef, Fire};
    use nfs_world::participants::HostRpc;
    let mut listener = PlayerListener::new(101);
    listener
        .initialize_world(&content_with(Some(&SPAWN_LAYOUT)))
        .unwrap();
    let fire = |event, ghost| Fire {
        event,
        player: Some(3),
        target: EntityRef { ghost, entity: 2 },
    };
    // Two participants settle at different times; replies keep their order.
    listener
        .held_glass
        .insert(40, vec![fire(11, 300), fire(9, 300), fire(7, 300)]);
    listener.held_glass.insert(41, vec![fire(11, 301)]);
    listener.pending_settled.push_back((1_100, 40));
    listener.pending_settled.push_back((1_250, 41));
    listener.advance_settled(1_099).unwrap();
    assert!(listener.pending_rpcs.is_empty());
    assert_eq!(listener.pending_settled.len(), 2);
    // A full queue keeps the due release for the next tick.
    let filler = HostRpc::Event(fire(5, 999));
    listener
        .pending_rpcs
        .resize(nfs_world::session::MAX_RPC_QUEUE - 2, filler);
    listener.advance_settled(1_100).unwrap();
    assert_eq!(listener.pending_settled.len(), 2);
    assert_eq!(listener.held_glass.len(), 2);
    listener.pending_rpcs.clear();
    listener.advance_settled(1_100).unwrap();
    assert_eq!(listener.pending_settled.len(), 1);
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .map(|r| match r {
                HostRpc::Event(f) => (f.event, f.target.ghost),
                _ => panic!("event expected"),
            })
            .collect::<Vec<_>>(),
        vec![(11, 300), (9, 300), (7, 300)]
    );
    assert!(!listener.held_glass.contains_key(&40));
    listener.advance_settled(1_250).unwrap();
    assert!(listener.pending_settled.is_empty());
    assert!(listener.held_glass.is_empty());
    assert_eq!(listener.pending_rpcs.len(), 4);
    // A settled participant whose replies were already released is a no-op.
    listener.pending_settled.push_back((1_300, 40));
    listener.advance_settled(1_300).unwrap();
    assert_eq!(listener.pending_rpcs.len(), 4);
}

#[test]
fn settled_logic_precedes_glass_atomically_for_three_new_lifetimes() {
    use crate::garage_logic::{GarageLogic, Which};
    use nfs_world::{
        logic::{EntityRef, Fire},
        participants::HostRpc,
    };
    let mut policy = crate::garage_logic::tests::presentation_json();
    let mut definition = policy["spawn"]["entities"][3].clone();
    definition["capture_id"] = serde_json::json!(350);
    policy["exit"]["entities"] = serde_json::json!([definition]);
    policy["world_settled"]["events"] = serde_json::json!([
        {"target":{"capture_id":350},"entity":1,"event":21},
        {"target":{"capture_id":350},"entity":2,"event":20}
    ]);
    policy["returning"] =
        serde_json::json!({"scene_updates":[],"entities":[],"delete_capture_ids":[350]});
    let logic = std::sync::Arc::new(GarageLogic::from_json(&policy).unwrap());
    let (mut players, _) = crate::garage_logic::tests::world();
    let player = players
        .create(
            HOST_SELECTOR as u8,
            100,
            Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap();
    let participant = players
        .join(HOST_SELECTOR as u8, 100, player.id)
        .unwrap()
        .unwrap()
        .id;
    let mut listener = PlayerListener::new(100);
    listener.players = players;
    listener.logic = Some(logic.clone());
    let mut ghosts = BTreeMap::new();
    for phase in [Which::Spawn, Which::Entry] {
        logic
            .produce(phase, &mut listener.players, participant, &mut ghosts)
            .unwrap();
    }
    let mut previous = None;
    for cycle in 0..3 {
        logic
            .produce(Which::Exit, &mut listener.players, participant, &mut ghosts)
            .unwrap();
        let target = ghosts[&350];
        assert_ne!(previous, Some(target));
        listener.logic_ghosts.insert(participant, ghosts.clone());
        let fire = Fire {
            event: 11,
            player: Some(0),
            target: EntityRef {
                ghost: 700,
                entity: 2,
            },
        };
        listener.held_glass.insert(participant, vec![fire]);
        let due = 100 + cycle * 1000;
        listener.pending_settled.push_back((due, participant));
        listener.advance_settled(due - 1).unwrap();
        assert!(listener.pending_rpcs.is_empty());
        listener
            .pending_rpcs
            .resize(nfs_world::session::MAX_RPC_QUEUE - 2, HostRpc::Event(fire));
        listener.advance_settled(due).unwrap();
        assert_eq!(listener.pending_settled.len(), 1);
        assert!(listener.held_glass.contains_key(&participant));
        listener.pending_rpcs.clear();
        listener.advance_settled(due).unwrap();
        let events = listener
            .pending_rpcs
            .iter()
            .map(|r| match r {
                HostRpc::Event(f) => (f.event, f.target.ghost),
                _ => panic!("expected event"),
            })
            .collect::<Vec<_>>();
        assert_eq!(events, vec![(21, target), (20, target), (11, 700)]);
        listener.advance_settled(due + 1).unwrap();
        assert_eq!(listener.pending_rpcs.len(), 3);
        listener.pending_rpcs.clear();
        logic
            .produce(
                Which::Returning,
                &mut listener.players,
                participant,
                &mut ghosts,
            )
            .unwrap();
        assert!(listener.players.objects().get(target).is_none());
        previous = Some(target);
    }
    // A foreign owner cannot consume the callback or its held replies.
    listener.pending_settled.push_back((4000, participant));
    listener.persona = 999;
    assert_eq!(
        listener.advance_settled(4000),
        Err(replication::Error::UnknownObject)
    );
    assert_eq!(listener.pending_settled.len(), 1);
    assert!(listener.pending_rpcs.is_empty());
}
#[test]
fn debug_hides_fire_on_the_main_root_in_installed_order_or_not_at_all() {
    use nfs_world::logic::{EntityRef, Fire};
    use nfs_world::participants::HostRpc;
    let fires = super::debug_hide_fires(361, &[(2, 9), (1, 4)]).unwrap();
    assert_eq!(
        fires,
        vec![
            HostRpc::Event(Fire {
                event: 9,
                player: None,
                target: EntityRef {
                    ghost: 361,
                    entity: 2
                },
            }),
            HostRpc::Event(Fire {
                event: 4,
                player: None,
                target: EntityRef {
                    ghost: 361,
                    entity: 1
                },
            }),
        ]
    );
    assert!(super::debug_hide_fires(361, &[]).unwrap().is_empty());
    // A root that cannot be addressed refuses the whole set.
    assert!(super::debug_hide_fires(0, &[(1, 4)]).is_err());
    assert!(super::debug_hide_fires(8192, &[(1, 4)]).is_err());
}
