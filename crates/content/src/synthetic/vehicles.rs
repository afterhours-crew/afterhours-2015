// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An authored vehicle with deliberately different mesh-link and object order.
use super::{Asset, Field, ObjectFields, PartitionWriter, SceneObjects, connection, vector};
use crate::vehicle_profiles::Rules;

pub const BLUEPRINT: &str = "vehicles/player/synthetic/car";
pub const BUNDLE: &str = "vehicles/player/synthetic/car_bundle";
pub const MESH_LINK: u32 = 0x8123_4567;

fn guid(seed: u8) -> [u8; 16] {
    let mut bytes = [0xf1; 16];
    bytes[15] = seed;
    bytes
}
pub(super) fn import(seed: u8) -> Field {
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
    let mut classes = [
        ("RaceVehicleEntityData", "root"),
        ("RaceVehicleComponentData", "chassis"),
        ("PartComponentData", "part"),
        ("FxComponentData", "triple_nibbles"),
        ("PlayerEntryComponentData", "bool"),
        ("RaceWheelComponentData", "wheel"),
        ("NOSComponentData", "index"),
        ("WheelCustomizationComponentData", "tuning"),
        ("SuspensionComponentData", "empty"),
        ("RaceVehicleCustomizationComponentData", "appearance"),
        ("AuthoritativeComponentData", "four_bit"),
        ("LockOnComponentData", "tagged"),
        ("RaceVehicleAudioComponentData", "nibbles_guid"),
        ("MeshCustomizationNetworkComponentData", "mesh"),
        ("RaceVehicleHealthComponentData", "guid_float"),
    ]
    .map(|(a, b)| (a.to_owned(), Some(b.to_owned())))
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();
    for class in [
        "RaceVehicleBlueprint",
        "LogicPrefabBlueprint",
        "MeshCustomizationComponentData",
        "RimCustomizationComponentData",
    ] {
        classes.insert(class.into(), None);
    }
    Rules {
        classes,
        bus_classes: ["RaceVehicleBlueprint".into(), "LogicPrefabBlueprint".into()].into(),
        mesh_classes: [
            ("MeshCustomizationComponentData".into(), "none".into()),
            ("RimCustomizationComponentData".into(), "rim".into()),
        ]
        .into(),
        mesh_link: MESH_LINK,
        bundle_prefix: "vehicles/player/".into(),
    }
}

pub fn assets() -> Vec<Asset> {
    assets_with(|_, _| {})
}

