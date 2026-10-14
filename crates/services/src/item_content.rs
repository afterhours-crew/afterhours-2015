// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Versioned static item definitions. V1 contains GUID/class bindings; V2 adds
//! asset prices, links, constructor defaults and an explicit initial recipe.
//! V3 adds explicit supported purchase offers. No format accepts captured
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
pub const MAX_STORE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemContent {
    pub catalog: Catalog,
    inventory: Option<InventoryCatalog>,
    purchase_offers: Option<crate::purchase::Offers>,
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
        if value["version"] == INVENTORY_VERSION || value["version"] == PURCHASE_VERSION {
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
        })
    }
    pub fn inventory(&self) -> Option<&InventoryCatalog> {
        self.inventory.as_ref()
    }
    pub fn purchase_offers(&self) -> Option<&crate::purchase::Offers> {
        self.purchase_offers.as_ref()
    }
    fn inventory_from_json(value: &Value) -> Result<Self, ContentError> {
        let mut expected = vec![
            "format",
            "version",
            "build_sha256",
            "definitions",
            "initial_definitions",
        ];
        if value["version"] == PURCHASE_VERSION {
            expected.push("purchase_offers");
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
        let purchase_offers = if value["version"] == PURCHASE_VERSION {
            let offers = crate::purchase::Offers::new(guids(&value["purchase_offers"])?)
                .map_err(|_| ContentError::Invalid)?;
            for guid in offers.definitions() {
                let definition = inventory
                    .definition(guid)
                    .map_err(|_| ContentError::Invalid)?;
                if definition.class != DefinitionClass::AirFilterItemData
                    || !definition.flags.purchasable
                {
                    return Err(ContentError::Invalid);
                }
            }
            Some(offers)
        } else {
            None
        };
        Ok(Self {
            catalog: inventory.bindings().clone(),
            inventory: Some(inventory),
            purchase_offers,
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
