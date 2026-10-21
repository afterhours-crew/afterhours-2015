// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The installation-derived content a profile reads: REP thresholds,
//! persistent-table schemas, item definitions and vehicle blueprint names.
//!
//! Every file is optional. A missing or invalid file only removes the fields
//! it supplies and adds a warning; the save itself is still shown. Each file
//! is validated by the same loader the server uses, except the vehicle
//! names, which read only two fields of the server's vehicle content.

use crate::{
    exit::{Failure, Result},
    paths, records,
};
use nfs_services::{item_content::ItemContent, persistent, reputation::Thresholds};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

/// Content cache directories inspected when choosing one automatically.
const MAX_CACHE_ENTRIES: usize = 256;
/// The server's own bound for the vehicle content file.
const MAX_VEHICLE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_VEHICLES: usize = 512;
const MAX_ALIASES: usize = 512;
const MAX_BLUEPRINT_BYTES: usize = 256;

/// How much content a command needs. Listing accounts skips the large item
/// and vehicle files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Summary,
    Full,
}

/// Where the content came from, reported with the profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(rename = "ContentSource")]
pub struct Source {
    pub directory: PathBuf,
    /// `argument`, `current-session` or `only-cache`.
    pub chosen_by: &'static str,
}

/// A vehicle's blueprint from the installed vehicle content. `label` is the
/// blueprint's asset name with the `Car_` prefix removed and underscores as
/// spaces; it is not the game's localized display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Vehicle {
    pub blueprint: String,
    pub asset_name: String,
    pub label: String,
}

/// Item definition GUID to vehicle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vehicles(BTreeMap<[u8; 16], Vehicle>);

#[derive(Deserialize)]
struct VehicleFile {
    format: String,
    version: u64,
    build_sha256: String,
    vehicles: Vec<VehicleRow>,
}
#[derive(Deserialize)]
struct VehicleRow {
    blueprint: String,
    definitions: Vec<String>,
}

/// 32 lowercase hexadecimal characters as a GUID.
pub fn guid(text: &str) -> Option<[u8; 16]> {
    if !records::is_lower_hex(text, 32) {
        return None;
    }
    let mut out = [0; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

impl Vehicles {
    pub fn from_slice(bytes: &[u8]) -> std::result::Result<Self, String> {
        let file: VehicleFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if file.format != "nfs-vehicle-content"
            || !(1..=2).contains(&file.version)
            || file.build_sha256 != nfs_services::SUPPORTED_BUILD_SHA256
        {
            return Err("unsupported format, version or build".into());
        }
        if file.vehicles.len() > MAX_VEHICLES {
            return Err("too many vehicles".into());
        }
        let mut out = BTreeMap::new();
        for row in file.vehicles {
            let blueprint = row.blueprint;
            if blueprint.is_empty()
                || blueprint.len() > MAX_BLUEPRINT_BYTES
                || !blueprint
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
            {
                return Err("invalid blueprint".into());
            }
            if row.definitions.is_empty() || row.definitions.len() > MAX_ALIASES {
                return Err("invalid definition list".into());
            }
            let asset_name = blueprint.rsplit('/').next().unwrap_or_default().to_owned();
            let label = asset_name
                .strip_prefix("Car_")
                .unwrap_or(&asset_name)
                .replace('_', " ");
            let vehicle = Vehicle {
                blueprint,
                asset_name,
                label,
            };
            for alias in &row.definitions {
                let id = guid(alias).ok_or("invalid definition")?;
                if out.insert(id, vehicle.clone()).is_some() {
                    return Err("duplicate definition".into());
                }
            }
        }
        Ok(Self(out))
    }

    fn load(path: &Path) -> std::result::Result<Self, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_VEHICLE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_VEHICLE_BYTES {
            return Err("file exceeds its bound".into());
        }
        Self::from_slice(&bytes)
    }

    pub fn get(&self, definition: &[u8; 16]) -> Option<&Vehicle> {
        self.0.get(definition)
    }
}

/// Loaded content. A `None` field was absent or invalid; `warnings` says which.
#[derive(Default)]
pub struct Content {
    pub source: Option<Source>,
    pub thresholds: Option<Thresholds>,
    pub tables: Option<persistent::Catalog>,
    pub items: Option<ItemContent>,
    pub vehicles: Option<Vehicles>,
    /// Localized car names read from the installation; see [`super::localize`].
    pub car_names: Option<super::names::CarNames>,
    pub localization: Option<super::names::Localization>,
    /// The decoded car images for this content, when `car-images` has run.
    pub car_images: Option<super::images::Cached>,
    pub warnings: Vec<String>,
}

/// An existing ordinary directory or file reached without any link.
fn ordinary(path: &Path, directory: bool) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| {
        !paths::is_link(&meta)
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            }
    })
}

