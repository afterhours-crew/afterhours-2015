// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The game's localized car names, read from the player's own installation
//! each time a profile is shown. Nothing is written, cached or shipped.
//!
//! In the supported build every item, the cars included, is listed in the
//! `Win32/levels/frontend/frontend` superbundle. A car's
//! `RaceVehicleItemData` instance, whose GUID is the item definition a save
//! stores, carries `UI.LongNameId`, `MediumNameId` and `ShortNameId`. The
//! locale's `FsUITextDatabase` asset in `Win32/globals` names a binary and a
//! histogram chunk, both kept inline in a `Win32/loc/<language>`
//! superbundle; [`nfs_frostbite::textdb`] decodes them. These locations are
//! facts of the supported build; another layout yields no names, never
//! guessed ones.

use nfs_frostbite::{
    Limits,
    ebx::{Partition, Value},
    install::{Installation, Store},
    textdb::{Lookup, TextDatabase},
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const ITEM_SUPERBUNDLE: &str = "Win32/levels/frontend/frontend";
pub const TEXT_SUPERBUNDLE: &str = "Win32/globals";
/// Superbundles holding the text database chunks, compared in lower case.
const LOC_PREFIX: &str = "win32/loc/";
const ITEM_PREFIX: &str = "items/";
const VEHICLE_CLASS: &str = "RaceVehicleItemData";
const TEXT_CLASSES: [&str; 2] = ["FsUITextDatabase", "UITextDatabase"];
/// The locale used when neither an argument nor the registry names one.
pub const DEFAULT_LOCALE: &str = "en_US";
/// Item assets inspected; the supported build lists about five thousand.
pub const MAX_ITEM_ASSETS: usize = 16_384;
/// Car definitions kept; the supported build has fewer than a hundred.
pub const MAX_VEHICLES: usize = 512;

/// A car's three names in one language. A name is `null` when its string ID
/// is absent from the text database or uses an encoding this reader does
/// not decode.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(rename = "CarNames")]
pub struct Names {
    /// The longest form; a car's `name` repeats it.
    pub long: Option<String>,
    pub medium: Option<String>,
    pub short: Option<String>,
}

/// Where a profile's car names came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Localization {
    /// The text database's locale, such as `en_US`.
    pub locale: String,
    /// `argument`, `registry` (the game's own registration) or `default`.
    pub locale_from: &'static str,
    pub game_directory: PathBuf,
    /// `argument`, `current-session` or `registry`.
    pub game_directory_from: &'static str,
    /// Car definitions the installation lists.
    pub cars: usize,
}

/// A car definition's names and the string IDs that did not resolve.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub names: Names,
    pub unresolved: Vec<String>,
}

/// Every car definition's names in one locale.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CarNames {
    pub locale: String,
    entries: BTreeMap<[u8; 16], Entry>,
}

