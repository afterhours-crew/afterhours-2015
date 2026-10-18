// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Catalog, FORMAT, ItemCatalog, MAX_BYTES, VERSION};
use crate::{Error, items::ITEM_SYSTEM, scan::Assets};
use nfs_frostbite::ebx::{Fields, Object, Pointer, Value};
use nfs_world::items::{DefinitionClass as Class, MAX_DEFINITIONS};
use nfs_world::replication::vehicle::customization::{Family, family};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

fn bad(field: &str) -> Error {
    Error::Content(format!("vehicle parts: invalid {field}"))
}
fn get<'a>(fields: &'a Fields, name: &str) -> Result<&'a Value, Error> {
    fields.get(name).ok_or_else(|| bad(name))
}
fn integer(fields: &Fields, name: &str) -> Result<i64, Error> {
    match get(fields, name)? {
        Value::Int32(v) => Ok(i64::from(*v)),
        Value::UInt32(v) => Ok(i64::from(*v)),
        _ => Err(bad(name)),
    }
}
fn flag(fields: &Fields, name: &str) -> Result<bool, Error> {
    get(fields, name)?.as_bool().ok_or_else(|| bad(name))
}
fn float_value(value: &Value) -> Result<f32, Error> {
    match value {
        Value::Float32(v) if v.is_finite() => Ok(*v),
        _ => Err(bad("float")),
    }
}
fn float(fields: &Fields, name: &str) -> Result<f32, Error> {
    float_value(get(fields, name)?)
}
fn list<'a>(fields: &'a Fields, name: &str) -> Result<&'a [Value], Error> {
    get(fields, name)?
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or_else(|| bad(name))
}
fn floats(fields: &Fields, name: &str) -> Result<Vec<f32>, Error> {
    list(fields, name)?.iter().map(float_value).collect()
}
fn guids(fields: &Fields, name: &str) -> Result<Vec<String>, Error> {
    list(fields, name)?
        .iter()
        .map(|v| match v {
            Value::Guid(v) => Ok(nfs_frostbite::hex(v)),
            _ => Err(bad(name)),
        })
        .collect()
}
fn rear(fields: &Fields) -> Result<bool, Error> {
    match get(fields, "Location")? {
        Value::Enum { value: 0, .. } => Ok(false),
        Value::Enum { value: 1, .. } => Ok(true),
        _ => Err(bad("Location")),
    }
}
fn pointer(fields: &Fields, name: &str) -> Result<Pointer, Error> {
    get(fields, name)?.as_pointer().ok_or_else(|| bad(name))
}
fn present(fields: &Fields, name: &str) -> Result<bool, Error> {
    Ok(pointer(fields, name)? != Pointer::Null)
}
fn tuning_field(class: Class) -> Option<&'static str> {
    use Class::*;
    Some(match class {
        BrakesItemData => "BrakeTuning",
        ControlArmTuningItemData => "ControlArmTuning",
        DifferentialTuningItemData => "DifferentialTuning",
        GearboxTuningItemData => "GearboxTuning",
        HandbrakeTuningItemData => "HandbrakeTuning",
        NosTuningItemData => "NosTuning",
        PersistantTuningItemData => "PersistantTuning",
        SteeringTuningItemData => "SteeringTuning",
        SuspensionTuningItemData => "SuspensionTuning",
        SwaybarTuningItemData => "SwaybarTuning",
        TireCompositionItemData => "TireTuning",
        SpoilerItemData => "SpoilerTuning",
        _ => return None,
    })
}

fn transmission(assets: &mut Assets, source: &str, fields: &Fields) -> Result<Json, Error> {
    let (name, index) = assets
        .target(source, pointer(fields, "PersistantTuning")?)?
        .ok_or_else(|| bad("persistent tuning target"))?;
    let object = &assets.objects(&name)?[index];
    if object.class != "PersistantTuningConfigData" {
        return Err(bad("persistent tuning class"));
    }
    let fields = get(&object.fields, "ManualTransmissionTuning")?
        .as_fields()
        .ok_or_else(|| bad("manual transmission struct"))?;
    Ok(json!([
        integer(fields, "SliderRangeMax")?,
        integer(fields, "SliderRangeMin")?
    ]))
}

