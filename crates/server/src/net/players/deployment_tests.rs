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
    let filler = HostRpc::Participant(notification);
    listener
        .pending_rpcs
        .resize(nfs_world::session::MAX_RPC_QUEUE - 2, filler);
    let before = listener.level_poll.clone();
    listener.advance_level_poll(100).unwrap();
    assert_eq!(listener.level_poll, before);
    assert_eq!(listener.pending_spawns.len(), 1);
    assert_eq!(
        listener.pending_rpcs.len(),
        nfs_world::session::MAX_RPC_QUEUE - 2
    );
    listener.pending_rpcs.clear();
    listener.advance_level_poll(100).unwrap();
    assert!(matches!(listener.pending_rpcs[0], HostRpc::LevelPoll(_)));
    assert_eq!(listener.pending_rpcs[1], filler);
    assert!(matches!(listener.pending_rpcs[2],HostRpc::Event(e) if e.event==14));
    assert!(listener.pending_spawns.is_empty());
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
    let deliver = |listener: &mut PlayerListener, ghost, event| {
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
            .put(0, 7)
            .put(82, 7)
            .put_span(body.span())
            .put(0, 1)
            .align();
        let accepted = listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        });
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
        assert!(!deliver(&mut listener, current, timer::STOP));
    }
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