/// Edit the blueprint fields or its unexported objects for malformed-input tests.
pub fn assets_with(edit: impl FnOnce(&mut ObjectFields, &mut SceneObjects)) -> Vec<Asset> {
    let mut app = vec![("StaticLiveryTextures", import(7))];
    for (i, name) in [
        "DefaultPrimaryPaintFrontRimsColor",
        "DefaultPrimaryPaintFrontRimsMaterial",
        "DefaultSecondaryPaintFrontRimsColor",
        "DefaultSecondaryPaintFrontRimsMaterial",
        "DefaultPrimaryPaintRearRimsColor",
        "DefaultPrimaryPaintRearRimsMaterial",
        "DefaultSecondaryPaintRearRimsColor",
        "DefaultSecondaryPaintRearRimsMaterial",
    ]
    .into_iter()
    .enumerate()
    {
        app.push((name, vector(i as f32 / 8., 0.25, 0.75)));
    }
    // One-based pointers: primary blueprint=1, entity=2, chassis=3, part=4,
    // mesh network=17, health=18, base mesh=19, rim mesh=20, child bus=21.
    let mut objects = vec![
        (
            "RaceVehicleEntityData",
            vec![
                ("DefaultTeam", Field::Enum("Team", "Team_First", 1)),
                (
                    "Components",
                    Field::RawPointers((3..=21).filter(|i| *i != 4).collect()),
                ),
            ],
        ),
        (
            "RaceVehicleComponentData",
            vec![
                ("RaceVehicleConfig", import(5)),
                ("Components", Field::RawPointers(vec![4])),
            ],
        ),
        (
            "PartComponentData",
            vec![
                ("HealthStates", Field::RawPointers(vec![0, 0])),
                ("IsNetworkable", Field::Bool(true)),
            ],
        ),
        ("FxComponentData", vec![]),
        ("PlayerEntryComponentData", vec![]),
        ("RaceWheelComponentData", vec![]),
        ("RaceWheelComponentData", vec![]),
        ("RaceWheelComponentData", vec![]),
        ("RaceWheelComponentData", vec![]),
        ("NOSComponentData", vec![]),
        (
            "WheelCustomizationComponentData",
            vec![
                ("FrontWheelDiameter", Field::Float(0.5)),
                ("RearWheelDiameter", Field::Float(0.625)),
            ],
        ),
        ("RaceVehicleCustomizationComponentData", app),
        ("AuthoritativeComponentData", vec![]),
        ("LockOnComponentData", vec![]),
        // Sound blueprint deliberately unresolved: it is not an entity bus.
        (
            "RaceVehicleAudioComponentData",
            vec![("Blueprint", import(99))],
        ),
        ("MeshCustomizationNetworkComponentData", vec![]),
        (
            "RaceVehicleHealthComponentData",
            vec![
                ("MaxHealth", Field::Float(100.)),
                ("PlayerHealthProfile", import(6)),
            ],
        ),
        (
            "MeshCustomizationComponentData",
            vec![
                ("BitsRequiredForIndex", Field::Int(2)),
                (
                    "CustomizableMeshInfos",
                    Field::Imports(vec![(guid(8), guid(0x88))]),
                ),
            ],
        ),
        (
            "RimCustomizationComponentData",
            vec![
                ("BitsRequiredForIndex", Field::Int(3)),
                (
                    "CustomizableMeshInfos",
                    Field::Imports(vec![(guid(9), guid(0x89))]),
                ),
            ],
        ),
        ("LogicPrefabBlueprint", vec![("Flags", Field::UInt(0))]),
    ];
    let mut fields = vec![
        ("Name", Field::CString(BLUEPRINT.into())),
        ("Flags", Field::UInt(1)),
        ("Object", Field::RawPointer(2)),
        (
            "LinkConnections",
            Field::Structs(
                "LinkConnection",
                vec![
                    connection(17, MESH_LINK, 20, 0),
                    connection(17, MESH_LINK, 19, 0),
                ],
            ),
        ),
    ];
    edit(&mut fields, &mut objects);
    let curve = (0..10)
        .map(|i| {
            let (x, y) = match i {
                0 => (-1., 0.),
                1 => (1., 100.),
                _ => ((i - 2) as f32 / 7., 0.5),
            };
            vec![
                ("x", Field::Float(x)),
                ("y", Field::Float(y)),
                ("z", Field::Float(123.)),
            ]
        })
        .collect();
    let chassis = vec![
        ("Mass", Field::Float(1000.)),
        ("FrontAxle", Field::Float(1.)),
        ("WheelBase", Field::Float(2.)),
        ("FrontWeightBias", Field::Structs("Vec3", curve)),
    ];
    let suspension = [
        "RideHeightSpecsFront",
        "SpringSpecsFront",
        "SpringProgressionFront",
        "RideHeightSpecsRear",
        "SpringSpecsRear",
        "SpringProgressionRear",
    ]
    .into_iter()
    .zip([8., 120., 1., 9., 150., 1.5])
    .map(|(n, v)| (n, Field::Float(v)))
    .collect();
    vec![
        asset(
            1,
            "vehicles/player/synthetic/reference",
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
        asset(
            2,
            BUNDLE,
            "BlueprintBundle",
            vec![("Blueprint", import(3))],
            vec![],
        ),
        asset(3, BLUEPRINT, "RaceVehicleBlueprint", fields, objects),
        asset(
            4,
            &format!("{BUNDLE}_networkregistry_win32"),
            "NetworkRegistryAsset",
            vec![("Objects", Field::Imports(vec![(guid(3), guid(0x83))]))],
            vec![],
        ),
        asset(
            5,
            "vehicles/player/synthetic/config",
            "RaceVehicleConfigData",
            vec![
                ("Chassis", Field::RawPointer(2)),
                ("Suspension", Field::RawPointer(3)),
                (
                    "VehicleModeAtReset",
                    Field::Enum("VehicleMode", "VehicleMode_Parked", 2),
                ),
            ],
            vec![
                ("RaceVehicleChassisConfigData", chassis),
                ("RaceVehicleSuspensionConfigData", suspension),
            ],
        ),
        asset(
            6,
            "vehicles/player/synthetic/health",
            "RaceVehicleHealthProfileConfigData",
            vec![(
                "Segments",
                Field::Structs(
                    "HealthSegment",
                    vec![
                        vec![("Size", Field::Int(70))],
                        vec![("Size", Field::Int(30))],
                    ],
                ),
            )],
            vec![],
        ),
        asset(
            7,
            "vehicles/player/synthetic/wraps",
            "StaticLiveryTexturesAsset",
            vec![
                ("DiffuseTextures", Field::RawPointers(vec![0, 0])),
                ("HighlightTextures", Field::RawPointers(vec![0, 0, 0])),
            ],
            vec![],
        ),
        asset(
            8,
            "vehicles/player/synthetic/base_mesh",
            "CustomizableMeshInfo",
            vec![
                ("BlueprintGuids", Field::Guids(vec![guid(0x41), guid(0x42)])),
                ("BundledWithComponent", Field::Bool(true)),
            ],
            vec![],
        ),
        asset(
            9,
            "vehicles/player/synthetic/rim_mesh",
            "CustomizableMeshInfo",
            vec![
                ("BlueprintGuids", Field::Guids(vec![guid(0x43)])),
                ("BundledWithComponent", Field::Bool(false)),
                (
                    "OwningBundleReference",
                    Field::Struct(
                        "AssetReference",
                        vec![("Name", Field::CString("parts/synthetic/rims".into()))],
                    ),
                ),
            ],
            vec![],
        ),
    ]
}
