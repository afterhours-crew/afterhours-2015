// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[test]
fn transmission_follows_imports_and_internal_links_and_rejects_bad_targets() {
    use crate::synthetic::{Asset, Field, PartitionWriter, guid, reference};
    let dir = std::env::temp_dir().join(format!(
        "nfs-content-transmission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let target = |max| {
        vec![(
            "ManualTransmissionTuning",
            Field::Struct(
                "SyntheticRange",
                vec![
                    ("SliderRangeMax", Field::Int(max)),
                    ("SliderRangeMin", Field::Int(2)),
                ],
            ),
        )]
    };
    let assets = vec![
        Asset {
            name: "source".into(),
            partition: PartitionWriter::default().build_many(
                guid(1),
                "PersistantTuningItemData",
                guid(2),
                vec![],
                vec![
                    ("PersistantTuningConfigData", target(13)),
                    ("UnrelatedData", target(9)),
                ],
            ),
        },
        Asset {
            name: "imported".into(),
            partition: PartitionWriter::default().build(
                reference(3).0,
                "PersistantTuningConfigData",
                reference(3).1,
                target(11),
            ),
        },
    ];
    crate::synthetic::write(&dir, &assets).unwrap();
    let install =
        nfs_frostbite::install::Installation::open(&dir, crate::Limits::default()).unwrap();
    let mut assets = Assets::scan(&install).unwrap();
    let fields = |pointer| Fields(vec![("PersistantTuning".into(), Value::Pointer(pointer))]);
    assert_eq!(
        transmission(&mut assets, "source", &fields(Pointer::Internal(1))).unwrap(),
        json!([13, 2])
    );
    assert_eq!(
        transmission(
            &mut assets,
            "source",
            &fields(Pointer::Import {
                partition: reference(3).0,
                instance: reference(3).1
            })
        )
        .unwrap(),
        json!([11, 2])
    );
    for pointer in [
        Pointer::Null,
        Pointer::Internal(2),
        Pointer::Internal(30),
        Pointer::Import {
            partition: guid(40),
            instance: guid(41),
        },
        Pointer::Import {
            partition: reference(3).0,
            instance: guid(41),
        },
    ] {
        assert!(transmission(&mut assets, "source", &fields(pointer)).is_err());
    }
    drop(assets);
    drop(install);
    std::fs::remove_dir_all(dir).unwrap();
}

fn object(class: Class, fields: Vec<(&str, Value)>) -> Object {
    Object {
        class: class.name().into(),
        guid: Some([0x12; 16]),
        fields: Fields(fields.into_iter().map(|(k, v)| (k.into(), v)).collect()),
    }
}
fn put(object: &mut Object, name: &str, value: Value) {
    object.fields.0.retain(|(k, _)| k != name);
    object.fields.0.push((name.into(), value));
}
fn fields(class: Class) -> Object {
    let mut obj = object(class, vec![]);
    if family(class) != Family::Other {
        put(
            &mut obj,
            "MeshGuids",
            Value::Array(vec![Value::Guid([3; 16]), Value::Guid([8; 16])]),
        );
    }
    match family(class) {
        Family::Performance => put(&mut obj, "Percent", Value::Float32(0.25)),
        Family::Appearance => {
            put(&mut obj, "WideCollision", Value::Bool(true));
            put(
                &mut obj,
                "PerformanceModifier",
                Value::Pointer(Pointer::Null),
            );
        }
        Family::Other => (),
    }
    if let Some(name) = tuning_field(class) {
        put(&mut obj, name, Value::Pointer(Pointer::Null));
    }
    obj
}
fn document(rows: &BTreeMap<&str, Json>) -> Json {
    let mut parts = json!({"customization":[],"nos":[],"wheels":[],"appearance":[],"mesh":[]});
    for (name, row) in rows {
        parts[*name] = json!([row]);
    }
    parts
}
fn validate(obj: &Object, transmission: Json) -> Result<BTreeMap<&'static str, Json>, Error> {
    let rows = rows(obj, || Ok(transmission.clone()))?;
    let classes =
        ItemCatalog::new([(obj.guid.unwrap(), Class::from_name(&obj.class).unwrap())]).unwrap();
    Catalog::from_rows(&document(&rows), &classes).map_err(|_| bad("models"))?;
    Ok(rows)
}
fn location(rear: bool) -> Value {
    Value::Enum {
        value: i32::from(rear),
        name: None,
    }
}

