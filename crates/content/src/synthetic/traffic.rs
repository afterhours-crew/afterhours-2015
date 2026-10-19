// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored traffic topology, with an empty spline before a usable one.
use super::{Asset, Field, ObjectFields, PartitionWriter, SceneObjects};
pub(super) fn links() -> crate::traffic::Links {
    crate::traffic::Links {
        roads: 111,
        provider: 222,
        personas: 333,
    }
}
const BUNDLE: &str = "vehicles/runtime/0_bundle";
const BLUEPRINT: &str = "synthetic/traffic/vehicle";
fn guid(seed: u8) -> [u8; 16] {
    let mut g = [0xf3; 16];
    g[15] = seed;
    g
}
fn reference(seed: u8) -> ([u8; 16], [u8; 16]) {
    (guid(seed), guid(seed ^ 0x80))
}
fn import(seed: u8) -> Field {
    let (a, b) = reference(seed);
    Field::Import(a, b)
}
fn asset(
    seed: u8,
    name: &str,
    class: &str,
    mut fields: ObjectFields,
    mut objects: SceneObjects,
) -> Asset {
    if seed == 1 {
        super::rivals::append(&mut fields, &mut objects);
        append_lifecycle(&mut fields, &mut objects);
    }
    let (partition, instance) = reference(seed);
    Asset {
        name: name.into(),
        partition: PartitionWriter::default()
            .build_many(partition, class, instance, fields, objects),
    }
}

