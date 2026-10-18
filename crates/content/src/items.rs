// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Item definitions from the installed item system.
//!
//! `Items/GameItemSystem` lists every definition (`Items`) and the starting
//! inventory (`Inventory`) as import references. Each definition is the
//! exported `*ItemData` instance of its own partition; its instance GUID is the
//! definition GUID. Class-specific default fields become the derived default
//! of the item wire record: each 32-bit word is the stored bit pattern of the
//! same-named field (floats as their bits), colour/material vectors are three
//! words and licence-plate text is eight NUL-padded bytes.
use crate::Error;
use crate::scan::Assets;
use nfs_frostbite::ebx::{Fields, Object, Value};
use nfs_services::item_content::{CAR_OWNERSHIP_VERSION, FORMAT, ItemContent};
use nfs_world_core::items::{DefinitionClass, Layout};
use serde_json::{Value as Json, json};

pub const ITEM_SYSTEM: &str = "items/gameitemsystem";
const PLATE_TEXT_BYTES: usize = 8;
mod unlocks;

enum Slot {
    Word(&'static str),
    Vector(&'static str),
    Text(&'static str),
}
use Slot::{Text, Vector, Word};

/// Default fields of each derived layout, in wire order. `None`: the layout has
/// no static default (timed discounts carry instance dates).
fn slots(layout: Layout) -> Option<&'static [Slot]> {
    Some(match layout {
        Layout::Empty => &[],
        Layout::RaceVehicle => &[
            Vector("DefaultPaint"),
            Vector("Material"),
            Word("WindowTint"),
            Text("DefaultLicensePlateText"),
            Word("MaxHP"),
            Word("MaxTorque"),
            Word("ZeroToSixty"),
            Word("ZeroToOneHundred"),
            Word("TopSpeedMPH"),
            Word("QuarterMileMPH"),
            Word("QuarterMileTime"),
            Word("VehicleItemFlags"),
        ],
        Layout::Spoiler => &[Word("DownforceSetting")],
        Layout::CategoryUnlockController => &[Word("UnlockMask")],
        Layout::LiveryCustomization => &[
            Word("ByteVaultDataId"),
            Word("LibraryPresetId"),
            Word("DynamicFlags"),
        ],
        Layout::NosTuning => &[Word("NosSetting")],
        Layout::PersistantTuning => &[
            Word("ABSSetting"),
            Word("StabilityControlSetting"),
            Word("TractionControlSetting"),
            Word("AirPressureFrontSetting"),
            Word("AirPressureRearSetting"),
        ],
        Layout::SteeringTuning => &[Word("SteerRateSetting"), Word("SteerRangeSetting")],
        Layout::SuspensionTuning => &[
            Word("SpringStiffnessFrontSetting"),
            Word("SpringStiffnessRearSetting"),
            Word("DampingFrontSetting"),
            Word("DampingRearSetting"),
        ],
        Layout::SwaybarTuning => &[
            Word("AntiRollBarsFrontSetting"),
            Word("AntiRollBarsRearSetting"),
        ],
        Layout::TireComposition => &[Word("TireSetting")],
        Layout::BrakeDiscs => &[Word("BrakeStrengthSetting"), Word("BrakeBiasSetting")],
        Layout::Discount => &[Word("DiscountPercent")],
        Layout::Rims => &[
            Word("RimSelection"),
            Vector("PrimaryPaint"),
            Vector("PrimaryMaterial"),
            Vector("SecondaryPaint"),
            Vector("SecondaryMaterial"),
        ],
        Layout::TimedDiscount => return None,
        Layout::ControlArmTuning => &[
            Word("CasterSetting"),
            Word("CamberFrontSetting"),
            Word("CamberRearSetting"),
            Word("RideHeightHeightSetting"),
            Word("RideHeightRakeSetting"),
            Word("ToeFrontSetting"),
            Word("ToeRearSetting"),
            Word("TrackWidthFrontSetting"),
            Word("TrackWidthRearSetting"),
        ],
        Layout::DifferentialTuning => &[Word("DifferentialSetting")],
        Layout::GearboxTuning => &[Word("GearboxSetting")],
        Layout::HandbrakeTuning => &[Word("HandbrakeStrengthSetting")],
    })
}

fn missing(class: &str, field: &str) -> Error {
    Error::Content(format!(
        "{class}.{field} is missing or has an unexpected type"
    ))
}

fn word(fields: &Fields, class: &str, name: &str) -> Result<u32, Error> {
    fields
        .get(name)
        .and_then(Value::bits32)
        .ok_or_else(|| missing(class, name))
}

fn derived_default(class: &str, layout: Layout, fields: &Fields) -> Result<Option<String>, Error> {
    let Some(slots) = slots(layout) else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    for slot in slots {
        match slot {
            Word(name) => bytes.extend(word(fields, class, name)?.to_le_bytes()),
            Vector(name) => {
                let vector = fields
                    .get(name)
                    .and_then(Value::as_fields)
                    .ok_or_else(|| missing(class, name))?;
                for axis in ["x", "y", "z"] {
                    bytes.extend(word(vector, class, axis)?.to_le_bytes());
                }
            }
            Text(name) => {
                let text = fields
                    .get(name)
                    .and_then(Value::as_str)
                    .ok_or_else(|| missing(class, name))?;
                if text.len() > PLATE_TEXT_BYTES {
                    return Err(Error::Content(format!(
                        "{class}.{name} exceeds eight bytes"
                    )));
                }
                let mut fixed = [0u8; PLATE_TEXT_BYTES];
                fixed[..text.len()].copy_from_slice(text.as_bytes());
                bytes.extend(fixed);
            }
        }
    }
    Ok(Some(nfs_frostbite::hex(&bytes)))
}

fn references(assets: &mut Assets, object: &Object, field: &str) -> Result<Vec<Json>, Error> {
    let list = object
        .fields
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| missing(&object.class, field))?
        .to_vec();
    list.iter()
        .map(|value| {
            let (_, target) = assets.resolve(value.as_pointer())?;
            let guid = target
                .guid
                .ok_or_else(|| Error::Content("reference to an unexported instance".into()))?;
            Ok(json!(nfs_frostbite::hex(&guid)))
        })
        .collect()
}

fn definition(assets: &mut Assets, object: &Object) -> Result<Json, Error> {
    let name = object.class.as_str();
    let class = DefinitionClass::from_name(name)
        .ok_or_else(|| Error::Content(format!("unsupported item class {name}")))?;
    let f = &object.fields;
    let flag = |field: &str| {
        f.get(field)
            .and_then(Value::as_bool)
            .ok_or_else(|| missing(name, field))
    };
    let ownership = match f
        .get("InventoryOwnershipLevel")
        .and_then(Value::as_enum_name)
    {
        Some("InventoryOwnershipLevelType_Claimable") => 0,
        Some("InventoryOwnershipLevelType_Owned") => 1,
        Some("InventoryOwnershipLevelType_Purchasable") => 2,
        _ => return Err(missing(name, "InventoryOwnershipLevel")),
    };
    let guid = object
        .guid
        .ok_or_else(|| Error::Content("item definition is not exported".into()))?;
    Ok(json!({
        "guid": nfs_frostbite::hex(&guid),
        "class": name,
        "buy_price": word(f, name, "BuyPrice")?,
        "sell_price": word(f, name, "SellPrice")?,
        "quantity": word(f, name, "Quantity")?,
        "ownership_level": ownership,
        "flags": {
            "deprecated": flag("Deprecated")?,
            "requires_owner": flag("RequiresOwner")?,
            "uses_scope": flag("UsesScope")?,
            "purchasable": flag("Purchasable")?,
            "optional": flag("Optional")?,
        },
        "sub_items": references(assets, object, "SubItems")?,
        "additional_items": references(assets, object, "AdditionalItems")?,
        "default_derived_hex": derived_default(name, class.layout(), f)?,
    }))
}

/// Supported leaf performance/visual parts. REP-only requirements are resolved
/// separately; other unlock conditions never become implicit unlocked offers.
fn initial_offer(object: &Object) -> Result<bool, Error> {
    part_offer(object, false)
}
fn car_offer(object: &Object) -> Result<bool, Error> {
    if object.class != "RaceVehicleItemData" {
        return Ok(false);
    }
    let f = &object.fields;
    let flag = |name: &str| {
        f.get(name)
            .and_then(Value::as_bool)
            .ok_or_else(|| missing(&object.class, name))
    };
    Ok(flag("Purchasable")?
        && !flag("Deprecated")?
        && !flag("RequiresOwner")?
        && flag("UsesScope")?
        && matches!(word(f, &object.class, "Quantity")?, 1 | u32::MAX)
        && matches!(
            f.get("UnlockCriteriaDescriptionID")
                .and_then(Value::as_str)
                .ok_or_else(|| missing(&object.class, "UnlockCriteriaDescriptionID"))?,
            "" | "ID_UNLOCKED_AT_REP_LEVEL"
        ))
}
fn part_offer(object: &Object, granted: bool) -> Result<bool, Error> {
    if !DefinitionClass::from_name(&object.class).is_some_and(nfs_services::purchase::part_class) {
        return Ok(false);
    }
    offer_flags(object, granted, false)
}

fn offer_flags(object: &Object, granted: bool, composite: bool) -> Result<bool, Error> {
    let f = &object.fields;
    let empty = |name: &str| {
        f.get(name)
            .and_then(Value::as_array)
            .map(|v| v.is_empty())
            .ok_or_else(|| missing(&object.class, name))
    };
    let flag = |name: &str| {
        f.get(name)
            .and_then(Value::as_bool)
            .ok_or_else(|| missing(&object.class, name))
    };
    Ok((flag("Purchasable")? || granted)
        && !flag("Deprecated")?
        && flag("RequiresOwner")?
        && !flag("UsesScope")?
        && word(f, &object.class, "Quantity")? == 1
        && word(f, &object.class, "UnlocksAtRepLevel")?
            <= nfs_services::reputation::MAX_THRESHOLDS as u32
        && matches!(
            f.get("UnlockCriteriaDescriptionID")
                .and_then(Value::as_str)
                .ok_or_else(|| missing(&object.class, "UnlockCriteriaDescriptionID"))?,
            "" | "ID_UNLOCKED_AT_REP_LEVEL"
        )
        // UsesScope is false above: an authored but inactive part scope must
        // not suppress shared wheels. The car's active scope still determines
        // compatibility at purchase and equip.
        && (composite || empty("SubItems")?)
        && empty("AdditionalItems")?)
}

fn body_kit_offer(assets: &mut Assets, object: &Object) -> Result<bool, Error> {
    if object.class != "BodyKitItemData" || !offer_flags(object, false, true)? {
        return Ok(false);
    }
    let children = object
        .fields
        .get("SubItems")
        .and_then(Value::as_array)
        .ok_or_else(|| missing(&object.class, "SubItems"))?;
    if children.is_empty() || children.len() > nfs_services::purchase::MAX_KIT_PARTS {
        return Ok(false);
    }
    let mut slots = std::collections::BTreeSet::new();
    for child in children {
        let (_, part) = assets.resolve(child.as_pointer())?;
        let Some(class) = DefinitionClass::from_name(&part.class) else {
            return Ok(false);
        };
        let definition = definition(assets, &part)?;
        if !nfs_services::purchase::visual_class(class)
            || definition["flags"]["requires_owner"] != true
            || definition["flags"]["deprecated"] != false
            || definition["quantity"] != 1
            || definition["sub_items"] != json!([])
            || definition["additional_items"] != json!([])
            || definition["default_derived_hex"].is_null()
        {
            return Ok(false);
        }
        let location = if nfs_services::purchase::spatial_class(class) {
            match part.fields.get("Location") {
                Some(Value::Enum {
                    value: value @ 0..=1,
                    ..
                }) => *value,
                _ => return Err(missing(&part.class, "Location")),
            }
        } else {
            0
        };
        if !slots.insert((part.class, location)) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn vehicle_scope(object: &Object) -> Result<Option<Json>, Error> {
    if object.class != "RaceVehicleItemData" {
        return Ok(None);
    }
    if object.fields.get("UsesScope").and_then(Value::as_bool) != Some(true) {
        return Err(missing(&object.class, "UsesScope"));
    }
    let parts = object
        .fields
        .get("SortedScope")
        .and_then(Value::as_array)
        .ok_or_else(|| missing(&object.class, "SortedScope"))?;
    let parts = parts
        .iter()
        .map(|value| match value {
            Value::Guid(guid) => Ok(nfs_frostbite::hex(guid)),
            _ => Err(missing(&object.class, "SortedScope GUID")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let guid = object
        .guid
        .ok_or_else(|| missing(&object.class, "exported GUID"))?;
    Ok(Some(
        json!({"vehicle":nfs_frostbite::hex(&guid),"definitions":parts}),
    ))
}

fn fitment_restrictions(object: &Object) -> Result<Option<Json>, Error> {
    let conflicts = object
        .fields
        .get("SortedRestrictions")
        .and_then(Value::as_array)
        .ok_or_else(|| missing(&object.class, "SortedRestrictions"))?;
    let conflicts = conflicts
        .iter()
        .map(|v| match v {
            Value::Guid(guid) => Ok(nfs_frostbite::hex(guid)),
            _ => Err(missing(&object.class, "SortedRestrictions GUID")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if conflicts.is_empty() {
        return Ok(None);
    }
    let guid = object
        .guid
        .ok_or_else(|| missing(&object.class, "exported GUID"))?;
    Ok(Some(
        json!({"definition":nfs_frostbite::hex(&guid),"conflicts":conflicts}),
    ))
}

/// Build the canonical `nfs-item-definitions` document with explicit kit offers.
pub fn build(assets: &mut Assets) -> Result<Vec<u8>, Error> {
    let thresholds = nfs_services::reputation::Thresholds::from_json(
        &serde_json::from_slice(&crate::reputation::build(assets)?)
            .map_err(|_| Error::Content("REP thresholds encoding".into()))?,
    )
    .map_err(|_| Error::Content("REP thresholds invalid".into()))?;
    let system = assets
        .objects(ITEM_SYSTEM)?
        .first()
        .cloned()
        .filter(|o| o.class == "ItemSystemData")
        .ok_or_else(|| Error::Content(format!("{ITEM_SYSTEM} is not an item system")))?;
    let mut definitions = Vec::new();
    let mut purchase_offers = Vec::new();
    let mut car_offers = Vec::new();
    let mut vehicle_scopes = Vec::new();
    let mut part_locations = Vec::new();
    let mut purchase_requirements = Vec::new();
    let mut restrictions = Vec::new();
    let mut body_kits = Vec::new();
    let item_unlocks = unlocks::items(assets, &thresholds)?;
    for value in system
        .fields
        .get("Items")
        .and_then(Value::as_array)
        .ok_or_else(|| missing("ItemSystemData", "Items"))?
    {
        let (_, object) = assets.resolve(value.as_pointer())?;
        if car_offer(&object)? {
            let level = word(&object.fields, &object.class, "UnlocksAtRepLevel")?;
            let score = thresholds
                .score_for_level(level)
                .map_err(|_| Error::Content("car REP level exceeds installed thresholds".into()))?;
            car_offers.push(json!({"definition":nfs_frostbite::hex(&object.guid.ok_or_else(||missing(&object.class,"exported GUID"))?),"minimum_rep_score":score}));
        }
        if DefinitionClass::from_name(&object.class)
            .is_some_and(nfs_services::purchase::spatial_class)
        {
            let location = match object.fields.get("Location") {
                Some(Value::Enum {
                    value: value @ 0..=1,
                    ..
                }) => *value,
                _ => return Err(missing(&object.class, "Location")),
            };
            part_locations.push(json!({"definition":nfs_frostbite::hex(&object.guid.ok_or_else(|| missing(&object.class,"exported GUID"))?),"location":location}));
        }
        if let Some(scope) = vehicle_scope(&object)? {
            vehicle_scopes.push(scope);
        }
        if let Some(row) = fitment_restrictions(&object)? {
            restrictions.push(row);
        }
        let kit = body_kit_offer(assets, &object)?;
        if kit {
            body_kits.push(nfs_frostbite::hex(
                &object
                    .guid
                    .ok_or_else(|| missing(&object.class, "exported GUID"))?,
            ));
        }
        if kit
            || initial_offer(&object)?
            || object.guid.is_some_and(|g| item_unlocks.contains_key(&g))
        {
            let level = word(&object.fields, &object.class, "UnlocksAtRepLevel")?;
            let score = thresholds.score_for_level(level).map_err(|_| {
                Error::Content("part REP level exceeds installed thresholds".into())
            })?;
            purchase_requirements.push(json!({"definition":nfs_frostbite::hex(&object.guid.ok_or_else(|| missing(&object.class,"exported GUID"))?),"minimum_rep_score":score}));
            purchase_offers.push(nfs_frostbite::hex(
                &object
                    .guid
                    .ok_or_else(|| Error::Content("offer definition is not exported".into()))?,
            ));
        }
        definitions.push(definition(assets, &object)?);
    }
    let initial = references(assets, &system, "Inventory")?;
    let document = json!({
        "format": FORMAT,
        "version": CAR_OWNERSHIP_VERSION,
        "build_sha256": nfs_services::SUPPORTED_BUILD_SHA256,
        "definitions": definitions,
        "initial_definitions": initial,
        "purchase_offers": purchase_offers,
        "car_offers": car_offers,
        "body_kits": body_kits,
        "vehicle_scopes": vehicle_scopes,
        "part_locations":part_locations,
        "fitment_restrictions":restrictions,
        "purchase_requirements":purchase_requirements,
        "category_unlocks":unlocks::build(assets, &thresholds)?,
        "rep_rewards":unlocks::rewards(assets, &thresholds)?,
        "item_unlocks": item_unlocks.iter().map(|(g, (_, s))| json!({"definition":nfs_frostbite::hex(g),"minimum_rep_score":s})).collect::<Vec<_>>(),
    });
    // The consuming loader validates classes, layouts, references and bounds;
    // its export is the canonical form written to the cache.
    let content = ItemContent::from_json(&document)
        .map_err(|e| Error::Content(format!("item definitions rejected: {e}")))?;
    serde_json::to_vec(&content.to_json()).map_err(|_| Error::Content("item encoding".into()))
}

#[cfg(test)]
mod offer_tests {
    use super::*;
    #[test]
    fn fitment_conflicts_preserve_installed_guids_and_reject_malformed_arrays() {
        let mut object = Object {
            class: "CanardItemData".into(),
            guid: Some([1; 16]),
            fields: Fields(vec![(
                "SortedRestrictions".into(),
                Value::Array(vec![Value::Guid([2; 16])]),
            )]),
        };
        assert_eq!(
            fitment_restrictions(&object).unwrap(),
            Some(
                json!({"definition":"01010101010101010101010101010101","conflicts":["02020202020202020202020202020202"]})
            )
        );
        object.fields.0[0].1 = Value::Array(vec![Value::UInt32(2)]);
        assert!(fitment_restrictions(&object).is_err());
        object.fields.0[0].1 = Value::Array(vec![]);
        assert_eq!(fitment_restrictions(&object).unwrap(), None);
        object.fields.0.clear();
        assert!(fitment_restrictions(&object).is_err());
    }
    #[test]
    fn vehicle_scope_comes_only_from_explicit_installed_guid_arrays() {
        let mut object = Object {
            class: "RaceVehicleItemData".into(),
            guid: Some([1; 16]),
            fields: Fields(vec![
                ("UsesScope".into(), Value::Bool(true)),
                (
                    "SortedScope".into(),
                    Value::Array(vec![Value::Guid([2; 16])]),
                ),
            ]),
        };
        assert_eq!(
            vehicle_scope(&object).unwrap().unwrap(),
            json!({
            "vehicle":"01010101010101010101010101010101",
            "definitions":["02020202020202020202020202020202"]})
        );
        object.fields.0[0].1 = Value::Bool(false);
        assert!(vehicle_scope(&object).is_err());
        object.fields.0[0].1 = Value::Bool(true);
        object.fields.0[1].1 = Value::Array(vec![Value::UInt32(2)]);
        assert!(vehicle_scope(&object).is_err());
        object.class = "AirFilterItemData".into();
        assert_eq!(vehicle_scope(&object).unwrap(), None);
    }
    #[test]
    fn only_explicit_leaf_parts_with_known_unlocks_become_offers() {
        let mut fields = std::collections::BTreeMap::new();
        for name in ["Purchasable", "RequiresOwner"] {
            fields.insert(name.into(), Value::Bool(true));
        }
        for name in ["Deprecated", "UsesScope"] {
            fields.insert(name.into(), Value::Bool(false));
        }
        for name in [
            "SortedScope",
            "SortedRestrictions",
            "SubItems",
            "AdditionalItems",
        ] {
            fields.insert(name.into(), Value::Array(vec![]));
        }
        fields.insert("Quantity".into(), Value::UInt32(1));
        fields.insert("UnlocksAtRepLevel".into(), Value::UInt32(0));
        fields.insert(
            "UnlockCriteriaDescriptionID".into(),
            Value::String(String::new()),
        );
        let original = Object {
            class: "AirFilterItemData".into(),
            guid: Some([1; 16]),
            fields: Fields(fields.into_iter().collect()),
        };
        assert!(initial_offer(&original).unwrap());
        for class in [
            "BrakesItemData",
            "NosTuningItemData",
            "EcuItemData",
            "SteeringTuningItemData",
            "HoodItemData",
            "RimsItemData",
        ] {
            assert!(
                initial_offer(&Object {
                    class: class.into(),
                    ..original.clone()
                })
                .unwrap()
            );
        }
        for class in [
            "CurrencyItemData",
            "RaceVehicleItemData",
            "BodyKitItemData",
            "LiveryCustomizationItemData",
        ] {
            assert!(
                !initial_offer(&Object {
                    class: class.into(),
                    ..original.clone()
                })
                .unwrap()
            );
        }
        for (key, value) in [
            ("UnlocksAtRepLevel", Value::UInt32(257)),
            ("Purchasable", Value::Bool(false)),
            ("UsesScope", Value::Bool(true)),
            ("Deprecated", Value::Bool(true)),
            (
                "UnlockCriteriaDescriptionID",
                Value::String("special requirement".into()),
            ),
            (
                "SubItems",
                Value::Array(vec![Value::Pointer(nfs_frostbite::ebx::Pointer::Internal(
                    0,
                ))]),
            ),
        ] {
            let mut changed = original.clone();
            changed
                .fields
                .0
                .iter_mut()
                .find(|(name, _)| name == key)
                .unwrap()
                .1 = value;
            assert!(!initial_offer(&changed).unwrap(), "accepted {key}");
        }
        let mut restricted = original.clone();
        for (name, value) in &mut restricted.fields.0 {
            if name == "UnlocksAtRepLevel" {
                *value = Value::UInt32(2);
            }
            if name == "UnlockCriteriaDescriptionID" {
                *value = Value::String("ID_UNLOCKED_AT_REP_LEVEL".into());
            }
        }
        assert!(initial_offer(&restricted).unwrap());
        // Shared visual parts may retain an inactive scope naming another car.
        // Candidate compatibility comes from the current car's active scope.
        let mut shared = original.clone();
        shared.class = "RimsItemData".into();
        shared
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "SortedScope")
            .unwrap()
            .1 = Value::Array(vec![Value::Guid([9; 16])]);
        assert!(initial_offer(&shared).unwrap());
        shared
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "UsesScope")
            .unwrap()
            .1 = Value::Bool(true);
        assert!(!initial_offer(&shared).unwrap());
        let mut missing = original;
        missing
            .fields
            .0
            .retain(|(name, _)| name != "UnlocksAtRepLevel");
        assert!(initial_offer(&missing).is_err());
    }
}