#[test]
fn tuning_presence_and_performance_groups_follow_their_own_assets() {
    for class in [
        Class::BrakesItemData,
        Class::ControlArmTuningItemData,
        Class::DifferentialTuningItemData,
        Class::GearboxTuningItemData,
        Class::HandbrakeTuningItemData,
        Class::NosTuningItemData,
        Class::PersistantTuningItemData,
        Class::SteeringTuningItemData,
        Class::SuspensionTuningItemData,
        Class::SwaybarTuningItemData,
        Class::TireCompositionItemData,
        Class::SpoilerItemData,
    ] {
        let mut obj = fields(class);
        put(&mut obj, "GlobalAssetListIndex", Value::UInt32(7));
        if class == Class::NosTuningItemData {
            put(&mut obj, "NOSUpgradeIndex", Value::UInt32(3));
        }
        let empty = validate(&obj, json!([13, 2])).unwrap();
        assert_eq!(
            empty["customization"]["tuning"],
            json!({"asset_present":false})
        );
        assert!(empty["customization"]["chassis"].get("list_a").is_none());
        put(
            &mut obj,
            tuning_field(class).unwrap(),
            Value::Pointer(Pointer::Internal(1)),
        );
        let active = validate(&obj, json!([13, 2])).unwrap();
        assert_eq!(active["customization"]["chassis"]["list_a"], 7);
        assert_eq!(active["customization"]["tuning"]["asset_present"], true);
        if class == Class::PersistantTuningItemData {
            assert_eq!(
                active["customization"]["tuning"]["transmission_values"],
                json!([13, 2])
            );
            assert!(validate(&obj, json!([16, 2])).is_err());
        }
        put(&mut obj, "GlobalAssetListIndex", Value::Int32(256));
        assert!(validate(&obj, json!([13, 2])).is_err());
    }
    let mut clutch = fields(Class::ClutchItemData);
    put(
        &mut clutch,
        "PerformanceModifier",
        Value::Pointer(Pointer::Internal(1)),
    );
    put(&mut clutch, "PerformanceModifierIndex", Value::Int32(5));
    assert_eq!(
        validate(&clutch, Json::Null).unwrap()["customization"]["chassis"]["list_b"],
        5
    );
    let mut induction = fields(Class::ForcedInductionItemData);
    put(
        &mut induction,
        "ForcedInductionUpgradeIndex",
        Value::Int32(9),
    );
    assert_eq!(
        validate(&induction, Json::Null).unwrap()["customization"]["chassis"]["index"],
        9
    );
    put(
        &mut induction,
        "ForcedInductionUpgradeIndex",
        Value::Int32(64),
    );
    assert!(validate(&induction, Json::Null).is_err());
}

fn rim() -> Object {
    let mut obj = fields(Class::RimsItemData);
    for (name, value) in [
        ("Diameter", 0.5),
        ("Width", 0.25),
        ("BrakeDiscRatio", 0.75),
        ("BrakeDiscXOffset", 0.1),
        ("LipSize", 0.03),
    ] {
        put(&mut obj, name, Value::Float32(value));
    }
    put(
        &mut obj,
        "SizeOptions",
        Value::Array(vec![Value::Float32(0.5), Value::Float32(0.625)]),
    );
    put(&mut obj, "Location", location(true));
    put(&mut obj, "AllowWidthOverride", Value::Bool(false));
    obj
}

