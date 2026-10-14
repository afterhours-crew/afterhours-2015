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
use nfs_services::item_content::{FORMAT, ItemContent, PURCHASE_VERSION};
use nfs_world_core::items::{DefinitionClass, Layout};
use serde_json::{Value as Json, json};

pub const ITEM_SYSTEM: &str = "items/gameitemsystem";
const PLATE_TEXT_BYTES: usize = 8;

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

/// Initial purchase scope: unrestricted leaf air filters available without REP
/// progression. Unknown requirements never become an implicit unlocked offer.
fn initial_offer(object: &Object) -> Result<bool, Error> {
    if object.class != "AirFilterItemData" {
        return Ok(false);
    }
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
    Ok(flag("Purchasable")?
        && !flag("Deprecated")?
        && flag("RequiresOwner")?
        && !flag("UsesScope")?
        && word(f, &object.class, "Quantity")? == 1
        && word(f, &object.class, "UnlocksAtRepLevel")? == 0
        && f.get("UnlockCriteriaDescriptionID")
            .and_then(Value::as_str)
            .ok_or_else(|| missing(&object.class, "UnlockCriteriaDescriptionID"))?
            .is_empty()
        && empty("SortedScope")?
        && empty("SortedRestrictions")?
        && empty("SubItems")?
        && empty("AdditionalItems")?)
}

/// Build the canonical `nfs-item-definitions` version 3 document.
pub fn build(assets: &mut Assets) -> Result<Vec<u8>, Error> {
    let system = assets
        .objects(ITEM_SYSTEM)?
        .first()
        .cloned()
        .filter(|o| o.class == "ItemSystemData")
        .ok_or_else(|| Error::Content(format!("{ITEM_SYSTEM} is not an item system")))?;
    let mut definitions = Vec::new();
    let mut purchase_offers = Vec::new();
    for value in system
        .fields
        .get("Items")
        .and_then(Value::as_array)
        .ok_or_else(|| missing("ItemSystemData", "Items"))?
    {
        let (_, object) = assets.resolve(value.as_pointer())?;
        if initial_offer(&object)? {
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
        "version": PURCHASE_VERSION,
        "build_sha256": nfs_services::SUPPORTED_BUILD_SHA256,
        "definitions": definitions,
        "initial_definitions": initial,
        "purchase_offers": purchase_offers,
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
    fn only_explicit_unrestricted_entry_parts_become_offers() {
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
        for (key, value) in [
            ("UnlocksAtRepLevel", Value::UInt32(2)),
            ("Purchasable", Value::Bool(false)),
            ("UsesScope", Value::Bool(true)),
            ("Deprecated", Value::Bool(true)),
            (
                "UnlockCriteriaDescriptionID",
                Value::String("special requirement".into()),
            ),
            ("SortedRestrictions", Value::Array(vec![Value::UInt32(1)])),
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
        let mut missing = original;
        missing
            .fields
            .0
            .retain(|(name, _)| name != "UnlocksAtRepLevel");
        assert!(initial_offer(&missing).is_err());
    }
}