/// Separate from traversal so class-specific fields can be tested without game data.
fn rows(
    object: &Object,
    mut transmission: impl FnMut() -> Result<Json, Error>,
) -> Result<BTreeMap<&'static str, Json>, Error> {
    let class = Class::from_name(&object.class).ok_or_else(|| bad("item class"))?;
    let f = &object.fields;
    let id = object.guid.ok_or_else(|| bad("unexported item"))?;
    let base = json!({"guid":nfs_frostbite::hex(&id),"class_name":object.class});
    let mut tuning = json!({});
    let mut tuned = false;
    if let Some(name) = tuning_field(class) {
        tuned = present(f, name)?;
        tuning["asset_present"] = json!(tuned);
        if class == Class::PersistantTuningItemData && tuned {
            tuning["transmission_values"] = transmission()?;
        }
    }
    let mut chassis = match family(class) {
        Family::Other => json!({"family":"other"}),
        Family::Performance => {
            json!({"family":"performance","performance_value":float(f,"Percent")?})
        }
        Family::Appearance => json!({"family":"customization","flag":flag(f,"WideCollision")?}),
    };
    if tuned {
        chassis["list_a"] = json!(integer(f, "GlobalAssetListIndex")?);
    }
    if (family(class) == Family::Appearance || class == Class::ClutchItemData)
        && present(f, "PerformanceModifier")?
    {
        chassis["list_b"] = json!(integer(f, "PerformanceModifierIndex")?);
    }
    if class == Class::ForcedInductionItemData {
        chassis["index"] = json!(integer(f, "ForcedInductionUpgradeIndex")?);
    }
    if class == Class::RimsItemData {
        chassis["rim_values"] = json!(floats(f, "SizeOptions")?);
        chassis["rear"] = json!(rear(f)?);
    }
    let mut custom = base.clone();
    custom["tuning"] = tuning;
    custom["chassis"] = chassis;
    let mut out = BTreeMap::from([("customization", custom)]);
    if class == Class::NosTuningItemData {
        out.insert(
            "nos",
            json!({"guid":base["guid"],"variant":integer(f,"NOSUpgradeIndex")?}),
        );
    }
    let tire_infos = if class == Class::TiresItemData {
        list(f, "TireMeshInfos")?
            .iter()
            .map(|v| v.as_fields().ok_or_else(|| bad("tire struct")))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let wheel = match class {
        Class::RimsItemData => Some(
            json!({"diameter":float(f,"Diameter")?,"width":float(f,"Width")?,
            "scale":float(f,"BrakeDiscRatio")?,"sizes":floats(f,"SizeOptions")?,"flag":flag(f,"AllowWidthOverride")?}),
        ),
        Class::TiresItemData => Some(json!({"alternatives":tire_infos.iter().map(|f| Ok([
            float(f,"InnerDiameter")?,float(f,"OuterDiameter")?,float(f,"Width")?
        ])).collect::<Result<Vec<_>,Error>>()?})),
        Class::FendersItemData => {
            Some(json!({"width":float(f,"RimWidth")?,"enabled":flag(f,"OverrideRimWidth")?}))
        }
        _ => None,
    };
    if let Some(mut row) = wheel {
        row["rear"] = json!(rear(f)?);
        row.as_object_mut()
            .expect("object")
            .extend(base.as_object().expect("object").clone());
        out.insert("wheels", row);
    }
    let appearance = match class {
        Class::RimsItemData => Some(json!({"rear":rear(f)?})),
        Class::LicensePlateBackgroundItemData => {
            Some(json!({"indices":[integer(f,"DiffuseMapIndex")?]}))
        }
        Class::LicensePlateFrameItemData => {
            Some(json!({"indices":[integer(f,"DiffuseMapIndex")?,integer(f,"NormalMapIndex")?]}))
        }
        Class::StaticLiveryCustomizationItemData => Some(
            json!({"indices":[integer(f,"DiffuseTextureIndex")?,integer(f,"HighlightTextureIndex")?]}),
        ),
        Class::LiveryCustomizationItemData => {
            // The signed asset field carries an opaque 64-bit identifier.
            let hash = match get(f, "ByteVaultHash")? {
                Value::Int64(v) => *v as u64,
                _ => return Err(bad("ByteVaultHash")),
            };
            Some(json!({"data_id":hash.to_string(),"version":integer(f,"StaticLiveryIndex")?}))
        }
        _ => None,
    };
    if let Some(mut row) = appearance {
        row.as_object_mut()
            .expect("object")
            .extend(base.as_object().expect("object").clone());
        out.insert("appearance", row);
    }
    let mut mesh = base;
    if class == Class::TiresItemData {
        mesh["kind"] = json!("tire");
        mesh["rear"] = json!(rear(f)?);
        mesh["alternatives"] = json!(
            tire_infos
                .iter()
                .map(|f| guids(f, "TireMeshGuids"))
                .collect::<Result<Vec<_>, _>>()?
        );
    } else if class == Class::LightsItemData {
        mesh["kind"] = json!("light");
        mesh["guids"] = json!(guids(f, "MeshGuids")?);
        mesh["rear"] = json!(rear(f)?);
        mesh["extent"] = json!(float(f, "AnimationExtent")?);
    } else if class == Class::SpoilerItemData {
        mesh["kind"] = json!("spoiler");
        mesh["guids"] = json!(guids(f, "MeshGuids")?);
        mesh["extent"] = json!(float(f, "AnimationExtent")?);
        mesh["selector"] = json!(integer(f, "AnimationId")?);
    } else if family(class) != Family::Other {
        mesh["kind"] = json!("part");
        mesh["guids"] = json!(guids(f, "MeshGuids")?);
        mesh["rim"] = if class == Class::RimsItemData {
            json!([
                float(f, "BrakeDiscXOffset")?,
                float(f, "Diameter")?,
                float(f, "LipSize")?
            ])
        } else {
            Json::Null
        };
    } else {
        mesh["kind"] = json!("ignore");
    }
    out.insert("mesh", mesh);
    Ok(out)
}