#[test]
fn rim_fields_feed_chassis_geometry_appearance_and_mesh_without_losing_order() {
    let obj = rim();
    let rows = validate(&obj, Json::Null).unwrap();
    assert_eq!(rows["wheels"]["scale"], 0.75);
    assert_eq!(rows["wheels"]["sizes"], json!([0.5, 0.625]));
    assert_eq!(
        rows["customization"]["chassis"]["rim_values"],
        rows["wheels"]["sizes"]
    );
    assert_eq!(rows["appearance"]["rear"], true);
    assert_eq!(
        rows["mesh"]["guids"],
        json!(["03".repeat(16), "08".repeat(16)])
    );
    assert_eq!(rows["mesh"]["rim"], json!([0.1_f32, 0.5_f32, 0.03_f32]));
    for (name, value) in [
        (
            "Location",
            Value::Enum {
                value: 2,
                name: None,
            },
        ),
        ("Diameter", Value::Float32(f32::NAN)),
        ("SizeOptions", Value::Array(vec![Value::Float32(1.); 257])),
        ("MeshGuids", Value::Array(vec![Value::Guid([1; 16]); 257])),
        ("AllowWidthOverride", Value::Int32(1)),
    ] {
        let mut changed = obj.clone();
        put(&mut changed, name, value);
        assert!(validate(&changed, Json::Null).is_err(), "{name}");
    }
    for name in ["WideCollision", "PerformanceModifier", "MeshGuids"] {
        let mut missing = obj.clone();
        missing.fields.0.retain(|(k, _)| k != name);
        assert!(validate(&missing, Json::Null).is_err(), "{name}");
    }
}

#[test]
fn tire_variants_and_fender_overrides_preserve_axle_and_mesh_groups() {
    let mut tire = fields(Class::TiresItemData);
    put(&mut tire, "Location", location(false));
    let infos = (0..2)
        .map(|i| {
            Value::Struct(Fields(vec![
                ("InnerDiameter".into(), Value::Float32(0.5 + i as f32)),
                ("OuterDiameter".into(), Value::Float32(0.75 + i as f32)),
                ("Width".into(), Value::Float32(0.25)),
                (
                    "TireMeshGuids".into(),
                    Value::Array(vec![Value::Guid([i + 20; 16])]),
                ),
            ]))
        })
        .collect();
    put(&mut tire, "TireMeshInfos", Value::Array(infos));
    let rows = validate(&tire, Json::Null).unwrap();
    assert_eq!(
        rows["wheels"]["alternatives"],
        json!([[0.5, 0.75, 0.25], [1.5, 1.75, 0.25]])
    );
    assert_eq!(
        rows["mesh"]["alternatives"],
        json!([["14".repeat(16)], ["15".repeat(16)]])
    );
    assert_eq!(rows["mesh"]["rear"], false);
    let mut fender = fields(Class::FendersItemData);
    put(&mut fender, "Location", location(true));
    put(&mut fender, "RimWidth", Value::Float32(0.375));
    put(&mut fender, "OverrideRimWidth", Value::Bool(true));
    let rows = validate(&fender, Json::Null).unwrap();
    assert_eq!(rows["wheels"]["width"], 0.375);
    assert_eq!(rows["wheels"]["enabled"], true);
}

#[test]
fn wraps_use_static_identifiers_and_preserve_all_64_hash_bits() {
    let wrap = object(
        Class::LiveryCustomizationItemData,
        vec![
            ("ByteVaultHash", Value::Int64(-13)),
            ("StaticLiveryIndex", Value::Int32(2)),
            ("ByteVaultDataId", Value::Int32(31)),
            ("LibraryPresetId", Value::Int32(-1)),
        ],
    );
    let rows = validate(&wrap, Json::Null).unwrap();
    assert_eq!(rows["appearance"]["data_id"], "18446744073709551603");
    assert_eq!(rows["appearance"]["version"], 2);
    assert_eq!(rows["mesh"]["kind"], "ignore");
    for (class, names, values) in [
        (
            Class::LicensePlateBackgroundItemData,
            vec!["DiffuseMapIndex"],
            vec![-1],
        ),
        (
            Class::LicensePlateFrameItemData,
            vec!["DiffuseMapIndex", "NormalMapIndex"],
            vec![2, 254],
        ),
        (
            Class::StaticLiveryCustomizationItemData,
            vec!["DiffuseTextureIndex", "HighlightTextureIndex"],
            vec![8, 19],
        ),
    ] {
        let obj = object(
            class,
            names
                .into_iter()
                .zip(values.iter().map(|v| Value::Int32(*v)))
                .collect(),
        );
        assert_eq!(
            validate(&obj, Json::Null).unwrap()["appearance"]["indices"],
            json!(values)
        );
    }
    let mut unknown = wrap;
    unknown.class = "UnknownItemData".into();
    assert!(super::rows(&unknown, || Ok(Json::Null)).is_err());
}