/// `<data>/artifacts/content/<fingerprint>` when every step is ordinary.
fn cache(data: &Path, fingerprint: &str) -> Option<PathBuf> {
    let artifacts = data.join("artifacts");
    let root = artifacts.join("content");
    let directory = root.join(fingerprint);
    (records::is_lower_hex(fingerprint, 64)
        && ordinary(&artifacts, true)
        && ordinary(&root, true)
        && ordinary(&directory, true))
    .then_some(directory)
}

/// The content the current session's preparation reported.
fn from_current_session(data: &Path) -> Option<PathBuf> {
    let pointer: records::SessionPointer =
        records::read_json(&data.join(crate::session::POINTER_FILE)).ok()?;
    if pointer.version != records::VERSION || !records::is_id(&pointer.id) {
        return None;
    }
    let report = paths::session_dir(data, &pointer.id).join("prepare.stdout.json");
    let value: serde_json::Value =
        serde_json::from_slice(&records::read_bounded(&report).ok()?).ok()?;
    cache(data, value["install_content"]["fingerprint"].as_str()?)
}

/// The only content cache in the data directory, if there is exactly one.
fn only_cache(data: &Path) -> Option<PathBuf> {
    let root = data.join("artifacts").join("content");
    let mut found = None;
    for entry in std::fs::read_dir(&root).ok()?.take(MAX_CACHE_ENTRIES) {
        let name = entry.ok()?.file_name();
        let Some(directory) = name.to_str().and_then(|name| cache(data, name)) else {
            continue;
        };
        if found.replace(directory).is_some() {
            return None;
        }
    }
    found
}

/// Choose the content directory: the explicit one, else the one the current
/// session prepared, else the only cache present.
pub fn locate(data: &Path, explicit: Option<&Path>) -> Result<Option<Source>> {
    if let Some(directory) = explicit {
        let directory = paths::normalize(directory)?;
        if !ordinary(&directory, true) {
            return Err(Failure::refused(
                "--content-dir must be an existing ordinary directory.",
            ));
        }
        return Ok(Some(Source {
            directory,
            chosen_by: "argument",
        }));
    }
    if let Some(directory) = from_current_session(data) {
        return Ok(Some(Source {
            directory,
            chosen_by: "current-session",
        }));
    }
    Ok(only_cache(data).map(|directory| Source {
        directory,
        chosen_by: "only-cache",
    }))
}