impl CarNames {
    pub fn get(&self, definition: &[u8; 16]) -> Option<&Entry> {
        self.entries.get(definition)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `ll_CC`, such as `en_US`: two lower-case and two upper-case letters.
pub fn is_locale(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2] == b'_'
        && bytes[3..].iter().all(u8::is_ascii_uppercase)
}

pub(crate) fn failure(what: &str, error: nfs_frostbite::Error) -> String {
    format!("{what}: {error}")
}

/// The locale's text database from the installation.
fn text_database(
    install: &Installation,
    store: &mut Store,
    locale: &str,
) -> Result<TextDatabase, String> {
    let asset = format!("localization/textdbs/{}", locale.to_ascii_lowercase());
    let listed = install
        .superbundle_ebx(TEXT_SUPERBUNDLE)
        .map_err(|e| failure("text database list", e))?;
    let location = listed
        .iter()
        .find(|location| location.entry.name == asset)
        .ok_or_else(|| format!("the installation has no {locale} text database"))?;
    let objects = store
        .objects(&location.entry)
        .map_err(|e| failure("text database asset", e))?;
    let database = objects
        .iter()
        .find(|object| TEXT_CLASSES.contains(&object.class.as_str()))
        .ok_or("the text database asset has no text database object")?;
    let guid = |field: &str| match database.fields.get(field) {
        Some(Value::Guid(guid)) => Ok(*guid),
        _ => Err(format!("the text database asset has no {field}")),
    };
    let size = |field: &str| database.fields.get(field).and_then(Value::bits32);
    let wanted = [
        (guid("BinaryChunk")?, size("BinaryChunkSize")),
        (guid("HistogramChunk")?, size("HistogramChunkSize")),
    ];
    let mut found: [Option<Vec<u8>>; 2] = [None, None];
    for superbundle in install.superbundles() {
        if !superbundle.to_ascii_lowercase().starts_with(LOC_PREFIX) {
            continue;
        }
        let chunks = install
            .inline_chunks(superbundle)
            .map_err(|e| failure("text chunk list", e))?;
        for chunk in chunks {
            for (slot, (id, expected)) in found.iter_mut().zip(wanted) {
                if slot.is_none() && chunk.id == id {
                    let bytes = install
                        .inline_chunk_bytes(&chunk)
                        .map_err(|e| failure("text chunk", e))?;
                    if expected.is_some_and(|size| size as usize != bytes.len()) {
                        return Err("a text chunk differs from its declared size".into());
                    }
                    *slot = Some(bytes);
                }
            }
        }
        if found.iter().all(Option::is_some) {
            break;
        }
    }
    let [Some(binary), Some(histogram)] = found else {
        return Err(format!("the {locale} text chunks are missing"));
    };
    TextDatabase::parse(&binary, &histogram, install.limits())
        .map_err(|e| failure("text database", e))
}

fn names(ui: Option<&nfs_frostbite::ebx::Fields>, text: &TextDatabase) -> Entry {
    let mut unresolved = Vec::new();
    let mut lookup = |field: &str| {
        let id = ui.and_then(|ui| ui.get(field)).and_then(Value::as_str)?;
        if id.is_empty() {
            return None;
        }
        match text.get(id) {
            Lookup::Found(name) => Some(name),
            Lookup::Missing | Lookup::Undecodable => {
                unresolved.push(id.to_owned());
                None
            }
        }
    };
    let names = Names {
        long: lookup("LongNameId"),
        medium: lookup("MediumNameId"),
        short: lookup("ShortNameId"),
    };
    Entry { names, unresolved }
}

/// Every car definition's names in `locale`, read from the installation at
/// `game`. Errors are sentences for a warning; nothing is partially guessed.
pub fn read(game: &Path, locale: &str, limits: Limits) -> Result<CarNames, String> {
    let install = Installation::open(game, limits).map_err(|e| failure("installation", e))?;
    let mut store = Store::open(&install).map_err(|e| failure("archive catalogue", e))?;
    let text = text_database(&install, &mut store, locale)?;
    if !text.is_single_byte() {
        return Err(format!(
            "the {locale} text database uses a multi-byte encoding this launcher does not decode"
        ));
    }
    let entries = car_items(&install, &mut store, &limits)?
        .into_iter()
        .map(|(guid, fields)| {
            (
                guid,
                names(fields.get("UI").and_then(Value::as_fields), &text),
            )
        })
        .collect();
    Ok(CarNames {
        locale: locale.to_owned(),
        entries,
    })
}

/// Every car item of the installation: its definition GUID and the fields
/// of its `RaceVehicleItemData` instance, in item-asset order.
pub(crate) fn car_items(
    install: &Installation,
    store: &mut Store,
    limits: &Limits,
) -> Result<Vec<([u8; 16], nfs_frostbite::ebx::Fields)>, String> {
    let listed = install
        .superbundle_ebx(ITEM_SUPERBUNDLE)
        .map_err(|e| failure("item list", e))?;
    let mut cars = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut inspected = 0usize;
    for location in listed
        .iter()
        .filter(|location| location.entry.name.starts_with(ITEM_PREFIX))
    {
        inspected += 1;
        if inspected > MAX_ITEM_ASSETS {
            return Err("the installation lists more item assets than the bound".into());
        }
        let data = store
            .bytes(&location.entry)
            .map_err(|e| failure("item asset", e))?;
        let partition = Partition::parse(&data, limits).map_err(|e| failure("item asset", e))?;
        if partition.primary_class() != Some(VEHICLE_CLASS) {
            continue;
        }
        let objects = partition
            .objects(limits)
            .map_err(|e| failure("car item", e))?;
        for object in objects {
            let Some(guid) = object.guid.filter(|_| object.class == VEHICLE_CLASS) else {
                continue;
            };
            if !seen.insert(guid) {
                return Err("a car definition is listed twice".into());
            }
            if seen.len() > MAX_VEHICLES {
                return Err("the installation lists more cars than the bound".into());
            }
            cars.push((guid, object.fields));
        }
    }
    if cars.is_empty() {
        return Err("the installation lists no car items".into());
    }
    Ok(cars)
}

#[cfg(test)]
pub(crate) mod tests;
