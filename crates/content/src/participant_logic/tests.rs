// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, connection};
use nfs_frostbite::synthetic::{Asset as Fixture, Field as F, PartitionWriter};
use serde_json::json;

const LEVEL: &str = "levels/test/root";
const SCENE: &str = "levels/test/scene";
const CHILD: &str = "test/child";
const PARENT: &str = "test/parent";
fn reference(seed: u8) -> ([u8; 16], [u8; 16]) {
    ([seed; 16], [seed + 100; 16])
}
fn import(seed: u8) -> F {
    let (a, b) = reference(seed);
    F::Import(a, b)
}
fn fixture(
    seed: u8,
    name: &str,
    class: &str,
    fields: Vec<(&'static str, F)>,
    objects: Vec<(&str, Vec<(&'static str, F)>)>,
) -> Fixture {
    let (a, b) = reference(seed);
    Fixture {
        name: name.into(),
        partition: PartitionWriter::default().build_many(a, class, b, fields, objects),
    }
}
struct Installation(std::path::PathBuf);
impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn make_index(ambiguous: bool) -> (Installation, AssetIndex) {
    let port = crate::BuildIdentity::supported().participant_logic;
    let mut scene_objects = vec![
        (
            "ParticipantLogicSpawnerSetControlledEntityData",
            vec![("ParticipantLogicBlueprint", import(3))],
        ),
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(4))],
        ),
        // Same-asset decoy before the bound channel must not win a global lookup.
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(4))],
        ),
    ];
    if ambiguous {
        scene_objects.push((
            "ParticipantLogicSpawnerEntityData",
            vec![("ParticipantLogicBlueprint", import(3))],
        ));
    }
    let assets = vec![
        fixture(
            1,
            LEVEL,
            "LevelData",
            vec![("Objects", F::RawPointers(vec![2]))],
            vec![(
                "SubWorldReferenceObjectData",
                vec![
                    ("BundleName", F::CString(SCENE.into())),
                    ("AutoLoad", F::Bool(true)),
                    ("IsWin32SubLevel", F::Bool(true)),
                ],
            )],
        ),
        fixture(
            2,
            SCENE,
            "SubWorldData",
            vec![
                (
                    "Objects",
                    F::RawPointers((2..scene_objects.len() as u32 + 2).collect()),
                ),
                (
                    "LinkConnections",
                    F::Structs(
                        "LinkConnection",
                        vec![connection(2, port.spawner_channels, 4, 0)],
                    ),
                ),
            ],
            scene_objects,
        ),
        fixture(
            3,
            CHILD,
            "ParticipantLogicBlueprint",
            vec![
                ("Flags", F::UInt(1)),
                ("Objects", F::RawPointers(vec![2, 3])),
                (
                    "LinkConnections",
                    F::Structs(
                        "LinkConnection",
                        vec![connection(2, port.root_channels, 3, 0)],
                    ),
                ),
            ],
            vec![
                ("ParticipantLogicEntityData", vec![]),
                (
                    "LinkedSchematicChannelReferenceEntityData",
                    vec![("ChannelAsset", import(4))],
                ),
            ],
        ),
        fixture(4, "test/channel", "SchematicChannelAsset", vec![], vec![]),
        fixture(
            5,
            &format!("{SCENE}_networkregistry_win32"),
            "NetworkRegistryAsset",
            vec![("Objects", F::Imports(vec![reference(6), reference(3)]))],
            vec![],
        ),
        fixture(
            6,
            PARENT,
            "ParticipantLogicBlueprint",
            vec![
                ("Flags", F::UInt(1)),
                ("Objects", F::RawPointers(vec![2, 3])),
            ],
            vec![
                ("ParticipantLogicEntityData", vec![]),
                (
                    "LinkedSchematicChannelEntityData",
                    vec![("ChannelAsset", import(4))],
                ),
            ],
        ),
    ];
    let temp = Installation(std::env::temp_dir().join(format!(
        "nfs-participant-layout-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    )));
    let _ = std::fs::remove_dir_all(&temp.0);
    let (mut options, _) = synthetic::write(&temp.0, &assets).unwrap();
    options.identity.scenes.level = LEVEL.into();
    options.identity.scenes.classes.insert(
        "ParticipantLogicSpawnerSetControlledEntityData".into(),
        None,
    );
    options
        .identity
        .blueprint_class_ids
        .insert("ParticipantLogicBlueprint".into(), 30);
    let index = AssetIndex::open(&temp.0, &options).unwrap();
    (temp, index)
}

#[test]
fn installed_registry_order_and_explicit_scene_links_determine_layout() {
    let (_temp, mut index) = make_index(false);
    let layout = index
        .participant_layout(SCENE, CHILD, Owner::Scene)
        .unwrap();
    assert_eq!(
        layout.asset,
        Asset {
            bundle: 1,
            type_id: 30,
            local_index: 1
        }
    );
    assert_eq!(layout.catalog, [(30, 2)]);
    assert_eq!(layout.bus_count, 1);
    assert_eq!(
        layout.fields[1].channel,
        Some(Channel::Scene {
            content_key: crate::assets::scene_key(SCENE),
            serializer: 2
        })
    );
    assert_eq!(
        layout.profile().unwrap().kinds(),
        [Kind::Root, Kind::RpcReference]
    );
    let child = index
        .participant_layout(
            SCENE,
            CHILD,
            Owner::Participant {
                blueprint: PARENT.into(),
            },
        )
        .unwrap();
    assert_eq!(
        child.fields[1].channel,
        Some(Channel::Participant { serializer: 1 })
    );
    assert!(
        index
            .participant_layout(
                SCENE,
                CHILD,
                Owner::Participant {
                    blueprint: CHILD.into()
                }
            )
            .is_err()
    );
    assert!(
        index
            .participant_layout(SCENE, "missing", Owner::Scene)
            .is_err()
    );
}

#[test]
fn ambiguous_spawner_and_unsupported_composition_fail() {
    let (_temp, mut index) = make_index(true);
    assert!(
        index
            .participant_layout(SCENE, CHILD, Owner::Scene)
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    let (_temp, mut index) = make_index(false);
    index
        .identity
        .participant_logic
        .classes
        .remove("LinkedSchematicChannelReferenceEntityData");
    assert!(
        index
            .participant_layout(SCENE, CHILD, Owner::Scene)
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
}

#[test]
fn rules_reject_unknown_kinds_duplicate_spawners_and_unbounded_identifiers() {
    let rules = crate::BuildIdentity::supported().participant_logic;
    assert_eq!(Rules::from_json(&rules.to_json()).unwrap(), rules);
    let mut v = rules.to_json();
    v["root_channels"] = json!(0);
    assert!(Rules::from_json(&v).is_err());
    let mut v = rules.to_json();
    v["classes"]["Unknown"] = json!("guess");
    assert!(Rules::from_json(&v).is_err());
    let mut v = rules.to_json();
    v["spawners"] = json!(["Spawner", "Spawner"]);
    assert!(Rules::from_json(&v).is_err());
    let mut v = rules.to_json();
    v["extra"] = json!(1);
    assert!(Rules::from_json(&v).is_err());
}

#[test]
fn synced_float_keeps_its_position_between_other_serializers() {
    use crate::graph::tests::{Memory, local};
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            (
                "Objects",
                Value::Array(vec![local(1), local(2), local(3), local(4)]),
            ),
        ],
    );
    source.put("a", 1, "ParticipantLogicEntityData", &[]);
    source.put("a", 2, "SyncedIntEntityData", &[]);
    source.put("a", 3, "SyncedFloatEntityData", &[]);
    source.put("a", 4, "SyncedBoolEntityData", &[]);
    let rules = crate::BuildIdentity::supported().participant_logic;
    let nodes = Graph::new(&mut source).read(Site::root("a")).unwrap();
    let (_, fields) = shape(&nodes, &rules).unwrap();
    assert_eq!(
        fields.iter().map(|(_, kind)| *kind).collect::<Vec<_>>(),
        vec![
            Kind::Root,
            Kind::I32Property,
            Kind::FloatProperty,
            Kind::BoolProperty
        ]
    );
    assert_eq!(Rules::from_json(&rules.to_json()).unwrap(), rules);
}

#[test]
fn shape_counts_repeated_prefab_occurrences_and_rejects_duplicate_roots() {
    use crate::graph::tests::{Memory, local, ptr};
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
        ],
    );
    source.put("a", 1, "ParticipantLogicEntityData", &[]);
    for i in [2, 3] {
        source.put(
            "a",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("b", 0))],
        );
    }
    source.put(
        "b",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array(vec![local(1)])),
        ],
    );
    source.put("b", 1, "SyncedBoolEntityData", &[]);
    let rules = crate::BuildIdentity::supported().participant_logic;
    let mut nodes = Graph::new(&mut source).read(Site::root("a")).unwrap();
    let (buses, fields) = shape(&nodes, &rules).unwrap();
    assert_eq!(buses, 3);
    assert_eq!(fields.len(), 3);
    assert_ne!(fields[1].0, fields[2].0);
    nodes.push(nodes[1].clone());
    assert!(shape(&nodes, &rules).is_err());
}