impl Content {
    /// Load what `need` asks for from `source`. Never fails: problems become
    /// warnings and absent fields.
    pub fn load(source: Option<Source>, need: Need) -> Self {
        let mut content = Content::default();
        let Some(source) = source else {
            content.warnings.push(
                "No prepared content found; REP levels, table schemas and car names are unavailable. Run setup, or pass --content-dir."
                    .into(),
            );
            return content;
        };
        let directory = source.directory.clone();
        content.source = Some(source);
        let warnings = &mut content.warnings;
        let mut file = |name: &str, what: &str| {
            let path = directory.join(name);
            if ordinary(&path, false) {
                Some(path)
            } else {
                warnings.push(format!("{name} is missing; {what} are unavailable."));
                None
            }
        };
        let thresholds = file("reputation-thresholds.json", "REP levels and thresholds");
        let tables = file("persistent-content.json", "REP scores and saved stats");
        let (items, vehicles) = match need {
            Need::Summary => (None, None),
            Need::Full => (
                file("item-content.json", "item classes"),
                file("vehicle-profiles.json", "car names"),
            ),
        };
        content.thresholds = thresholds.and_then(|path| {
            Thresholds::load(&path)
                .map_err(|e| {
                    content
                        .warnings
                        .push(format!("reputation-thresholds.json: {e}"))
                })
                .ok()
        });
        content.tables = tables.and_then(|path| {
            persistent::Catalog::load(&path)
                .map_err(|e| {
                    content
                        .warnings
                        .push(format!("persistent-content.json: {e}"))
                })
                .ok()
        });
        content.items = items.and_then(|path| {
            ItemContent::load(&path)
                .map_err(|e| content.warnings.push(format!("item-content.json: {e}")))
                .ok()
        });
        content.vehicles = vehicles.and_then(|path| {
            Vehicles::load(&path)
                .map_err(|e| content.warnings.push(format!("vehicle-profiles.json: {e}")))
                .ok()
        });
        content
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn vehicle_file(rows: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "format": "nfs-vehicle-content",
            "version": 2,
            "build_sha256": nfs_services::SUPPORTED_BUILD_SHA256,
            "vehicles": rows,
        }))
        .unwrap()
    }

    #[test]
    fn vehicle_names_come_from_blueprints_and_reject_ambiguous_files() {
        let id = "322ee1d18db783faf3017e18d69cfcc7";
        let bytes = vehicle_file(json!([{
            "blueprint": "Vehicles/player/Car_Acura_RSXS_2004/Car_Acura_RSXS_2004",
            "definitions": [id],
            "appearance": {"ignored": true}
        }]));
        let vehicles = Vehicles::from_slice(&bytes).unwrap();
        let vehicle = vehicles.get(&guid(id).unwrap()).unwrap();
        assert_eq!(vehicle.asset_name, "Car_Acura_RSXS_2004");
        assert_eq!(vehicle.label, "Acura RSXS 2004");
        assert!(vehicles.get(&[0; 16]).is_none());
        for rows in [
            json!([{"blueprint": "a", "definitions": [id]}, {"blueprint": "b", "definitions": [id]}]),
            json!([{"blueprint": "a", "definitions": []}]),
            json!([{"blueprint": "a", "definitions": ["322EE1D18DB783FAF3017E18D69CFCC7"]}]),
            json!([{"blueprint": "", "definitions": [id]}]),
            json!([{"blueprint": "a\u{0}", "definitions": [id]}]),
            json!([{"blueprint": "a".repeat(MAX_BLUEPRINT_BYTES + 1), "definitions": [id]}]),
            json!([{"definitions": [id]}]),
        ] {
            assert!(Vehicles::from_slice(&vehicle_file(rows)).is_err());
        }
        let mut other_build: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        other_build["build_sha256"] = json!("0".repeat(64));
        assert!(Vehicles::from_slice(&serde_json::to_vec(&other_build).unwrap()).is_err());
        let many: Vec<_> = (0..=MAX_VEHICLES)
            .map(|i| json!({"blueprint": format!("v{i}"), "definitions": [format!("{i:032x}")]}))
            .collect();
        assert!(Vehicles::from_slice(&vehicle_file(json!(many))).is_err());
    }

    #[test]
    fn content_is_chosen_explicitly_from_the_current_session_or_as_the_only_cache() {
        let data = crate::testing::scratch("profile-content");
        assert_eq!(locate(&data, None).unwrap(), None);
        assert!(locate(&data, Some(&data.join("missing"))).is_err());
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let root = data.join("artifacts").join("content");
        std::fs::create_dir_all(root.join(&a)).unwrap();
        std::fs::create_dir_all(root.join("not-a-fingerprint")).unwrap();
        let chosen = locate(&data, None).unwrap().unwrap();
        assert_eq!(
            (chosen.directory, chosen.chosen_by),
            (root.join(&a), "only-cache")
        );
        std::fs::create_dir_all(root.join(&b)).unwrap();
        assert_eq!(locate(&data, None).unwrap(), None, "ambiguous caches");
        let id = "c".repeat(32);
        let session = paths::session_dir(&data, &id);
        std::fs::create_dir_all(&session).unwrap();
        records::write_new_json(
            &data.join(crate::session::POINTER_FILE),
            &records::SessionPointer {
                version: records::VERSION,
                id: id.clone(),
            },
        )
        .unwrap();
        std::fs::write(
            session.join("prepare.stdout.json"),
            json!({"install_content": {"fingerprint": b, "directory": "ignored"}}).to_string(),
        )
        .unwrap();
        let chosen = locate(&data, None).unwrap().unwrap();
        assert_eq!(
            (chosen.directory, chosen.chosen_by),
            (root.join(&b), "current-session")
        );
        // A recorded fingerprint never leads outside the content cache.
        std::fs::write(
            session.join("prepare.stdout.json"),
            json!({"install_content": {"fingerprint": "../../.."}}).to_string(),
        )
        .unwrap();
        assert_eq!(locate(&data, None).unwrap(), None);
        let explicit = locate(&data, Some(&root.join(&a))).unwrap().unwrap();
        assert_eq!(explicit.chosen_by, "argument");
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn missing_and_invalid_files_become_warnings() {
        let data = crate::testing::scratch("profile-content-files");
        let content = Content::load(None, Need::Full);
        assert!(content.thresholds.is_none() && content.warnings.len() == 1);
        std::fs::write(data.join("reputation-thresholds.json"), b"{}").unwrap();
        let source = Source {
            directory: data.clone(),
            chosen_by: "argument",
        };
        let summary = Content::load(Some(source.clone()), Need::Summary);
        assert!(summary.thresholds.is_none() && summary.tables.is_none());
        assert_eq!(summary.warnings.len(), 2, "{:?}", summary.warnings);
        let full = Content::load(Some(source), Need::Full);
        assert_eq!(full.warnings.len(), 4, "{:?}", full.warnings);
        std::fs::remove_dir_all(data).unwrap();
    }
}
