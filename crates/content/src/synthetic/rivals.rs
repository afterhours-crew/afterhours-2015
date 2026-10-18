// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A made-up roaming manager sharing the traffic fixture's AI managers.
use super::{Asset, Field, ObjectFields, PartitionWriter, SceneObjects, vector};
fn guid(seed: u8) -> [u8; 16] {
    let mut g = [0xf6; 16];
    g[15] = seed;
    g
}
fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}

pub(super) fn append(fields: &mut ObjectFields, objects: &mut SceneObjects) {
    let ports = crate::BuildIdentity::supported().rival_links;
    let start = objects.len() as u32 + 2;
    let raw = |i| Field::RawPointer(start + i);
    let link = |source, port, target| {
        vec![
            ("Source", source),
            ("SourceFieldId", Field::UInt(port)),
            ("Target", target),
            ("TargetFieldId", Field::UInt(0)),
        ]
    };
    for (name, value) in fields {
        if *name == "Objects" {
            let Field::RawPointers(values) = value else {
                panic!()
            };
            values.extend(start..start + 9);
        } else if *name == "LinkConnections" {
            let Field::Structs(_, values) = value else {
                panic!()
            };
            values.extend([
                link(raw(0), ports.racers, import(2)),
                link(raw(0), ports.participants, Field::RawPointer(4)),
                link(raw(0), ports.actors, Field::RawPointer(2)),
                link(import(2), ports.provider, raw(1)),
                link(import(2), ports.regions, raw(4)),
                link(import(2), ports.spawn, raw(5)),
                link(raw(1), ports.provider, raw(2)),
                link(raw(2), ports.personas, raw(3)),
                link(Field::RawPointer(4), ports.personality_library, raw(6)),
                link(raw(6), ports.personalities, raw(7)),
                link(raw(6), ports.personalities, raw(8)),
                link(raw(3), ports.personality, raw(8)),
            ]);
        }
    }
    objects.extend([
        ("RoamingRacerManagerEntityData", vec![]),
        (
            "SpecificVehicleProviderEntityData",
            vec![("VehicleTags", Field::RawPointers(vec![]))],
        ),
        ("PersonaSetEntityData", vec![]),
        (
            "PersonaEntityData",
            vec![
                ("PersonaName", Field::CString("SyntheticRival".into())),
                ("PersonaNameId", Field::CString(String::new())),
                (
                    "VehicleItemData",
                    Field::Import(super::reference(3).0, super::reference(3).1),
                ),
                ("VehicleTags", Field::RawPointers(vec![])),
                ("VehicleStyle", Field::RawPointer(0)),
                ("AdditionalItems", Field::RawPointers(vec![])),
                ("OverrideCarColor", Field::Bool(false)),
                ("OverridePerformance", Field::Bool(true)),
                ("Performance", Field::Float(0.2)),
                ("OverrideGripValue", Field::Bool(true)),
                ("GripValue", Field::UInt(4)),
            ],
        ),
        (
            "VolumeVectorShapeData",
            vec![
                ("Height", Field::Float(20.)),
                (
                    "Points",
                    Field::Structs(
                        "Vec3",
                        [(-20., -20.), (20., -20.), (20., 20.), (-20., 20.)]
                            .into_iter()
                            .map(|(x, z)| {
                                vec![
                                    ("x", Field::Float(x)),
                                    ("y", Field::Float(0.)),
                                    ("z", Field::Float(z)),
                                ]
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        (
            "RacerSpawnPointEntityData",
            vec![(
                "Transform",
                Field::Struct(
                    "LinearTransform",
                    vec![
                        ("right", vector(1., 0., 0.)),
                        ("up", vector(0., 1., 0.)),
                        ("forward", vector(0., 0., 1.)),
                        ("trans", vector(8., 3., -9.)),
                    ],
                ),
            )],
        ),
    ]);
    objects.extend([
        ("PersonalityLibraryEntityData", vec![]),
        (
            "PersonalityEntityData",
            vec![("PersonalityName", Field::CString("First".into()))],
        ),
        (
            "PersonalityEntityData",
            vec![("PersonalityName", Field::CString("Selected".into()))],
        ),
    ]);
}
pub(super) fn install(assets: &mut Vec<Asset>) {
    assets.push(Asset {
        name: "synthetic/rivals/2".into(),
        partition: PartitionWriter::default().build(
            guid(2),
            "RoamingRacerEntityData",
            guid(0x82),
            vec![("EventType", Field::UInt(0))],
        ),
    });
}