#[test]
fn channel_binding_rejects_missing_duplicate_and_mistyped_endpoints() {
    use crate::graph::tests::{Memory, local, ptr};
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array(vec![local(1), local(2)])),
        ],
    );
    source.put("a", 1, "ParticipantLogicEntityData", &[]);
    source.put(
        "a",
        2,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    source.put(
        "b",
        0,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    source.put("c", 0, "SchematicChannelAsset", &[]);
    let mut graph = Graph::new(&mut source);
    let nodes = graph.read(Site::root("a")).unwrap();
    let reference = Endpoint {
        site: nodes[2].site.clone(),
        port: 0,
    };
    let target = Endpoint {
        site: Site::root("b"),
        port: 0,
    };
    assert_eq!(
        bind(
            &mut graph,
            &nodes,
            vec![reference.clone()],
            vec![target.clone()]
        )
        .unwrap()
        .len(),
        1
    );
    assert!(bind(&mut graph, &nodes, vec![reference.clone()], vec![]).is_err());
    assert!(
        bind(
            &mut graph,
            &nodes,
            vec![reference.clone(), reference.clone()],
            vec![target.clone(), target.clone()]
        )
        .is_err()
    );
    let mut wrong = target.clone();
    wrong.port = 9;
    assert!(bind(&mut graph, &nodes, vec![reference.clone()], vec![wrong]).is_err());
    assert!(bind(&mut graph, &nodes, vec![reference.clone()], vec![reference]).is_err());
}
