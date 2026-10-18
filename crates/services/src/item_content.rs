// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Versioned static item definitions. V1 contains GUID/class bindings; V2 adds
//! asset prices, links, constructor defaults and an explicit initial recipe.
//! V3 adds explicit supported purchase offers; V4 adds vehicle compatibility.
//! V5 adds visual slot locations and installed REP score requirements.
//! V6 adds explicit REP item grants; older formats never infer them.
//! V7 adds installed fitment conflicts for supported leaf selections.
//! V8 explicitly opts in supported body-kit offers and their owned child trees.
//! V9 adds explicit owned REP rewards; older formats grant no such items.
//! No format accepts captured
//! instances, player ids or reply envelopes.
use crate::{ContentError, SUPPORTED_BUILD_SHA256 as BUILD};
use nfs_world_core::items::{
    Catalog, Definition, DefinitionClass, DefinitionFlags, Derived, Guid, InventoryCatalog,
    MAX_COLLECTION, MAX_DEFINITIONS, MAX_WORDS, OwnershipLevel,
};
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::Path};

pub const FORMAT: &str = "nfs-item-definitions";
pub const VERSION: u8 = 1;
pub const INVENTORY_VERSION: u8 = 2;
pub const PURCHASE_VERSION: u8 = 3;
pub const SCOPED_PURCHASE_VERSION: u8 = 4;
pub const PART_POLICY_VERSION: u8 = 5;
pub const ITEM_UNLOCK_VERSION: u8 = 6;
pub const FITMENT_VERSION: u8 = 7;
pub const BODY_KIT_VERSION: u8 = 8;
pub const REP_REWARD_VERSION: u8 = 9;
pub const MAX_STORE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemContent {
    pub catalog: Catalog,
    inventory: Option<InventoryCatalog>,
    purchase_offers: Option<crate::purchase::Offers>,
    category_unlocks: Option<crate::category_unlocks::Policy>,
}

