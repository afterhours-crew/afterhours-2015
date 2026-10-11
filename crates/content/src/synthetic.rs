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

pub const TEMPLATE_RVA: u32 = 0x1010;
/// Made-up class IDs for synthetic installations.
pub const SYNTHETIC_CLASS_IDS: [(&str, u32); 2] =
    [("LogicPrefabBlueprint", 10), ("RaceVehicleBlueprint", 20)];

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
        ("UsesScope", Field::Bool(false)),
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
                vec![
                    ("Name", Field::CString(format!("{name}Column{i}"))),
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
            ("DefaultPaint", vector(0.025, 0.11, 0.6)),
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
            vec![("DownforceSetting", Field::Int(5))],
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
    assets
}

/// `LevelData` asset of the synthetic level.
pub const LEVEL: &str = "levels/Synth/Synth";

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
    vec![
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
    ]
}

/// Bundle whose registry lists two prefabs, a vehicle and a non-Blueprint object.
pub const REGISTRY_BUNDLE: &str = "levels/synthetic/gameplay";
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
        },
        limits: Limits::default(),
    };
    Ok((options, template))
}