pub(crate) fn build(assets: &mut Assets, classes: &ItemCatalog) -> Result<Vec<u8>, Error> {
    let system = assets
        .objects(ITEM_SYSTEM)?
        .first()
        .filter(|o| o.class == "ItemSystemData")
        .ok_or_else(|| bad("item system"))?
        .clone();
    let items = get(&system.fields, "Items")?
        .as_array()
        .filter(|v| v.len() <= MAX_DEFINITIONS)
        .ok_or_else(|| bad("item list"))?;
    let mut definitions = BTreeMap::new();
    for item in items {
        let (source, object) = assets.resolve(item.as_pointer())?;
        let id = object.guid.ok_or_else(|| bad("item GUID"))?;
        let generated = rows(&object, || transmission(assets, &source, &object.fields))?;
        if definitions.insert(id, generated).is_some() {
            return Err(bad("duplicate item"));
        }
    }
    let mut parts = json!({"customization":[],"nos":[],"wheels":[],"appearance":[],"mesh":[]});
    for rows in definitions.into_values() {
        for (section, row) in rows {
            parts[section].as_array_mut().expect("array").push(row);
        }
    }
    let value = json!({"format":FORMAT,"version":VERSION,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"parts":parts});
    Catalog::from_json(&value, classes)
        .map_err(|e| Error::Content(format!("generated vehicle parts rejected: {e:?}")))?;
    let bytes = serde_json::to_vec(&value).map_err(|_| bad("JSON encoding"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(bad("output size"));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
