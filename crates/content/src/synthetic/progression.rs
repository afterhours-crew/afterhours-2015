// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored progression graphs: a high-bit stable ID and an unrelated channel
//! with the same asset before the two explicitly selected spawner targets.
use super::{Asset, Field, ObjectFields, PartitionWriter, SceneObjects, connection};
use crate::progression::Rules;

pub const MAIN: &str = "synthetic/progression/main";
pub const INTRO: &str = "synthetic/progression/intro";
pub const SCENE: &str = "levels/runtime/progression";
fn guid(seed: u8) -> [u8; 16] {
    let mut value = [0xf2; 16];
    value[15] = seed;
    value
}
fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}
fn asset(seed: u8, name: &str, class: &str, fields: ObjectFields, objects: SceneObjects) -> Asset {
    Asset {
        name: name.into(),
        partition: PartitionWriter::default().build_many(
            guid(seed),
            class,
            guid(seed ^ 0x80),
            fields,
            objects,
        ),
    }
}
pub fn rules() -> Rules {
    let mut ports = crate::BuildIdentity::supported().progression.ports;
    for (i, value) in ports.values_mut().enumerate() {
        *value = 1000 + i as u32;
    }
    Rules {
        main: MAIN.into(),
        intro: INTRO.into(),
        scene: SCENE.into(),
        ports,
        classes: [
            ("ParticipantLogicBlueprint", None),
            ("ParticipantLogicEntityData", Some("root")),
            ("ObjectivePersistenceEntityData", None),
            ("ValuesPersistenceEntityData", None),
            ("RaceObjectiveEntityData", Some("objective")),
            ("StableUniqueIdEntityData", None),
            ("BoolEntityData", None),
            ("LinkedSchematicChannelReferenceEntityData", Some("channel")),
        ]
        .into_iter()
        .map(|(a, b)| (a.into(), b.map(str::to_owned)))
        .collect(),
    }
}
pub fn assets() -> Vec<Asset> {
    assets_with(|_, _, _| {})
}
pub fn assets_with(mut edit: impl FnMut(&str, &mut ObjectFields, &mut SceneObjects)) -> Vec<Asset> {
    let p = rules().ports;
    let mut out = Vec::new();
    for (seed, name, id) in [(1, MAIN, -13i64), (2, INTRO, 77)] {
        let mut fields = vec![
            ("Name", Field::CString(name.into())),
            ("Flags", Field::UInt(1)),
            ("Objects", Field::RawPointers((2..=8).collect())),
            (
                "LinkConnections",
                Field::Structs(
                    "LinkConnection",
                    vec![
                        connection(3, p["objectives"], 5, 0),
                        connection(5, p["stable_id"], 6, 0),
                        connection(2, p["root_channels"], 8, 0),
                    ],
                ),
            ),
            (
                "PropertyConnections",
                Field::Structs(
                    "PropertyConnection",
                    vec![connection(7, p["bool_value"], 5, p["active"])],
                ),
            ),
        ];
        let mut objects = vec![
            ("ParticipantLogicEntityData", vec![]),
            ("ObjectivePersistenceEntityData", vec![]),
            ("ValuesPersistenceEntityData", vec![]),
            ("RaceObjectiveEntityData", vec![]),
            (
                "StableUniqueIdEntityData",
                vec![("StableUniqueId64", Field::Long(id))],
            ),
            (
                "BoolEntityData",
                vec![("DefaultValue", Field::Bool(seed == 2))],
            ),
            (
                "LinkedSchematicChannelReferenceEntityData",
                vec![("ChannelAsset", import(3))],
            ),
        ];
        edit(name, &mut fields, &mut objects);
        out.push(asset(
            seed,
            name,
            "ParticipantLogicBlueprint",
            fields,
            objects,
        ));
    }
    out.push(asset(
        3,
        "synthetic/channel",
        "ChannelAsset",
        vec![],
        vec![],
    ));
    let mut fields = vec![
        ("Name", Field::CString(SCENE.into())),
        ("Objects", Field::RawPointers((2..=6).collect())),
        (
            "LinkConnections",
            Field::Structs(
                "LinkConnection",
                vec![
                    connection(2, p["spawner_channels"], 5, 0),
                    connection(3, p["spawner_channels"], 6, 0),
                ],
            ),
        ),
        (
            "PropertyConnections",
            Field::Structs("PropertyConnection", vec![]),
        ),
    ];
    let mut objects = vec![
        (
            "ParticipantLogicSpawnerEntityData",
            vec![("ParticipantLogicBlueprint", import(1))],
        ),
        (
            "ParticipantLogicSpawnerEntityData",
            vec![("ParticipantLogicBlueprint", import(2))],
        ),
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(3))],
        ),
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(3))],
        ),
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(3))],
        ),
    ];
    edit(SCENE, &mut fields, &mut objects);
    out.push(asset(4, SCENE, "SubWorldData", fields, objects));
    out.push(asset(
        5,
        &format!("{SCENE}_networkregistry_win32"),
        "NetworkRegistryAsset",
        vec![(
            "Objects",
            Field::Imports(vec![(guid(2), guid(0x82)), (guid(1), guid(0x81))]),
        )],
        vec![],
    ));
    out
}
