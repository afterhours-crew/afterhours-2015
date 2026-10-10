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
    assert_eq!(listener.players.objects().len(), 15);
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