fn fields(value: &Value, names: &[&str]) -> Result<(), ContentError> {
    let object = value.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|name| object.contains_key(*name)) {
        return Err(ContentError::Invalid);
    }
    Ok(())
}
fn guid(value: &Value) -> Result<Guid, ContentError> {
    let bytes = unhex(value, 16)?;
    bytes.try_into().map_err(|_| ContentError::Invalid)
}
fn unhex(value: &Value, bound: usize) -> Result<Vec<u8>, ContentError> {
    let text = value.as_str().ok_or(ContentError::Invalid)?;
    if !text.len().is_multiple_of(2) || text.len() / 2 > bound || !text.is_ascii() {
        return Err(ContentError::Invalid);
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().as_chunks::<2>().0 {
        let nibble = |byte: u8| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(ContentError::Invalid),
        };
        bytes.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn guids(value: &Value) -> Result<Vec<Guid>, ContentError> {
    let rows = value.as_array().ok_or(ContentError::Invalid)?;
    if rows.len() > MAX_COLLECTION {
        return Err(ContentError::TooLarge);
    }
    rows.iter().map(guid).collect()
}
fn word(value: &Value) -> Result<u32, ContentError> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(ContentError::Invalid)
}
fn flag(value: &Value) -> Result<bool, ContentError> {
    value.as_bool().ok_or(ContentError::Invalid)
}
impl ItemContent {
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let file = File::open(path).map_err(|_| ContentError::Io)?;
        let mut bytes = Vec::new();
        file.take(MAX_STORE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    pub fn from_json(value: &Value) -> Result<Self, ContentError> {
        if value["version"].as_u64().is_some_and(|v| {
            (u64::from(INVENTORY_VERSION)..=u64::from(REP_REWARD_VERSION)).contains(&v)
        }) {
            return Self::inventory_from_json(value);
        }
        fields(value, &["format", "version", "build_sha256", "definitions"])?;
        if value["format"] != FORMAT
            || value["version"] != VERSION
            || value["build_sha256"] != BUILD
        {
            return Err(ContentError::Invalid);
        }
        let rows = value["definitions"]
            .as_array()
            .ok_or(ContentError::Invalid)?;
        if rows.is_empty() || rows.len() > MAX_DEFINITIONS {
            return Err(ContentError::TooLarge);
        }
        let definitions = rows
            .iter()
            .map(|row| {
                fields(row, &["guid", "class"])?;
                let class =
                    DefinitionClass::from_name(row["class"].as_str().ok_or(ContentError::Invalid)?)
                        .ok_or(ContentError::Invalid)?;
                Ok((guid(&row["guid"])?, class))
            })
            .collect::<Result<Vec<_>, ContentError>>()?;
        Ok(Self {
            catalog: Catalog::new(definitions).map_err(|_| ContentError::Invalid)?,
            inventory: None,
            purchase_offers: None,
            category_unlocks: None,
        })
    }
    pub fn inventory(&self) -> Option<&InventoryCatalog> {
        self.inventory.as_ref()
    }
    pub fn purchase_offers(&self) -> Option<&crate::purchase::Offers> {
        self.purchase_offers.as_ref()
    }
    pub fn category_unlocks(&self) -> Option<&crate::category_unlocks::Policy> {
        self.category_unlocks.as_ref()
    }
    fn inventory_from_json(value: &Value) -> Result<Self, ContentError> {
        let version = u8::try_from(word(&value["version"])?).map_err(|_| ContentError::Invalid)?;
        let mut expected = vec![
            "format",
            "version",
            "build_sha256",
            "definitions",
            "initial_definitions",
        ];
        if version >= PURCHASE_VERSION {
            expected.push("purchase_offers");
        }
        if version >= SCOPED_PURCHASE_VERSION {
            expected.push("vehicle_scopes");
        }
        if version >= PART_POLICY_VERSION {
            expected.extend([
                "part_locations",
                "purchase_requirements",
                "category_unlocks",
            ]);
        }
        if version >= ITEM_UNLOCK_VERSION {
            expected.push("item_unlocks");
        }
        if version >= FITMENT_VERSION {
            expected.push("fitment_restrictions");
        }
        if version >= BODY_KIT_VERSION {
            expected.push("body_kits");
        }
        if version >= REP_REWARD_VERSION {
            expected.push("rep_rewards");
        }
        fields(value, &expected)?;
        if value["format"] != FORMAT || value["build_sha256"] != BUILD {
            return Err(ContentError::Invalid);
        }
        let rows = value["definitions"]
            .as_array()
            .ok_or(ContentError::Invalid)?;
        if rows.is_empty() || rows.len() > MAX_DEFINITIONS {
            return Err(ContentError::TooLarge);
        }
        let mut definitions = Vec::with_capacity(rows.len());
        for row in rows {
            fields(
                row,
                &[
                    "guid",
                    "class",
                    "buy_price",
                    "sell_price",
                    "quantity",
                    "ownership_level",
                    "flags",
                    "sub_items",
                    "additional_items",
                    "default_derived_hex",
                ],
            )?;
            let class =
                DefinitionClass::from_name(row["class"].as_str().ok_or(ContentError::Invalid)?)
                    .ok_or(ContentError::Invalid)?;
            let ownership_level = match row["ownership_level"].as_u64() {
                Some(0) => OwnershipLevel::Claimable,
                Some(1) => OwnershipLevel::Owned,
                Some(2) => OwnershipLevel::Purchasable,
                _ => return Err(ContentError::Invalid),
            };
            let f = &row["flags"];
            fields(
                f,
                &[
                    "deprecated",
                    "requires_owner",
                    "uses_scope",
                    "purchasable",
                    "optional",
                ],
            )?;
            let flags = DefinitionFlags {
                deprecated: flag(&f["deprecated"])?,
                requires_owner: flag(&f["requires_owner"])?,
                uses_scope: flag(&f["uses_scope"])?,
                purchasable: flag(&f["purchasable"])?,
                optional: flag(&f["optional"])?,
            };
            let default_derived = if row["default_derived_hex"].is_null() {
                None
            } else {
                Some(
                    Derived::decode(
                        class.layout(),
                        &unhex(&row["default_derived_hex"], 60 + MAX_WORDS * 8)?,
                    )
                    .map_err(|_| ContentError::Invalid)?,
                )
            };
            definitions.push((
                guid(&row["guid"])?,
                Definition {
                    class,
                    buy_price: word(&row["buy_price"])?,
                    sell_price: word(&row["sell_price"])?,
                    quantity: word(&row["quantity"])?,
                    ownership_level,
                    flags,
                    sub_items: guids(&row["sub_items"])?,
                    additional_items: guids(&row["additional_items"])?,
                    default_derived,
                },
            ));
        }
        let inventory = InventoryCatalog::new(definitions, guids(&value["initial_definitions"])?)
            .map_err(|_| ContentError::Invalid)?;
        let category_unlocks = if value["category_unlocks"].is_null() {
            None
        } else {
            let v = &value["category_unlocks"];
            fields(v, &["controller", "requirements"])?;
            let rows = v["requirements"]
                .as_array()
                .filter(|r| r.len() <= 32)
                .ok_or(ContentError::Invalid)?;
            let requirements = rows
                .iter()
                .map(|r| {
                    fields(r, &["mask", "minimum_rep_score"])?;
                    Ok((
                        word(&r["mask"])?,
                        i32::try_from(word(&r["minimum_rep_score"])?)
                            .map_err(|_| ContentError::Invalid)?,
                    ))
                })
                .collect::<Result<Vec<_>, ContentError>>()?;
            Some(
                crate::category_unlocks::Policy::new(
                    &inventory,
                    guid(&v["controller"])?,
                    requirements,
                )
                .map_err(|_| ContentError::Invalid)?,
            )
        };
        let mut item_unlocks = if version >= ITEM_UNLOCK_VERSION {
            let rows = value["item_unlocks"]
                .as_array()
                .filter(|r| r.len() <= MAX_COLLECTION)
                .ok_or(ContentError::Invalid)?;
            let rows = rows
                .iter()
                .map(|row| {
                    fields(row, &["definition", "minimum_rep_score"])?;
                    Ok((
                        guid(&row["definition"])?,
                        i32::try_from(word(&row["minimum_rep_score"])?)
                            .map_err(|_| ContentError::Invalid)?,
                    ))
                })
                .collect::<Result<Vec<_>, ContentError>>()?;
            Some(
                crate::item_unlocks::Policy::new(&inventory, rows)
                    .map_err(|_| ContentError::Invalid)?,
            )
        } else {
            None
        };
        if version >= REP_REWARD_VERSION {
            let rows = value["rep_rewards"]
                .as_array()
                .filter(|r| r.len() <= MAX_COLLECTION)
                .ok_or(ContentError::Invalid)?;
            let rows = rows
                .iter()
                .map(|row| {
                    fields(row, &["definition", "minimum_rep_score"])?;
                    Ok((
                        guid(&row["definition"])?,
                        i32::try_from(word(&row["minimum_rep_score"])?)
                            .map_err(|_| ContentError::Invalid)?,
                    ))
                })
                .collect::<Result<Vec<_>, ContentError>>()?;
            item_unlocks = Some(
                item_unlocks
                    .ok_or(ContentError::Invalid)?
                    .with_rewards(&inventory, rows)
                    .map_err(|_| ContentError::Invalid)?,
            );
        }
        let body_kits = if version >= BODY_KIT_VERSION {
            guids(&value["body_kits"])?
        } else {
            vec![]
        };
        let purchase_offers = if version >= PURCHASE_VERSION {
            let mut offers = crate::purchase::Offers::new(guids(&value["purchase_offers"])?)
                .map_err(|_| ContentError::Invalid)?;
            for guid in offers.definitions() {
                let definition = inventory
                    .definition(guid)
                    .map_err(|_| ContentError::Invalid)?;
                if !(crate::purchase::performance_class(definition.class)
                    || (version >= PART_POLICY_VERSION
                        && crate::purchase::visual_class(definition.class))
                    || (definition.class == DefinitionClass::BodyKitItemData
                        && body_kits.contains(guid)))
                    || (!definition.flags.purchasable
                        && !item_unlocks
                            .as_ref()
                            .is_some_and(|p| p.requirements().contains_key(guid)))
                {
                    return Err(ContentError::Invalid);
                }
            }
            if version >= SCOPED_PURCHASE_VERSION {
                let rows = value["vehicle_scopes"]
                    .as_array()
                    .ok_or(ContentError::Invalid)?;
                if rows.len() > 512 {
                    return Err(ContentError::TooLarge);
                }
                let scopes = rows
                    .iter()
                    .map(|row| {
                        fields(row, &["vehicle", "definitions"])?;
                        Ok((guid(&row["vehicle"])?, guids(&row["definitions"])?))
                    })
                    .collect::<Result<Vec<_>, ContentError>>()?;
                offers = offers
                    .with_vehicle_scopes(&inventory, scopes)
                    .map_err(|_| ContentError::Invalid)?;
            }
            if version >= PART_POLICY_VERSION {
                let rows = value["part_locations"]
                    .as_array()
                    .filter(|r| r.len() <= MAX_DEFINITIONS)
                    .ok_or(ContentError::Invalid)?;
                let locations = rows
                    .iter()
                    .map(|row| {
                        fields(row, &["definition", "location"])?;
                        Ok((
                            guid(&row["definition"])?,
                            u8::try_from(word(&row["location"])?)
                                .map_err(|_| ContentError::Invalid)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, ContentError>>()?;
                let rows = value["purchase_requirements"]
                    .as_array()
                    .filter(|r| r.len() <= MAX_COLLECTION)
                    .ok_or(ContentError::Invalid)?;
                let requirements = rows
                    .iter()
                    .map(|row| {
                        fields(row, &["definition", "minimum_rep_score"])?;
                        Ok((
                            guid(&row["definition"])?,
                            i32::try_from(word(&row["minimum_rep_score"])?)
                                .map_err(|_| ContentError::Invalid)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, ContentError>>()?;
                offers = offers
                    .with_part_policy(&inventory, locations, requirements)
                    .map_err(|_| ContentError::Invalid)?;
            }
            if let Some(policy) = item_unlocks {
                offers = offers
                    .with_item_unlocks(policy)
                    .map_err(|_| ContentError::Invalid)?;
            }
            if version >= FITMENT_VERSION {
                let rows = value["fitment_restrictions"]
                    .as_array()
                    .filter(|r| r.len() <= MAX_DEFINITIONS)
                    .ok_or(ContentError::Invalid)?;
                let restrictions = rows
                    .iter()
                    .map(|row| {
                        fields(row, &["definition", "conflicts"])?;
                        Ok((guid(&row["definition"])?, guids(&row["conflicts"])?))
                    })
                    .collect::<Result<Vec<_>, ContentError>>()?;
                offers = offers
                    .with_fitment_restrictions(&inventory, restrictions)
                    .map_err(|_| ContentError::Invalid)?;
            }
            if version >= BODY_KIT_VERSION {
                offers = offers
                    .with_body_kits(&inventory, body_kits)
                    .map_err(|_| ContentError::Invalid)?;
            }
            Some(offers)
        } else {
            None
        };
        Ok(Self {
            catalog: inventory.bindings().clone(),
            inventory: Some(inventory),
            purchase_offers,
            category_unlocks,
        })
    }
    pub fn to_json(&self) -> Value {
        if let Some(inventory) = &self.inventory {
            let definitions=inventory.definitions().map(|(guid,d)|{
                let flags=&d.flags;
                let level=match d.ownership_level { OwnershipLevel::Claimable=>0,OwnershipLevel::Owned=>1,OwnershipLevel::Purchasable=>2 };
                // Construction validated the derived value; this branch only
                // exports its typed static defaults, never a captured record.
                let derived=d.default_derived.as_ref().map(|value|hex(&value.encode().expect("validated catalog default")));
                json!({"guid":hex(guid),"class":d.class.name(),"buy_price":d.buy_price,"sell_price":d.sell_price,
                    "quantity":d.quantity,"ownership_level":level,"flags":{"deprecated":flags.deprecated,"requires_owner":flags.requires_owner,
                    "uses_scope":flags.uses_scope,"purchasable":flags.purchasable,"optional":flags.optional},
                    "sub_items":d.sub_items.iter().map(|g|hex(g)).collect::<Vec<_>>(),
                    "additional_items":d.additional_items.iter().map(|g|hex(g)).collect::<Vec<_>>(),"default_derived_hex":derived})
            }).collect::<Vec<_>>();
            let mut document = json!({"format":FORMAT,"version":INVENTORY_VERSION,"build_sha256":BUILD,"definitions":definitions,
                "initial_definitions":inventory.initial_definitions().iter().map(|g|hex(g)).collect::<Vec<_>>()});
            if let Some(offers) = &self.purchase_offers {
                document["version"] = json!(PURCHASE_VERSION);
                document["purchase_offers"] =
                    json!(offers.definitions().map(|g| hex(g)).collect::<Vec<_>>());
                if let Some(scopes) = offers.vehicle_scopes() {
                    document["version"] = json!(SCOPED_PURCHASE_VERSION);
                    document["vehicle_scopes"] = json!(scopes.iter().map(|(vehicle, parts)| {
                        json!({"vehicle":hex(vehicle),"definitions":parts.iter().map(|g|hex(g)).collect::<Vec<_>>()})
                    }).collect::<Vec<_>>());
                }
                if let Some(policy) = offers.part_policy() {
                    document["version"] = json!(PART_POLICY_VERSION);
                    document["category_unlocks"] = self.category_unlocks.as_ref().map(|p| json!({"controller":hex(&p.controller),"requirements":p.requirements.iter().map(|(m,s)|json!({"mask":m,"minimum_rep_score":s})).collect::<Vec<_>>() })).unwrap_or(Value::Null);
                    document["part_locations"] = json!(
                        policy
                            .locations
                            .iter()
                            .map(|(g, l)| json!({"definition":hex(g),"location":l}))
                            .collect::<Vec<_>>()
                    );
                    document["purchase_requirements"] = json!(
                        policy
                            .minimum_rep_scores
                            .iter()
                            .map(|(g, s)| json!({"definition":hex(g),"minimum_rep_score":s}))
                            .collect::<Vec<_>>()
                    );
                }
                if let Some(unlocks) = offers.item_unlocks() {
                    document["version"] = json!(ITEM_UNLOCK_VERSION);
                    document["item_unlocks"] = json!(
                        unlocks
                            .requirements()
                            .iter()
                            .map(|(g, s)| json!({"definition":hex(g),"minimum_rep_score":s}))
                            .collect::<Vec<_>>()
                    );
                }
                if let Some(restrictions) = offers.fitment_restrictions() {
                    document["version"] = json!(FITMENT_VERSION);
                    if document.get("item_unlocks").is_none() {
                        document["item_unlocks"] = json!([]);
                    }
                    document["fitment_restrictions"] = json!(restrictions.iter().map(|(g, conflicts)| {
                        json!({"definition":hex(g),"conflicts":conflicts.iter().map(|g|hex(g)).collect::<Vec<_>>()})
                    }).collect::<Vec<_>>());
                }
                if !offers.body_kits().is_empty() {
                    document["version"] = json!(BODY_KIT_VERSION);
                    document["body_kits"] = json!(
                        offers
                            .body_kits()
                            .keys()
                            .map(|g| hex(g))
                            .collect::<Vec<_>>()
                    );
                }
                if let Some(rewards) = offers.item_unlocks().and_then(|p| p.rewards()) {
                    document["version"] = json!(REP_REWARD_VERSION);
                    if document.get("body_kits").is_none() {
                        document["body_kits"] = json!([]);
                    }
                    document["rep_rewards"] = json!(
                        rewards
                            .iter()
                            .map(|(g, s)| json!({"definition":hex(g),"minimum_rep_score":s}))
                            .collect::<Vec<_>>()
                    );
                }
            }
            return document;
        }
        let definitions: Vec<_> = self
            .catalog
            .definitions()
            .map(|(guid, class)| {
                let hex: String = guid.iter().map(|byte| format!("{byte:02x}")).collect();
                json!({"guid":hex,"class":class.name()})
            })
            .collect();
        json!({"format":FORMAT,"version":VERSION,"build_sha256":BUILD,"definitions":definitions})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> Value {
        json!({"format":FORMAT,"version":VERSION,"build_sha256":BUILD,
        "definitions":[{"guid":"11111111111111111111111111111111","class":"CurrencyItemData"}]})
    }
    #[test]
    fn strict_static_format_round_trips_without_profile_state() {
        let value = document();
        let content = ItemContent::from_json(&value).unwrap();
        assert_eq!(content.catalog.len(), 1);
        assert_eq!(content.to_json(), value);
        for field in ["items", "owner", "buy_price", "body_hex"] {
            let mut value = document();
            value[field] = json!([]);
            assert!(ItemContent::from_json(&value).is_err());
        }
        for (field, bad) in [
            ("version", json!(2)),
            ("build_sha256", json!("wrong")),
            ("format", json!("capture")),
        ] {
            let mut value = document();
            value[field] = bad;
            assert!(ItemContent::from_json(&value).is_err());
        }
    }
    #[test]
    fn rejects_unknown_classes_duplicate_guids_and_malformed_guid_encodings() {
        for bad in ["UnknownItemData", ""] {
            let mut value = document();
            value["definitions"][0]["class"] = json!(bad);
            assert!(ItemContent::from_json(&value).is_err());
        }
        for bad in [
            "11",
            "z1111111111111111111111111111111",
            "é111111111111111111111111111111",
        ] {
            let mut value = document();
            value["definitions"][0]["guid"] = json!(bad);
            assert!(ItemContent::from_json(&value).is_err());
        }
        let mut value = document();
        let row = value["definitions"][0].clone();
        value["definitions"].as_array_mut().unwrap().push(row);
        assert!(ItemContent::from_json(&value).is_err());
        let mut value = document();
        value["definitions"][0]["owner"] = json!(1);
        assert!(ItemContent::from_json(&value).is_err());
    }
    fn inventory_document() -> Value {
        let mut value = document();
        value["version"] = json!(INVENTORY_VERSION);
        value["initial_definitions"] = json!(["11111111111111111111111111111111"]);
        let row = &mut value["definitions"][0];
        row["buy_price"] = json!(123);
        row["sell_price"] = json!(45);
        row["quantity"] = json!(u32::MAX);
        row["ownership_level"] = json!(0);
        row["flags"] = json!({"deprecated":false,"requires_owner":false,"uses_scope":false,"purchasable":true,"optional":false});
        row["sub_items"] = json!([]);
        row["additional_items"] = json!([]);
        row["default_derived_hex"] = json!("");
        value
    }
    #[test]
    fn v9_owned_rep_rewards_are_explicit_strict_and_round_trip() {
        let mut value = inventory_document();
        value["version"] = json!(REP_REWARD_VERSION);
        value["initial_definitions"] = json!([]);
        value["definitions"][0]["class"] = json!("LiveryDecalSwatchPackItemData");
        value["definitions"][0]["quantity"] = json!(1);
        for name in [
            "purchase_offers",
            "body_kits",
            "vehicle_scopes",
            "part_locations",
            "purchase_requirements",
            "item_unlocks",
            "fitment_restrictions",
        ] {
            value[name] = json!([]);
        }
        value["category_unlocks"] = Value::Null;
        value["rep_rewards"] =
            json!([{"definition":"11111111111111111111111111111111","minimum_rep_score":25}]);
        let parsed = ItemContent::from_json(&value).unwrap();
        assert_eq!(parsed.to_json(), value);
        assert_eq!(
            parsed
                .purchase_offers()
                .unwrap()
                .item_unlocks()
                .unwrap()
                .rewards()
                .unwrap()
                .len(),
            1
        );
        for mutation in 0..10 {
            let mut bad = value.clone();
            match mutation {
                0 => bad["rep_rewards"][0]["minimum_rep_score"] = json!(0),
                1 => bad["definitions"][0]["flags"]["requires_owner"] = json!(true),
                2 => bad["definitions"][0]["flags"]["uses_scope"] = json!(true),
                3 => bad["definitions"][0]["class"] = json!("AirFilterItemData"),
                4 => bad["definitions"][0]["flags"]["deprecated"] = json!(true),
                5 => bad["definitions"][0]["ownership_level"] = json!(1),
                6 => bad["initial_definitions"] = json!(["11111111111111111111111111111111"]),
                7 => {
                    let duplicate = bad["rep_rewards"][0].clone();
                    bad["rep_rewards"].as_array_mut().unwrap().push(duplicate);
                }
                8 => {
                    bad.as_object_mut().unwrap().remove("rep_rewards");
                }
                _ => bad["version"] = json!(BODY_KIT_VERSION),
            }
            assert!(ItemContent::from_json(&bad).is_err(), "mutation {mutation}");
        }
        value["version"] = json!(BODY_KIT_VERSION);
        value.as_object_mut().unwrap().remove("rep_rewards");
        assert!(
            ItemContent::from_json(&value)
                .unwrap()
                .purchase_offers()
                .unwrap()
                .item_unlocks()
                .unwrap()
                .rewards()
                .is_none()
        );
    }
    #[test]
    fn v8_kit_offers_require_explicit_safe_subtrees_and_reject_legacy_implicit_support() {
        let mut value = inventory_document();
        value["version"] = json!(BODY_KIT_VERSION);
        value["initial_definitions"] = json!([]);
        value["definitions"][0]["class"] = json!("HoodItemData");
        value["definitions"][0]["quantity"] = json!(1);
        value["definitions"][0]["flags"]["requires_owner"] = json!(true);
        let mut kit = value["definitions"][0].clone();
        kit["class"] = json!("BodyKitItemData");
        kit["guid"] = json!("22222222222222222222222222222222");
        kit["sub_items"] = json!(["11111111111111111111111111111111"]);
        value["definitions"].as_array_mut().unwrap().push(kit);
        value["purchase_offers"] = json!(["22222222222222222222222222222222"]);
        value["body_kits"] = value["purchase_offers"].clone();
        value["vehicle_scopes"] = json!([]);
        value["part_locations"] = json!([]);
        value["category_unlocks"] = Value::Null;
        value["purchase_requirements"] =
            json!([{"definition":"22222222222222222222222222222222","minimum_rep_score":0}]);
        value["item_unlocks"] = json!([]);
        value["fitment_restrictions"] = json!([]);
        assert_eq!(ItemContent::from_json(&value).unwrap().to_json(), value);
        for mutation in 0..6 {
            let mut bad = value.clone();
            match mutation {
                0 => bad["body_kits"] = json!([]),
                1 => {
                    bad["body_kits"] = json!([
                        "22222222222222222222222222222222",
                        "22222222222222222222222222222222"
                    ])
                }
                2 => bad["definitions"][0]["class"] = json!("AirFilterItemData"),
                3 => {
                    bad["definitions"][1]["additional_items"] =
                        json!(["11111111111111111111111111111111"])
                }
                4 => bad["version"] = json!(FITMENT_VERSION),
                _ => {
                    bad["version"] = json!(FITMENT_VERSION);
                    bad.as_object_mut().unwrap().remove("body_kits");
                }
            }
            assert!(ItemContent::from_json(&bad).is_err(), "mutation {mutation}");
        }
    }
    #[test]
    fn v7_fitment_restrictions_are_explicit_bounded_and_round_trip() {
        let mut value = inventory_document();
        value["version"] = json!(FITMENT_VERSION);
        value["initial_definitions"] = json!([]);
        value["definitions"][0]["class"] = json!("AirFilterItemData");
        value["definitions"][0]["quantity"] = json!(1);
        value["definitions"][0]["flags"]["requires_owner"] = json!(true);
        let mut target = value["definitions"][0].clone();
        target["guid"] = json!("22222222222222222222222222222222");
        value["definitions"].as_array_mut().unwrap().push(target);
        value["purchase_offers"] = json!(["11111111111111111111111111111111"]);
        value["vehicle_scopes"] = json!([]);
        value["part_locations"] = json!([]);
        value["category_unlocks"] = Value::Null;
        value["purchase_requirements"] =
            json!([{"definition":"11111111111111111111111111111111","minimum_rep_score":0}]);
        value["item_unlocks"] = json!([]);
        value["fitment_restrictions"] = json!([{"definition":"11111111111111111111111111111111","conflicts":["22222222222222222222222222222222"]}]);
        assert_eq!(ItemContent::from_json(&value).unwrap().to_json(), value);
        for bad in [
            json!([]),
            json!(["33333333333333333333333333333333"]),
            json!([
                "22222222222222222222222222222222",
                "22222222222222222222222222222222"
            ]),
            json!(null),
        ] {
            let mut invalid = value.clone();
            invalid["fitment_restrictions"][0]["conflicts"] = bad;
            assert!(ItemContent::from_json(&invalid).is_err());
        }
        let mut duplicate = value.clone();
        duplicate["fitment_restrictions"]
            .as_array_mut()
            .unwrap()
            .push(value["fitment_restrictions"][0].clone());
        assert!(ItemContent::from_json(&duplicate).is_err());
        let mut missing = value.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("fitment_restrictions");
        assert!(ItemContent::from_json(&missing).is_err());
        value["version"] = json!(ITEM_UNLOCK_VERSION);
        assert!(ItemContent::from_json(&value).is_err());
        value
            .as_object_mut()
            .unwrap()
            .remove("fitment_restrictions");
        assert_eq!(ItemContent::from_json(&value).unwrap().to_json(), value);
    }
    #[test]
    fn v6_unlocks_are_explicit_and_consistent_with_offer_requirements() {
        let mut value = inventory_document();
        value["version"] = json!(ITEM_UNLOCK_VERSION);
        value["initial_definitions"] = json!([]);
        value["definitions"][0]["class"] = json!("AirFilterItemData");
        value["definitions"][0]["quantity"] = json!(1);
        value["definitions"][0]["flags"]["requires_owner"] = json!(true);
        value["definitions"][0]["flags"]["purchasable"] = json!(false);
        value["purchase_offers"] = json!(["11111111111111111111111111111111"]);
        value["vehicle_scopes"] = json!([]);
        value["part_locations"] = json!([]);
        value["category_unlocks"] = Value::Null;
        value["purchase_requirements"] =
            json!([{"definition":"11111111111111111111111111111111","minimum_rep_score":25}]);
        value["item_unlocks"] = value["purchase_requirements"].clone();
        assert_eq!(ItemContent::from_json(&value).unwrap().to_json(), value);
        for score in [0, 24, -1] {
            let mut invalid = value.clone();
            invalid["item_unlocks"][0]["minimum_rep_score"] = json!(score);
            assert!(ItemContent::from_json(&invalid).is_err());
        }
        let mut invalid = value.clone();
        invalid["item_unlocks"] = json!([]);
        assert!(ItemContent::from_json(&invalid).is_err());
        let mut invalid = value.clone();
        invalid["purchase_offers"] = json!([]);
        invalid["purchase_requirements"] = json!([]);
        assert!(ItemContent::from_json(&invalid).is_err());
        value["version"] = json!(PART_POLICY_VERSION);
        value.as_object_mut().unwrap().remove("item_unlocks");
        assert!(ItemContent::from_json(&value).is_err());
    }
    #[test]
    fn v2_is_static_typed_content_with_an_explicit_initial_recipe() {
        let value = inventory_document();
        let content = ItemContent::from_json(&value).unwrap();
        assert!(content.purchase_offers().is_none());
        assert_eq!(content.to_json(), value);
        let generated = content
            .inventory()
            .unwrap()
            .instantiate_initial(19)
            .unwrap();
        assert_eq!(generated.collection.items[&19].buy_price, 123);
        assert_eq!(
            generated.collection.items[&19].state,
            nfs_world_core::items::OWNED
        );
        let mut unknown = value;
        unknown["definitions"][0]["default_derived_hex"] = Value::Null;
        let unknown = ItemContent::from_json(&unknown).unwrap();
        assert_eq!(
            unknown.inventory().unwrap().instantiate_initial(19),
            Err(nfs_world_core::items::Error::UnsupportedDefault)
        );
    }
    #[test]
    fn v4_scopes_round_trip_and_reject_incomplete_or_foreign_definition_links() {
        let mut value = inventory_document();
        value["version"] = json!(SCOPED_PURCHASE_VERSION);
        value["definitions"][0]["class"] = json!("AirFilterItemData");
        let mut car = value["definitions"][0].clone();
        car["guid"] = json!("22222222222222222222222222222222");
        car["class"] = json!("RaceVehicleItemData");
        car["default_derived_hex"] = json!("00".repeat(68));
        value["definitions"].as_array_mut().unwrap().push(car);
        value["purchase_offers"] = json!(["11111111111111111111111111111111"]);
        value["vehicle_scopes"] = json!([{"vehicle":"22222222222222222222222222222222",
            "definitions":["11111111111111111111111111111111"]}]);
        let content = ItemContent::from_json(&value).unwrap();
        assert_eq!(content.to_json(), value);
        assert!(
            content
                .purchase_offers()
                .unwrap()
                .allows(&[0x22; 16], &[0x11; 16])
        );
        assert!(
            !content
                .purchase_offers()
                .unwrap()
                .allows(&[0x33; 16], &[0x11; 16])
        );
        for (field, bad) in [
            ("vehicle", json!("11111111111111111111111111111111")),
            ("definitions", json!(["33333333333333333333333333333333"])),
            (
                "definitions",
                json!([
                    "11111111111111111111111111111111",
                    "11111111111111111111111111111111"
                ]),
            ),
        ] {
            let mut bad_value = value.clone();
            bad_value["vehicle_scopes"][0][field] = bad;
            assert!(ItemContent::from_json(&bad_value).is_err());
        }
        value.as_object_mut().unwrap().remove("vehicle_scopes");
        assert!(ItemContent::from_json(&value).is_err());
    }

    #[test]
    fn v3_offers_are_explicit_bounded_and_resolve_to_purchasable_parts() {
        let mut value = inventory_document();
        value["version"] = json!(PURCHASE_VERSION);
        value["definitions"][0]["class"] = json!("AirFilterItemData");
        value["purchase_offers"] = json!(["11111111111111111111111111111111"]);
        let content = ItemContent::from_json(&value).unwrap();
        assert_eq!(content.to_json(), value);
        assert_eq!(content.purchase_offers().unwrap().definitions().count(), 1);
        for offers in [
            json!(["22222222222222222222222222222222"]),
            json!([
                "11111111111111111111111111111111",
                "11111111111111111111111111111111"
            ]),
        ] {
            let mut invalid = value.clone();
            invalid["purchase_offers"] = offers;
            assert!(ItemContent::from_json(&invalid).is_err());
        }
        value["definitions"][0]["flags"]["purchasable"] = json!(false);
        assert!(ItemContent::from_json(&value).is_err());
    }
    #[test]
    fn v5_spatial_and_rep_policy_is_complete_strict_and_round_trips() {
        let mut value = inventory_document();
        value["version"] = json!(PART_POLICY_VERSION);
        value["category_unlocks"] = Value::Null;
        value["definitions"][0]["class"] = json!("RimsItemData");
        value["definitions"][0]["default_derived_hex"] = json!("00".repeat(52));
        let mut car = value["definitions"][0].clone();
        car["guid"] = json!("22222222222222222222222222222222");
        car["class"] = json!("RaceVehicleItemData");
        car["default_derived_hex"] = json!("00".repeat(68));
        value["definitions"].as_array_mut().unwrap().push(car);
        value["purchase_offers"] = json!(["11111111111111111111111111111111"]);
        value["vehicle_scopes"] = json!([{"vehicle":"22222222222222222222222222222222","definitions":["11111111111111111111111111111111"]}]);
        value["part_locations"] =
            json!([{"definition":"11111111111111111111111111111111","location":1}]);
        value["purchase_requirements"] =
            json!([{"definition":"11111111111111111111111111111111","minimum_rep_score":25}]);
        assert_eq!(ItemContent::from_json(&value).unwrap().to_json(), value);
        for (key, field, bad) in [
            ("part_locations", "location", json!(2)),
            ("part_locations", "location", json!(-1)),
            (
                "part_locations",
                "definition",
                json!("22222222222222222222222222222222"),
            ),
            ("purchase_requirements", "minimum_rep_score", json!(-1)),
            (
                "purchase_requirements",
                "minimum_rep_score",
                json!(u32::MAX),
            ),
            (
                "purchase_requirements",
                "definition",
                json!("22222222222222222222222222222222"),
            ),
        ] {
            let mut changed = value.clone();
            changed[key][0][field] = bad;
            assert!(ItemContent::from_json(&changed).is_err());
        }
        for key in ["part_locations", "purchase_requirements"] {
            let mut missing = value.clone();
            missing[key] = json!([]);
            assert!(ItemContent::from_json(&missing).is_err());
            let mut duplicate = value.clone();
            duplicate[key]
                .as_array_mut()
                .unwrap()
                .push(value[key][0].clone());
            assert!(ItemContent::from_json(&duplicate).is_err());
        }
    }
    #[test]
    fn v2_rejects_profile_fields_bad_scalars_layouts_and_unknown_links() {
        for (field, bad) in [
            ("owner", json!(7)),
            ("state", json!(2)),
            ("buy_price", json!(-1)),
            ("sell_price", json!(u64::MAX)),
            ("quantity", json!(1.5)),
            ("ownership_level", json!(3)),
            ("default_derived_hex", json!("00")),
            ("sub_items", json!(["22222222222222222222222222222222"])),
        ] {
            let mut value = inventory_document();
            value["definitions"][0][field] = bad;
            assert!(ItemContent::from_json(&value).is_err(), "accepted {field}");
        }
        let mut value = inventory_document();
        value["definitions"][0]["flags"]["optional"] = json!(1);
        assert!(ItemContent::from_json(&value).is_err());
        value = inventory_document();
        value["definitions"][0]["sub_items"] = value["initial_definitions"].clone();
        assert!(ItemContent::from_json(&value).is_err());
    }
}
