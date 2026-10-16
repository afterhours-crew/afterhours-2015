// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Constructed installations for tests of this crate and of its users
//! (feature `synthetic`). They contain no game data: a minimal PE image with a
//! made-up template, five items of different derived layouts, an item system
//! and one table asset per required persistent table.
use crate::{BuildIdentity, Limits, Options, sha256_hex};
pub use nfs_frostbite::synthetic::{Asset, Field, PartitionWriter};
use nfs_services::persistent::required_tables;
use std::path::Path;
pub mod progression;
pub mod sequences;
pub mod vehicles;

pub const TEMPLATE_RVA: u32 = 0x1010;
/// Made-up class IDs for synthetic installations.
pub const SYNTHETIC_CLASS_IDS: [(&str, u32); 4] = [
    ("LogicPrefabBlueprint", 10),
    ("RaceVehicleBlueprint", 20),
    ("ParticipantLogicBlueprint", 30),
    ("ParticipantStateSequenceBlueprint", 40),
];

/// A minimal PE image with one `.data` section holding a 64-byte template.
pub fn executable(template: &[u8; 64]) -> Vec<u8> {
    let mut image = vec![0u8; 0x400];
    image[..2].copy_from_slice(b"MZ");
    image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    image[0x40..0x44].copy_from_slice(b"PE\0\0");
    image[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
    let section = 0x40 + 24;
    image[section..section + 5].copy_from_slice(b".data");
    for (i, word) in [0x1000u32, 0x1000, 0x200, 0x200].iter().enumerate() {
        image[section + 8 + i * 4..section + 12 + i * 4].copy_from_slice(&word.to_le_bytes());
    }
    image[0x210..0x250].copy_from_slice(template);
    image
}

pub fn guid(seed: u8) -> [u8; 16] {
    [seed; 16]
}
pub fn reference(seed: u8) -> ([u8; 16], [u8; 16]) {
    (guid(seed), guid(seed ^ 0x80))
}

/// One item partition (`seed`) with the common item fields and extras.
pub fn item(
    seed: u8,
    class: &str,
    sub_items: Vec<u8>,
    additional: Vec<u8>,
    extra: Vec<(&'static str, Field)>,
) -> Asset {
    let mut fields = vec![
        ("Name", Field::CString(format!("Items/{class}{seed}"))),
        ("BuyPrice", Field::Int(350)),
        ("SellPrice", Field::Int(210)),
        ("Quantity", Field::Int(-1)),
        (
            "InventoryOwnershipLevel",
            Field::Enum(
                "InventoryOwnershipLevelType",
                "InventoryOwnershipLevelType_Purchasable",
                2,
            ),
        ),
        ("Deprecated", Field::Bool(false)),
        ("RequiresOwner", Field::Bool(true)),
        ("UsesScope", Field::Bool(class == "RaceVehicleItemData")),
        ("Purchasable", Field::Bool(true)),
        ("Optional", Field::Bool(seed.is_multiple_of(2))),
        (
            "SubItems",
            Field::Imports(sub_items.into_iter().map(reference).collect()),
        ),
        (
            "AdditionalItems",
            Field::Imports(additional.into_iter().map(reference).collect()),
        ),
    ];
    if !extra.iter().any(|(name, _)| *name == "SortedRestrictions") {
        fields.push(("SortedRestrictions", Field::Guids(vec![])));
    }
    // Explicit synthetic overrides let tests vary constructor and offer flags.
    fields.retain(|(name, _)| !extra.iter().any(|(key, _)| key == name));
    fields.extend(extra);
    let (partition, instance) = reference(seed);
    Asset {
        name: format!("items/{}{seed}", class.to_lowercase()),
        partition: PartitionWriter::default().build(partition, class, instance, fields),
    }
}

pub fn vector(x: f32, y: f32, z: f32) -> Field {
    Field::Struct(
        "Vec3",
        vec![
            ("x", Field::Float(x)),
            ("y", Field::Float(y)),
            ("z", Field::Float(z)),
        ],
    )
}

pub fn table(index: u8, name: &str, secondary: bool, count: usize) -> Asset {
    let columns = (0..count)
        .map(|i| {
            if name == "GarageItemsTable" {
                let names = [
                    "PrimaryVehicleItem",
                    "Vehicle1",
                    "Vehicle2",
                    "Vehicle3",
                    "Vehicle4",
                ];
                vec![
                    ("Name", Field::CString(names[i].into())),
                    (
                        "DataType",
                        Field::Enum("PersistentPropertyType", "PersistentPropertyType_String", 2),
                    ),
                    ("DefaultValue", Field::CString("0".into())),
                ]
            } else {
                // The objective table keeps its real column names: saved
                // objective rows are written by those names at creation.
                let column = if name == nfs_services::fresh_account::OBJECTIVES_TABLE {
                    [
                        nfs_services::fresh_account::ACTIVE_COLUMN,
                        nfs_services::fresh_account::COMPLETED_COLUMN,
                    ][i]
                        .to_string()
                } else {
                    format!("{name}Column{i}")
                };
                vec![
                    ("Name", Field::CString(column)),
                    (
                        "DataType",
                        Field::Enum("PersistentPropertyType", "PersistentPropertyType_Int", 0),
                    ),
                    ("DefaultValue", Field::CString(String::new())),
                ]
            }
        })
        .collect();
    Asset {
        name: format!("tables/{}", name.to_lowercase()),
        partition: PartitionWriter::default().build(
            [0xA0 ^ index; 16],
            "PersistentTableAsset",
            [0x50 ^ index; 16],
            vec![
                ("Name", Field::CString(format!("Tables/{name}"))),
                ("TableName", Field::CString(name.into())),
                (
                    "Properties",
                    Field::Structs("PersistentPropertyInfo", columns),
                ),
                ("HasSecondaryKey", Field::Bool(secondary)),
                ("WriteWholeRows", Field::Bool(false)),
            ],
        ),
    }
}

/// Vehicle item 3; stored field order differs from the wire order on purpose.
pub fn vehicle(plate: &str) -> Asset {
    item(
        3,
        "RaceVehicleItemData",
        vec![],
        vec![2],
        vec![
            (
                "SortedScope",
                Field::Guids(vec![reference(1).1, reference(2).1]),
            ),
            ("DefaultPaint", vector(0.025, 0.11, 0.6)),
            ("RaceVehicle", vehicles::import(1)),
            ("Material", vector(0.76, 0.73, 1.0)),
            ("TopSpeedMPH", Field::Int(155)),
            ("WindowTint", Field::Float(0.9)),
            ("DefaultLicensePlateText", Field::CString(plate.into())),
            ("MaxHP", Field::Int(327)),
            ("MaxTorque", Field::Int(397)),
            ("ZeroToSixty", Field::Float(4.79)),
            ("ZeroToOneHundred", Field::Float(10.97)),
            ("QuarterMileMPH", Field::Int(110)),
            ("QuarterMileTime", Field::Float(13.26)),
            ("VehicleItemFlags", Field::Int(0)),
        ],
    )
}

pub fn assets() -> Vec<Asset> {
    let mut assets = vec![
        item(
            1,
            "SpoilerItemData",
            vec![],
            vec![],
            vec![
                ("DownforceSetting", Field::Int(5)),
                ("SpoilerTuning", Field::RawPointer(0)),
                ("WideCollision", Field::Bool(false)),
                ("PerformanceModifier", Field::RawPointer(0)),
                ("MeshGuids", Field::Guids(vec![])),
            ],
        ),
        item(2, "BodyKitItemData", vec![1], vec![], vec![]),
        vehicle("GHOST"),
        item(4, "TimedDiscountItemData", vec![], vec![], vec![]),
        item(
            5,
            "LiveryCustomizationItemData",
            vec![],
            vec![],
            vec![
                ("ByteVaultDataId", Field::Int(0)),
                ("ByteVaultHash", Field::Long(-13)),
                ("StaticLiveryIndex", Field::Int(2)),
                ("LibraryPresetId", Field::Int(-1)),
                ("DynamicFlags", Field::Int(0)),
            ],
        ),
    ];
    assets.push(Asset {
        name: crate::items::ITEM_SYSTEM.into(),
        partition: PartitionWriter::default().build(
            guid(0x10),
            "ItemSystemData",
            guid(0x11),
            vec![
                ("Name", Field::CString("Items/GameItemSystem".into())),
                ("Items", Field::Imports((1..=5).map(reference).collect())),
                (
                    "Inventory",
                    Field::Imports(vec![reference(3), reference(1)]),
                ),
            ],
        ),
    });
    for (i, (name, secondary, count)) in required_tables().iter().enumerate() {
        assets.push(table(i as u8, name, secondary.is_some(), *count));
    }
    assets.extend(registries());
    assets.extend(level());
    assets.extend(runtime_world());
    assets.extend(entitlements());
    assets.extend(speedwalls());
    assets.extend(reputation(vec![0, 13, 57]));
    assets.extend(vehicles::assets());
    assets
}

/// One SpeedWall references three modes, including adjacent and repeated hashes.
pub fn speedwalls() -> Vec<Asset> {
    let mut assets = vec![Asset {
        name: "gameplay/syntheticspeedwall".into(),
        partition: PartitionWriter::default().build(
            guid(0x73),
            "SpeedWallAsset",
            guid(0xF3),
            vec![
                ("CategoryName", Field::CString("SpeedWall_Synthetic".into())),
                (
                    "GameModeIds",
                    Field::Imports(vec![
                        reference(0x75),
                        reference(0x74),
                        reference(0x76),
                        reference(0x74),
                    ]),
                ),
            ],
        ),
    }];
    for (seed, id) in [(0x74, 10), (0x75, 11), (0x76, 99)] {
        assets.push(Asset {
            name: format!("modes/synthetic{seed}"),
            partition: PartitionWriter::default().build(
                guid(seed),
                "GameModeId",
                guid(seed ^ 0x80),
                vec![("IdHash", Field::UInt(id))],
            ),
        });
    }
    assets
}

/// An active client service with repeated licenses and a foreign-platform query.
pub fn entitlements() -> Vec<Asset> {
    let mut assets = vec![Asset {
        name: crate::licenses::SERVICE.into(),
        partition: PartitionWriter::default().build(
            guid(0x70),
            "PresenceEntitlementServiceData",
            guid(0xF0),
            vec![(
                "EntitlementQueries",
                Field::Imports(vec![reference(0x71), reference(0x72)]),
            )],
        ),
    }];
    for (seed, platform, names) in [
        (
            0x71,
            "GamePlatform_Win32",
            vec!["SyntheticA", "SyntheticB", "SyntheticA"],
        ),
        (0x72, "GamePlatform_Xenon", vec!["Foreign"]),
    ] {
        assets.push(Asset {
            name: format!("queries/synthetic{seed}"),
            partition: PartitionWriter::default().build(
                guid(seed),
                "EntitlementQuery",
                guid(seed ^ 0x80),
                vec![
                    (
                        "Platform",
                        Field::Enum("GamePlatform", platform, if seed == 0x71 { 1 } else { 2 }),
                    ),
                    (
                        "EntitlementsData",
                        Field::Structs(
                            "EntitlementData",
                            names
                                .into_iter()
                                .map(|name| vec![("License", Field::CString(name.into()))])
                                .collect(),
                        ),
                    ),
                ],
            ),
        });
    }
    assets
}

/// `LevelData` asset of the synthetic level.
pub const LEVEL: &str = "levels/Synth/Synth";

/// Constructed startup with the host's required roles and ten traffic slots.
pub const RUNTIME_LEVEL: &str = "levels/Runtime/Runtime";

pub fn runtime_world() -> Vec<Asset> {
    let roles = [
        "Garage",
        "SpawnPoints",
        "Gameplay",
        "Progression",
        "ParticipantStateMachine",
    ];
    let mut extras: Vec<_> = roles
        .iter()
        .map(|role| sub_world(&format!("levels/Runtime/{role}"), true))
        .collect();
    extras.push(layer_reference(0x26));
    let mut result = vec![Asset {
        name: RUNTIME_LEVEL.to_lowercase(),
        partition: PartitionWriter::default().build_many(
            guid(0x20),
            "LevelData",
            guid(0x30),
            vec![
                ("Name", Field::CString(RUNTIME_LEVEL.into())),
                ("Objects", Field::RawPointers(vec![7])),
            ],
            extras,
        ),
    }];
    for (i, role) in roles.iter().enumerate() {
        result.push(scene(
            0x21 + i as u8,
            &format!("levels/Runtime/{role}"),
            "SubWorldData",
            vec![],
        ));
    }
    let mut layer = vec![("SyntheticRpc", vec![])];
    for i in 0..10 {
        layer.push((
            "BlueprintBundleEntityData",
            vec![(
                "Bundle",
                Field::Struct(
                    "BundleReference",
                    bundle_ref(&format!("vehicles/runtime/{i}_bundle")),
                ),
            )],
        ));
        result.push(Asset {
            name: format!("vehicles/runtime/{i}_bundle"),
            partition: PartitionWriter::default().build(
                guid(0x40 + i),
                "BlueprintBundle",
                guid(0xC0 + i),
                vec![("Blueprint", Field::Import(guid(0x61), guid(0x71)))],
            ),
        });
    }
    layer.push((
        "BundlePreLoadEntityData",
        vec![(
            "BundlesToLoad",
            Field::Structs(
                "BundleReference",
                vec![bundle_ref("vehicles/shared/parts_bundle")],
            ),
        )],
    ));
    result.push(Asset {
        name: format!("{}/layer1", RUNTIME_LEVEL.to_lowercase()),
        partition: PartitionWriter::default().build_many(
            guid(0x26),
            "WorldPartData",
            guid(0x36),
            // Traffic composition is reversed; startup discovery uses partition order.
            vec![(
                "Objects",
                Field::RawPointers(std::iter::once(2).chain((3..=12).rev()).collect()),
            )],
            layer,
        ),
    });
    result.extend([
        blueprint(
            0x27,
            "gameplay/stats/garagestatslogic",
            "ParticipantLogicBlueprint",
        ),
        blueprint(
            0x28,
            "gameplay/garageloadlogic",
            "ParticipantLogicBlueprint",
        ),
        blueprint(0x29, "gameplay/unusedlogic", "ParticipantLogicBlueprint"),
        registry(
            0x2A,
            "levels/runtime/gameplay",
            vec![(guid(0x29), guid(0x39)), (guid(0x27), guid(0x37))],
        ),
        registry(
            0x2B,
            "levels/runtime/garage",
            vec![(guid(0x28), guid(0x38))],
        ),
    ]);
    result.retain(|a| {
        ![
            "levels/runtime/garage",
            "levels/runtime/gameplay",
            "levels/runtime/spawnpoints",
            "levels/runtime/progression",
        ]
        .contains(&a.name.as_str())
    });
    result.extend(garage_layout());
    result.extend(progression::assets());
    sequences::install(&mut result);
    result
}

/// Deliberately unrelated to the supported executable's port identifiers.
pub fn garage_links() -> crate::garage::Links {
    crate::garage::Links {
        primary_slot: 100,
        secondary_slots: 101,
        slot_location: 102,
        locator_transform: 103,
        exit_location: 104,
        items: [200, 201, 202, 203, 204],
    }
}

pub(crate) fn connection(
    source: u32,
    source_field: u32,
    target: u32,
    target_field: u32,
) -> Vec<(&'static str, Field)> {
    vec![
        ("Source", Field::RawPointer(source)),
        ("SourceFieldId", Field::Int(source_field as i32)),
        ("Target", Field::RawPointer(target)),
        ("TargetFieldId", Field::Int(target_field as i32)),
    ]
}

type ObjectFields = Vec<(&'static str, Field)>;
type SceneObjects = Vec<(&'static str, ObjectFields)>;

pub fn garage_layout() -> Vec<Asset> {
    garage_layout_with(|_, _, _, _| {})
}

pub(crate) fn garage_layout_with(
    edit: impl FnOnce(&mut ObjectFields, &mut SceneObjects, &mut ObjectFields, &mut SceneObjects),
) -> Vec<Asset> {
    let ports = garage_links();
    let mut garage_objects = vec![("GarageEntityData", vec![])];
    garage_objects.extend((0..5).map(|_| ("VehicleSlotEntityData", vec![])));
    garage_objects.extend((0..5).map(|i| {
        (
            "LocatorEntityData",
            vec![(
                "Transform",
                Field::Struct(
                    "LinearTransform",
                    vec![
                        ("right", vector(1.0, 0.0, 0.0)),
                        ("up", vector(0.0, 1.0, 0.0)),
                        ("forward", vector(0.0, 0.0, 1.0)),
                        ("trans", vector(i as f32 * 7.0, 2.0, -3.0)),
                    ],
                ),
            )],
        )
    }));
    let links = (0..5)
        .map(|i| {
            connection(
                2,
                if i == 0 {
                    ports.primary_slot
                } else {
                    ports.secondary_slots
                },
                i + 3,
                0,
            )
        })
        .collect();
    let properties = (0..5)
        .map(|i| {
            let mut row = connection(i + 8, ports.locator_transform, i + 3, ports.slot_location);
            row.push(("Flags", Field::UInt(1)));
            row
        })
        .collect();
    let mut gameplay_objects = vec![("PersistentGarageVehiclesEntityData", vec![])];
    gameplay_objects.extend((0..5).map(|_| ("ItemEntityData", vec![])));
    let mut garage_fields = vec![
        ("Name", Field::CString("levels/Runtime/Garage".into())),
        (
            "Objects",
            Field::RawPointers(
                std::iter::once(2)
                    .chain((3..=7).rev())
                    .chain(8..=12)
                    .collect(),
            ),
        ),
        ("LinkConnections", Field::Structs("LinkConnection", links)),
        (
            "PropertyConnections",
            Field::Structs("PropertyConnection", properties),
        ),
    ];
    let mut gameplay_fields = vec![
        ("Name", Field::CString("levels/Runtime/Gameplay".into())),
        (
            "Objects",
            Field::RawPointers(std::iter::once(2).chain((3..=7).rev()).collect()),
        ),
        (
            "LinkConnections",
            Field::Structs(
                "LinkConnection",
                (0..5)
                    .map(|i| connection(2, ports.items[i], i as u32 + 3, 0))
                    .collect(),
            ),
        ),
    ];
    edit(
        &mut garage_fields,
        &mut garage_objects,
        &mut gameplay_fields,
        &mut gameplay_objects,
    );
    vec![
        Asset {
            name: "levels/runtime/garage".into(),
            partition: PartitionWriter::default().build_many(
                guid(0x21),
                "SubWorldData",
                guid(0x31),
                garage_fields,
                garage_objects,
            ),
        },
        Asset {
            name: "levels/runtime/gameplay".into(),
            partition: PartitionWriter::default().build_many(
                guid(0x23),
                "SubWorldData",
                guid(0x33),
                gameplay_fields,
                gameplay_objects,
            ),
        },
        exit_layout_with(|_, _| {}),
    ]
}

pub(crate) fn exit_layout_with(edit: impl FnOnce(&mut ObjectFields, &mut SceneObjects)) -> Asset {
    let ports = garage_links();
    let locator = |x| {
        (
            "LocatorEntityData",
            vec![(
                "Transform",
                Field::Struct(
                    "LinearTransform",
                    vec![
                        ("right", vector(1.0, 0.0, 0.0)),
                        ("up", vector(0.0, 1.0, 0.0)),
                        ("forward", vector(0.0, 0.0, 1.0)),
                        ("trans", vector(x, 5.0, -7.0)),
                    ],
                ),
            )],
        )
    };
    // The destination is the second locator. The first is a decoy, and the
    // interface descriptor is metadata rather than a constructed scene object.
    let mut objects = vec![
        locator(10.0),
        locator(20.0),
        (
            "InterfaceDescriptorData",
            vec![(
                "Fields",
                Field::Structs(
                    "FieldDescriptor",
                    vec![vec![
                        ("Id", Field::Int(ports.exit_location as i32)),
                        (
                            "AccessType",
                            Field::Enum("FieldAccessType", "FieldAccessType_Source", 0),
                        ),
                    ]],
                ),
            )],
        ),
    ];
    let mut property = connection(3, ports.locator_transform, 4, ports.exit_location);
    property.push(("Flags", Field::UInt(17)));
    let mut fields = vec![
        ("Name", Field::CString("levels/Runtime/SpawnPoints".into())),
        ("Objects", Field::RawPointers(vec![2, 3])),
        ("Interface", Field::RawPointer(4)),
        (
            "PropertyConnections",
            Field::Structs("PropertyConnection", vec![property]),
        ),
    ];
    edit(&mut fields, &mut objects);
    Asset {
        name: "levels/runtime/spawnpoints".into(),
        partition: PartitionWriter::default().build_many(
            guid(0x22),
            "SubWorldData",
            guid(0x32),
            fields,
            objects,
        ),
    }
}

fn sub_world(name: &str, auto: bool) -> (&'static str, Vec<(&'static str, Field)>) {
    (
        "SubWorldReferenceObjectData",
        vec![
            ("BundleName", Field::CString(name.into())),
            ("AutoLoad", Field::Bool(auto)),
            ("IsWin32SubLevel", Field::Bool(true)),
        ],
    )
}

fn layer_reference(seed: u8) -> (&'static str, Vec<(&'static str, Field)>) {
    (
        "WorldPartReferenceObjectData",
        vec![("Blueprint", Field::Import(guid(seed), guid(seed | 0x10)))],
    )
}

fn bundle_ref(name: &str) -> Vec<(&'static str, Field)> {
    vec![("Name", Field::CString(name.into()))]
}

fn scene(
    seed: u8,
    name: &str,
    class: &str,
    extras: Vec<(&'static str, Vec<(&'static str, Field)>)>,
) -> Asset {
    Asset {
        name: name.to_lowercase(),
        partition: PartitionWriter::default().build_many(
            guid(seed),
            class,
            guid(seed | 0x10),
            vec![("Name", Field::CString(name.into()))],
            extras,
        ),
    }
}

/// A level with sub-levels Alpha (child Gamma), Off (not auto-loaded) and
/// Beta; the root layer has two blueprint bundles, Alpha's layer a preload.
pub fn level() -> Vec<Asset> {
    let mut assets = vec![
        scene(
            0xC0,
            LEVEL,
            "LevelData",
            vec![
                sub_world("Levels/Synth/Alpha", true),
                sub_world("levels/Synth/Off", false),
                sub_world("levels/Synth/Beta", true),
                layer_reference(0xC5),
            ],
        ),
        scene(
            0xC1,
            "Levels/Synth/Alpha",
            "SubWorldData",
            vec![sub_world("Levels/Synth/Gamma", true), layer_reference(0xC6)],
        ),
        scene(0xC2, "levels/Synth/Beta", "SubWorldData", vec![]),
        scene(0xC3, "Levels/Synth/Gamma", "SubWorldData", vec![]),
        scene(
            0xC5,
            "levels/Synth/Synth/layer1",
            "WorldPartData",
            vec![
                (
                    "BlueprintBundleEntityData",
                    vec![(
                        "Bundle",
                        Field::Struct("BundleReference", bundle_ref("Vehicles/Traffic/One_Bundle")),
                    )],
                ),
                (
                    "BlueprintBundleEntityData",
                    vec![(
                        "Bundle",
                        Field::Struct("BundleReference", bundle_ref("Vehicles/Traffic/Two_Bundle")),
                    )],
                ),
            ],
        ),
        scene(
            0xC6,
            "levels/Synth/Alpha/layer1",
            "WorldPartData",
            vec![(
                "BundlePreLoadEntityData",
                vec![(
                    "BundlesToLoad",
                    Field::Structs(
                        "BundleReference",
                        vec![bundle_ref("vehicles/Shared/Parts_Bundle")],
                    ),
                )],
            )],
        ),
    ];
    for (seed, name) in [
        (0xD7, "vehicles/shared/parts_bundle"),
        (0xD8, "vehicles/traffic/one_bundle"),
        (0xD9, "vehicles/traffic/two_bundle"),
        (0xDA, "vehicles/synthetic_bundle"),
    ] {
        assets.push(Asset {
            name: name.into(),
            partition: PartitionWriter::default().build(
                guid(seed),
                "BlueprintBundle",
                guid(seed ^ 0x80),
                vec![("Blueprint", Field::Import(guid(0x61), guid(0x71)))],
            ),
        });
    }
    assets
}

/// Bundle whose registry lists two prefabs, a vehicle and a non-Blueprint object.
pub const REGISTRY_BUNDLE: &str = "levels/synthetic/gameplay";

/// Synthetic calculator and a threshold asset under a deliberately different name.
pub fn reputation(values: Vec<i32>) -> Vec<Asset> {
    let (partition, instance) = reference(0x77);
    let calculator = crate::reputation::CALCULATOR;
    let thresholds = "synthetic/scoring/levels";
    vec![
        Asset {
            name: calculator.into(),
            partition: PartitionWriter::default().build_many(
                partition,
                "ParticipantLogicBlueprint",
                instance,
                vec![("Name", Field::CString(calculator.into()))],
                vec![(
                    "IntScoreToIntLevelEntityData",
                    vec![(
                        "LevelThresholdsAsset",
                        Field::Import(reference(0x78).0, reference(0x78).1),
                    )],
                )],
            ),
        },
        Asset {
            name: thresholds.into(),
            partition: PartitionWriter::default().build(
                reference(0x78).0,
                "IntScoreToIntLevelAsset",
                reference(0x78).1,
                vec![
                    ("Name", Field::CString(thresholds.into())),
                    ("LevelThresholds", Field::Ints(values)),
                ],
            ),
        },
    ]
}
/// Bundle whose registry lists a Blueprint class with no profiled class ID.
pub const UNPROFILED_BUNDLE: &str = "levels/synthetic/unprofiled";

fn blueprint(seed: u8, name: &str, class: &str) -> Asset {
    Asset {
        name: name.into(),
        partition: PartitionWriter::default().build(
            guid(seed),
            class,
            guid(seed | 0x10),
            vec![("Name", Field::CString(name.into()))],
        ),
    }
}

fn registry(seed: u8, bundle: &str, objects: Vec<([u8; 16], [u8; 16])>) -> Asset {
    let name = format!("{bundle}_networkregistry_win32");
    Asset {
        partition: PartitionWriter::default().build(
            guid(seed),
            "NetworkRegistryAsset",
            guid(seed | 0x10),
            vec![
                ("Name", Field::CString(name.clone())),
                ("Objects", Field::Imports(objects)),
            ],
        ),
        name,
    }
}

/// Blueprint assets and two bundle registries over them.
pub fn registries() -> Vec<Asset> {
    let object = |seed: u8| (guid(seed), guid(seed | 0x10));
    vec![
        blueprint(0x60, "prefabs/first", "LogicPrefabBlueprint"),
        blueprint(0x61, "vehicles/synthetic", "RaceVehicleBlueprint"),
        blueprint(0x62, "prefabs/second", "LogicPrefabBlueprint"),
        blueprint(0x63, "objects/unprofiled", "ObjectBlueprint"),
        registry(
            0x64,
            REGISTRY_BUNDLE,
            vec![
                object(0x60),
                object(0x61),
                (guid(0x10), guid(0x11)),
                object(0x62),
            ],
        ),
        registry(0x65, UNPROFILED_BUNDLE, vec![object(0x63)]),
    ]
}

/// Write an installation of `assets` under `root`. Returns options whose build
/// identity matches the written executable, and the template it embeds.
pub fn write(root: &Path, assets: &[Asset]) -> std::io::Result<(Options, [u8; 64])> {
    let template: [u8; 64] = std::array::from_fn(|i| (i * 3 + 1) as u8);
    let image = executable(&template);
    nfs_frostbite::synthetic::write_installation(root, &image, assets)?;
    let options = Options {
        identity: BuildIdentity {
            executable_sha256: sha256_hex(&image),
            mac_template_rva: TEMPLATE_RVA,
            mac_template_sha256: sha256_hex(&template),
            blueprint_class_ids: SYNTHETIC_CLASS_IDS
                .iter()
                .map(|(c, id)| (c.to_string(), *id))
                .collect(),
            garage_links: garage_links(),
            vehicles: vehicles::rules(),
            progression: progression::rules(),
            sequences: sequences::rules(),
            participant_logic: crate::BuildIdentity::supported().participant_logic,
            scenes: crate::scenes::Rules {
                level: RUNTIME_LEVEL.to_owned(),
                classes: [
                    ("LevelData", None),
                    ("StateSequenceRefererEntityData", None),
                    ("LogicPrefabReferenceObjectData", None),
                    ("LogicPrefabBlueprint", None),
                    ("ScriptedAISplineEntityData", None),
                    (
                        "StateSequenceManagerEntityData",
                        Some(nfs_world::replication::sublevel::Kind::Rpc),
                    ),
                    ("SubWorldData", None),
                    ("ParticipantLogicSpawnerEntityData", None),
                    (
                        "LinkedSchematicChannelEntityData",
                        Some(nfs_world::replication::sublevel::Kind::Rpc),
                    ),
                    ("WorldPartData", None),
                    ("SubWorldReferenceObjectData", None),
                    ("WorldPartReferenceObjectData", None),
                    (
                        "BlueprintBundleEntityData",
                        Some(nfs_world::replication::sublevel::Kind::LevelReference),
                    ),
                    ("BundlePreLoadEntityData", None),
                    (
                        "GarageEntityData",
                        Some(nfs_world::replication::sublevel::Kind::Rpc),
                    ),
                    (
                        "VehicleSlotEntityData",
                        Some(nfs_world::replication::sublevel::Kind::Rpc),
                    ),
                    (
                        "ItemEntityData",
                        Some(nfs_world::replication::sublevel::Kind::Rpc),
                    ),
                    ("PersistentGarageVehiclesEntityData", None),
                    ("LocatorEntityData", None),
                    (
                        "SyntheticRpc",
                        Some(nfs_world::replication::sublevel::Kind::Rpc),
                    ),
                    (
                        "SyntheticBool",
                        Some(nfs_world::replication::sublevel::Kind::BoolProperty),
                    ),
                ]
                .into_iter()
                .map(|(n, k)| (n.to_owned(), k))
                .collect(),
                launchers: ["SyntheticRpc".to_owned()].into(),
            },
        },
        limits: Limits::default(),
    };
    Ok((options, template))
}
