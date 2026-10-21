// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    assets::AssetIndex,
    synthetic::{self, Asset, Field, PartitionWriter},
};
struct Installation(std::path::PathBuf);
impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn index(extra: Vec<Asset>) -> (Installation, AssetIndex) {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = Installation(std::env::temp_dir().join(format!(
        "nfs-sequence-content-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    let mut assets = synthetic::assets();
    assets.retain(|a| !extra.iter().any(|b| a.name == b.name));
    assets.extend(extra);
    let (options, _) = synthetic::write(&dir.0, &assets).unwrap();
    let index = AssetIndex::open(&dir.0, &options).unwrap();
    (dir, index)
}
#[test]
fn generated_sequences_follow_registry_manager_and_explicit_channel_links() {
    let (_dir, mut index) = index(vec![]);
    let bytes = build(&mut index).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let content = SequenceContent::from_json(&value).unwrap();
    assert_eq!(value["version"], 2);
    assert_eq!(content.definition.asset.type_id, 40);
    assert_eq!(content.definition.asset.local_index, 0);
    assert_eq!(content.definition.embedded_bus, 2);
    assert_eq!(content.definition.manager.index, 8);
    assert_eq!(content.definition.channel.index, 7); // Same-asset decoy is serializer6.
    assert_eq!(content.arrive.as_ref().unwrap().0.channel.index, 8); // Garage decoy7.
    let spline = content.spline.unwrap();
    assert_eq!(spline.asset.type_id, 71);
    assert_eq!(spline.asset.local_index, 1); // Selected via the command/channel link.
    assert!(spline.flag);
    assert!(value["arrive"]["spline"].get("base_value").is_none());
    let returning = content.returning.unwrap();
    assert_eq!(returning.duration_ms, 7500);
    assert_eq!(
        returning.definition.layout,
        nfs_world::sequences::Layout::GarageEntry
    );
    assert_ne!(returning.definition.asset, content.arrive.unwrap().0.asset);
    assert_ne!(returning.definition.channel.index, 8);
    let crash = content.crash.unwrap();
    assert_eq!(crash.duration_ms, 4_500);
    assert_eq!(crash.definition.manager, content.definition.manager);
    assert_eq!(
        crash.definition.embedded_bus,
        content.definition.embedded_bus
    );
    assert!(value["crash"].get("channel").is_none());
    assert_eq!(
        crash.content.profile(crash.definition.asset).unwrap(),
        &nfs_world::crash::profile()
    );
}

#[test]
fn return_timer_must_be_server_world_time_and_have_both_event_edges() {
    for sequence in [synthetic::sequences::ENTRY, synthetic::sequences::CRASH] {
        for case in 0..5 {
            let extra = synthetic::sequences::assets_with(|name, fields, objects| {
                if name != sequence {
                    return;
                }
                let timer = &mut objects
                    .iter_mut()
                    .find(|(k, _)| *k == "DelayEntityData")
                    .unwrap()
                    .1;
                match case {
                    0 => {
                        timer.iter_mut().find(|(k, _)| *k == "Delay").unwrap().1 =
                            Field::Float(f32::NAN)
                    }
                    1 => {
                        timer.iter_mut().find(|(k, _)| *k == "Delay").unwrap().1 = Field::Float(31.)
                    }
                    2 => {
                        timer.iter_mut().find(|(k, _)| *k == "Realm").unwrap().1 =
                            Field::Enum("Realm", "Realm_Client", 2)
                    }
                    3 => {
                        timer.iter_mut().find(|(k, _)| *k == "AutoStart").unwrap().1 =
                            Field::Bool(true)
                    }
                    _ => {
                        let Field::Structs(_, rows) = &mut fields
                            .iter_mut()
                            .find(|(k, _)| *k == "EventConnections")
                            .unwrap()
                            .1
                        else {
                            panic!()
                        };
                        rows.pop();
                    }
                }
            });
            let (_dir, mut index) = index(extra);
            assert!(build(&mut index).is_err(), "case {case}");
        }
    }
}
#[test]
fn unsupported_composition_missing_links_and_wrong_defaults_fail_closed() {
    for case in 0..6 {
        let extra = synthetic::sequences::assets_with(|name, fields, objects| {
            if name != synthetic::sequences::EXIT {
                return;
            }
            match case {
                0 => objects[2].0 = "UnsupportedSequenceEntityData",
                1 => {
                    let Field::RawPointers(p) =
                        &mut fields.iter_mut().find(|(k, _)| *k == "Objects").unwrap().1
                    else {
                        panic!()
                    };
                    p.swap(2, 4);
                }
                2 => {
                    let Field::Structs(_, links) = &mut fields
                        .iter_mut()
                        .find(|(k, _)| *k == "LinkConnections")
                        .unwrap()
                        .1
                    else {
                        panic!()
                    };
                    links.pop();
                }
                3 => objects[5].1[0].1 = Field::UInt(1),
                4 => objects[4].1[0].1 = Field::RawPointer(0),
                5 => objects[5].1.push(("Blueprint", Field::RawPointer(1))),
                _ => unreachable!(),
            }
        });
        let (_dir, mut index) = index(extra);
        assert!(build(&mut index).is_err(), "case {case}");
    }
}
#[test]
fn scene_binding_requires_one_spawner_and_one_matching_target() {
    for case in 0..3 {
        let extra = synthetic::sequences::scene_assets_with(|name, fields, objects| {
            if !name.ends_with("/garage") {
                return;
            }
            match case {
                0 => {
                    let spawner = objects
                        .iter()
                        .find(|o| o.0 == "StateSequenceRefererEntityData")
                        .unwrap()
                        .clone();
                    objects.push(spawner);
                    let pointer = objects.len() as u32 + 1;
                    let Field::RawPointers(p) =
                        &mut fields.iter_mut().find(|(k, _)| *k == "Objects").unwrap().1
                    else {
                        panic!()
                    };
                    p.push(pointer);
                }
                1 => {
                    let Field::Structs(_, links) = &mut fields
                        .iter_mut()
                        .find(|(k, _)| *k == "LinkConnections")
                        .unwrap()
                        .1
                    else {
                        panic!()
                    };
                    let row = links[links.len() - 2].clone();
                    links.push(row);
                }
                2 => {
                    objects
                        .iter_mut()
                        .rev()
                        .find(|o| o.0 == "LinkedSchematicChannelEntityData")
                        .unwrap()
                        .1[0]
                        .1 = Field::RawPointer(0)
                }
                _ => unreachable!(),
            }
        });
        let (_dir, mut index) = index(extra);
        assert!(build(&mut index).is_err(), "case {case}");
    }
}
#[test]
fn non_blueprint_registry_keeps_embedded_object_identity_and_rejects_duplicates() {
    let registry = "levels/runtime/garage_networkregistry_win32";
    for duplicate in [false, true] {
        let asset = Asset {
            name: registry.into(),
            partition: PartitionWriter::default().build_many(
                [0xe5; 16],
                "NetworkRegistryAsset",
                [0xe6; 16],
                vec![(
                    "Objects",
                    Field::RawPointers(if duplicate {
                        vec![3, 2, 3]
                    } else {
                        vec![3, 2, 4]
                    }),
                )],
                vec![
                    ("ScriptedAISplineEntityData", vec![]),
                    ("ScriptedAISplineEntityData", vec![]),
                    ("UnrelatedAsset", vec![]),
                ],
            ),
        };
        let (_dir, mut index) = index(vec![asset]);
        let objects =
            index.registered_objects("levels/runtime/garage", "ScriptedAISplineEntityData");
        if duplicate {
            assert!(objects.is_err());
        } else {
            assert_eq!(
                objects.unwrap(),
                [(registry.into(), 2), (registry.into(), 1)]
            );
        }
    }
}
#[test]
fn build_rules_reject_unknown_roles_paths_and_runtime_fields() {
    let value = synthetic::sequences::rules().to_json();
    assert_eq!(
        Rules::from_json(&value).unwrap(),
        synthetic::sequences::rules()
    );
    for (key, bad) in [
        ("waiting", serde_json::json!("../bad")),
        ("spline_class_id", serde_json::json!(0)),
        ("classes", serde_json::json!({"EntityData":"captured"})),
        ("exit", value["waiting"].clone()),
    ] {
        let mut invalid = value.clone();
        invalid[key] = bad;
        assert!(Rules::from_json(&invalid).is_err());
    }
    let mut invalid = value;
    invalid["embedded_bus"] = serde_json::json!(2);
    assert!(Rules::from_json(&invalid).is_err());
}

#[test]
fn nested_presentation_channel_requires_the_matching_scene_endpoint() {
    let (_dir, mut valid) = index(vec![]);
    let content =
        SequenceContent::from_json(&serde_json::from_slice(&build(&mut valid).unwrap()).unwrap())
            .unwrap();
    assert!(content.returning.unwrap().definition.presentation.is_some());
    let extra = synthetic::sequences::assets_with(|name, fields, _| {
        if name == "synthetic/sequences/client_presentation" {
            fields.iter_mut().find(|(k, _)| *k == "Objects").unwrap().1 =
                Field::RawPointers(vec![]);
        }
    });
    let (_dir, mut unknown) = index(extra);
    assert!(build(&mut unknown).is_err());
}