fn append_lifecycle(fields: &mut ObjectFields, objects: &mut SceneObjects) {
    let ports = crate::BuildIdentity::supported().ai_links;
    let first = objects.len() as u32 + 2;
    for (name, value) in fields {
        if *name == "Objects" {
            let Field::RawPointers(values) = value else {
                panic!()
            };
            values.extend(first..first + 9);
        }
        if *name == "LinkConnections" {
            let Field::Structs(_, values) = value else {
                panic!()
            };
            values.push(super::connection(4, ports.initial, first, 0));
            for i in 0..3 {
                values.push(super::connection(first + 4 + i, ports.source, first + i, 0));
                values.push(super::connection(
                    first + 4 + i,
                    ports.destination,
                    first + i + 1,
                    0,
                ));
            }
            values.push(super::connection(
                first + 7,
                ports.reset_volumes,
                first + 8,
                0,
            ));
        }
    }
    for (i, class) in [
        "UpdateableParticipantStateEntityData",
        "GetAiSpawnParametersParticipantStateEntityData",
        "SpawningParticipantStateEntityData",
        "ParticipantStateEntityData",
    ]
    .into_iter()
    .enumerate()
    {
        objects.push((class, vec![("SendRpcToClientSide", Field::Bool(i == 3))]));
    }
    for _ in 0..3 {
        objects.push(("ParticipantTransitionEntityData", vec![]));
    }
    objects.push(("AiResetTrackerEntityData", vec![]));
    objects.push((
        "StreamedTriggerVolumeCollectionEntityData",
        vec![
            ("NotifyServer", Field::Bool(true)),
            ("ClientControlled", Field::Bool(false)),
        ],
    ));
}
pub(super) fn registered() -> Vec<([u8; 16], [u8; 16])> {
    vec![reference(3), reference(4)]
}
pub(super) fn append(fields: &mut ObjectFields, objects: &mut SceneObjects) {
    let start = objects.len() as u32 + 2;
    for (name, value) in fields {
        if *name == "Objects" {
            let Field::RawPointers(values) = value else {
                panic!()
            };
            values.push(start);
        }
    }
    objects.push((
        "LogicPrefabReferenceObjectData",
        vec![("Blueprint", import(1))],
    ));
}
pub(super) fn install(assets: &mut Vec<Asset>) {
    assets.retain(|a| a.name != BUNDLE);
    let mut blueprint = None;
    super::vehicles::assets_with(|fields, objects| {
        fields[3].1 = Field::Structs("LinkConnection", vec![]);
        objects[0].1[1].1 = Field::RawPointers(vec![3, 6, 7, 8, 9, 10, 11, 14, 15, 18, 21]);
        objects[13] = ("TrafficComponentData", vec![]);
        for (key, value) in fields.iter_mut() {
            if *key == "Name" {
                *value = Field::CString(BLUEPRINT.into())
            }
        }
        blueprint = Some(asset(
            8,
            BLUEPRINT,
            "RaceVehicleBlueprint",
            fields.clone(),
            objects.clone(),
        ));
    });
    let persona = vec![
        ("PersonaName", Field::CString("SyntheticTraffic".into())),
        ("VehicleItemData", import(6)),
        ("VehicleTags", Field::Imports(vec![reference(5)])),
        ("AdditionalItems", Field::RawPointers(vec![])),
        ("OverrideCarColor", Field::Bool(false)),
        ("OverridePerformance", Field::Bool(false)),
        ("OverrideGripValue", Field::Bool(false)),
    ];
    assets.extend([
        asset(
            1,
            "synthetic/traffic/manager",
            "LogicPrefabBlueprint",
            vec![
                ("Flags", Field::UInt(14)),
                ("Objects", Field::RawPointers(vec![2, 3, 4, 5, 6])),
                (
                    "LinkConnections",
                    Field::Structs(
                        "LinkConnection",
                        vec![
                            super::connection(3, links().provider, 6, 0),
                            super::connection(6, links().personas, 5, 0),
                            {
                                let mut row = super::connection(3, links().roads, 0, 0);
                                row[2].1 = import(4);
                                row
                            },
                        ],
                    ),
                ),
            ],
            vec![
                ("AiActorManagerEntityData", vec![]),
                (
                    "TrafficManagerEntityData",
                    vec![("Density", Field::Float(0.5))],
                ),
                ("AiParticipantManagerEntityData", vec![]),
                ("PersonaEntityData", persona),
                ("PersonaSetEntityData", vec![]),
            ],
        ),
        asset(
            3,
            "synthetic/traffic/empty",
            "TrafficSplineEntityData",
            vec![
                ("SpeedSegmentAsset", Field::RawPointer(0)),
                ("DesiredDensity", Field::Int(0)),
                ("VehicleTags", Field::RawPointers(vec![])),
            ],
            vec![],
        ),
        asset(
            4,
            "synthetic/traffic/road",
            "TrafficSplineEntityData",
            vec![
                ("SpeedSegmentAsset", Field::RawPointer(2)),
                ("DesiredDensity", Field::Int(10)),
                ("VehicleTags", Field::Imports(vec![reference(5)])),
            ],
            vec![
                (
                    "SplineSpeedSegmentsAsset",
                    vec![("SpeedSegments", Field::RawPointers(vec![3]))],
                ),
                (
                    "SplineSpeedSegment",
                    vec![
                        ("DesiredSpeed", Field::Float(17.)),
                        (
                            "Spline",
                            Field::Struct(
                                "SplineData",
                                vec![
                                    (
                                        "MatrixFormData",
                                        Field::Struct(
                                            "MatrixForm",
                                            vec![(
                                                "BaseData",
                                                Field::Struct(
                                                    "CubicBase",
                                                    vec![
                                                        ("P0", point(2.)),
                                                        ("P1", point(2. + 10. / 3.)),
                                                        ("P2", point(2. + 20. / 3.)),
                                                        ("P3", point(12.)),
                                                    ],
                                                ),
                                            )],
                                        ),
                                    ),
                                    ("SegmentEndTime", Field::Floats(vec![1.])),
                                    ("SegmentEndDistance", Field::Floats(vec![10.])),
                                    (
                                        "SegmentPoints",
                                        Field::Structs(
                                            "Vec3",
                                            vec![
                                                vec![
                                                    ("x", Field::Float(2.)),
                                                    ("y", Field::Float(3.)),
                                                    ("z", Field::Float(4.)),
                                                ],
                                                vec![
                                                    ("x", Field::Float(12.)),
                                                    ("y", Field::Float(3.)),
                                                    ("z", Field::Float(4.)),
                                                ],
                                            ],
                                        ),
                                    ),
                                ],
                            ),
                        ),
                    ],
                ),
            ],
        ),
        asset(5, "synthetic/traffic/tag", "GameTagAsset", vec![], vec![]),
        asset(
            6,
            "synthetic/traffic/item",
            "RaceVehicleItemData",
            vec![("RaceVehicle", import(7))],
            vec![],
        ),
        asset(
            7,
            "synthetic/traffic/reference",
            "RaceVehicleReference",
            vec![(
                "BundleReference",
                Field::Struct(
                    "AssetReference",
                    vec![("Name", Field::CString(BUNDLE.into()))],
                ),
            )],
            vec![],
        ),
        blueprint.unwrap(),
        asset(
            9,
            BUNDLE,
            "BlueprintBundle",
            vec![("Blueprint", import(8))],
            vec![],
        ),
        asset(
            10,
            &format!("{BUNDLE}_networkregistry_win32"),
            "NetworkRegistryAsset",
            vec![("Objects", Field::Imports(vec![reference(8)]))],
            vec![],
        ),
    ]);
}

fn point(x: f32) -> Field {
    Field::Struct(
        "Vec3",
        vec![
            ("x", Field::Float(x)),
            ("y", Field::Float(3.)),
            ("z", Field::Float(4.)),
        ],
    )
}
