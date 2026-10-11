// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scene serializer order comes from the installed composition graph, not from
//! a list of scene-specific indexes. Classes are interpreted only under the
//! selected executable's server construction rules. Shared blueprint instances
//! contribute once per occurrence; only cycles on the active path are rejected.
use crate::{Error, assets::AssetIndex, scan::Assets};
use nfs_frostbite::ebx::{Pointer, Value};
use nfs_world::replication::sublevel::{Content, MAX_PROFILES};
use nfs_world::replication::sublevel::{Kind, MAX_SERIALIZERS, Profile};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::{fs::File, io::Read, path::Path};

pub const MAX_GRAPH_DEPTH: usize = 32;
pub const MAX_GRAPH_OBJECTS: usize = 50_000;
pub const MAX_CATALOG_BYTES: u64 = 2 << 20;
pub const FORMAT: &str = "nfs-scene-catalog";

/// Executable-specific server construction rules. `None` means a known class
/// contributes no serializer; an absent class is unsupported, never skipped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rules {
    pub level: String,
    pub classes: BTreeMap<String, Option<Kind>>,
    pub launchers: BTreeSet<String>,
}

/// One scene's immutable wire shape and the serializer indexes of launchers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scene {
    pub profile: Profile,
    pub launchers: Vec<usize>,
}

/// Installed scene shapes and launcher endpoints, without live identities,
/// state values, service policy or network messages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    pub level: String,
    pub profiles: Content,
    pub launchers: nfs_world::launchers::Catalog,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_CATALOG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CATALOG_BYTES {
            return Err(Error::Content("scene catalog size bound".into()));
        }
        Self::from_json(
            &serde_json::from_slice(&bytes)
                .map_err(|_| Error::Content("invalid scene catalog JSON".into()))?,
        )
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Self, Error> {
        let bad = || Error::Content("invalid scene catalog".into());
        let fields = |v: &serde_json::Value, names: &[&str]| -> Result<(), Error> {
            let o = v.as_object().ok_or_else(bad)?;
            if o.len() != names.len() || names.iter().any(|n| !o.contains_key(*n)) {
                return Err(bad());
            }
            Ok(())
        };
        fields(
            value,
            &["format", "version", "build_sha256", "level", "scenes"],
        )?;
        if value["format"] != FORMAT
            || value["version"] != 1
            || value["build_sha256"] != nfs_services::SUPPORTED_BUILD_SHA256
        {
            return Err(bad());
        }
        let level = value["level"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or_else(bad)?
            .to_owned();
        let rows = value["scenes"]
            .as_array()
            .filter(|a| a.len() <= MAX_PROFILES)
            .ok_or_else(bad)?;
        let mut profiles = Vec::new();
        let mut launchers = Vec::new();
        for row in rows {
            fields(row, &["content_key", "root", "serializers", "launchers"])?;
            let key = row["content_key"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(bad)?;
            let root = row["root"].as_bool().ok_or_else(bad)?;
            if root != (key == 1) {
                return Err(bad());
            }
            let entries = row["serializers"]
                .as_array()
                .filter(|a| a.len() <= MAX_SERIALIZERS)
                .ok_or_else(bad)?;
            let kinds = entries
                .iter()
                .map(|v| Kind::from_name(v.as_str().ok_or_else(bad)?).ok_or_else(bad))
                .collect::<Result<Vec<_>, _>>()?;
            profiles.push((key, Profile::new(root, &kinds).map_err(|_| bad())?));
            let indices = row["launchers"]
                .as_array()
                .filter(|a| a.len() <= MAX_SERIALIZERS)
                .ok_or_else(bad)?;
            let indices = indices
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or_else(bad)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !indices.is_empty() {
                launchers.push((key, indices));
            }
        }
        let profiles = Content::new(profiles).map_err(|_| bad())?;
        profiles.profile(1).map_err(|_| bad())?;
        let launchers =
            nfs_world::launchers::Catalog::new(launchers, &profiles).map_err(|_| bad())?;
        Ok(Self {
            level,
            profiles,
            launchers,
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        let launchers: BTreeMap<_, _> = self.launchers.entries().collect();
        json!({"format":FORMAT,"version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
            "level":self.level,"scenes":self.profiles.profiles().map(|(key,profile)| json!({
                "content_key":key,"root":profile.is_root(),
                "serializers":profile.kinds().iter().map(|k|k.name()).collect::<Vec<_>>(),
                "launchers":launchers.get(&key).copied().unwrap_or(&[])
            })).collect::<Vec<_>>()})
    }
}

pub(crate) fn build(assets: &mut Assets, rules: &Rules) -> Result<Vec<u8>, Error> {
    let mut scenes = BTreeMap::new();
    let mut names = BTreeMap::from([(1, rules.level.to_lowercase())]);
    scenes.insert(1, profile(assets, rules, &rules.level)?);
    let startup = crate::world::startup_entries(assets, &rules.level)?;
    for entry in startup {
        let scene = match entry.kind {
            crate::world::StartupKind::SubLevel => profile(assets, rules, &entry.name)?,
            _ => bundle(assets, &entry.name)?,
        };
        let key = crate::assets::scene_key(&entry.name);
        names.insert(key, entry.name.to_lowercase());
        if scenes.insert(key, scene).is_some() {
            return Err(Error::Content("scene content key collision".into()));
        }
        if scenes.len() > MAX_PROFILES {
            return Err(Error::Content("scene count bound".into()));
        }
    }
    // A player's selected vehicle bundle is loaded dynamically. Discover every
    // race-vehicle bundle instead of baking a particular garage's choices in.
    for name in assets.named("BlueprintBundle") {
        let object = assets
            .objects(&name)?
            .first()
            .cloned()
            .ok_or_else(|| Error::Content("empty bundle".into()))?;
        let (_, blueprint) =
            assets.resolve(object.fields.get("Blueprint").and_then(Value::as_pointer))?;
        if blueprint.class == "RaceVehicleBlueprint" {
            let key = crate::assets::scene_key(&name);
            if names.get(&key).is_some_and(|previous| previous != &name) {
                return Err(Error::Content("scene content key collision".into()));
            }
            names.insert(key, name.clone());
            // Startup traffic bundles can also be race-vehicle bundles.
            if let std::collections::btree_map::Entry::Vacant(entry) = scenes.entry(key) {
                entry.insert(bundle(assets, &name)?);
            }
            if scenes.len() > MAX_PROFILES {
                return Err(Error::Content("scene count bound".into()));
            }
        }
    }
    let profiles = Content::new(
        scenes
            .iter()
            .map(|(k, s)| (*k, s.profile.clone()))
            .collect(),
    )
    .map_err(|_| Error::Content("scene catalog profiles".into()))?;
    let launchers = nfs_world::launchers::Catalog::new(
        scenes
            .into_iter()
            .filter(|(_, s)| !s.launchers.is_empty())
            .map(|(k, s)| (k, s.launchers))
            .collect(),
        &profiles,
    )
    .map_err(|_| Error::Content("scene catalog launchers".into()))?;
    let value = Catalog {
        level: rules.level.clone(),
        profiles,
        launchers,
    }
    .to_json();
    Catalog::from_json(&value)?;
    serde_json::to_vec(&value).map_err(|_| Error::Content("scene catalog encoding".into()))
}

fn bundle(assets: &mut Assets, name: &str) -> Result<Scene, Error> {
    if assets
        .objects(&name.to_lowercase())?
        .first()
        .is_none_or(|o| o.class != "BlueprintBundle")
    {
        return Err(Error::Content(format!("{name} is not a blueprint bundle")));
    }
    // A BlueprintBundle is a loadable asset collection. Its blueprint is
    // instantiated separately by entity ownership; it is not a sub-level's
    // composition graph. The bundle's scene owner has only the common root.
    Ok(Scene {
        profile: Profile::new(false, &[Kind::Noop])
            .map_err(|_| Error::Content("bundle profile".into()))?,
        launchers: Vec::new(),
    })
}

struct Walk<'a> {
    assets: &'a mut Assets,
    rules: &'a Rules,
    active: BTreeSet<(String, usize)>,
    visited: usize,
    kinds: Vec<Kind>,
    launchers: Vec<usize>,
}

impl Walk<'_> {
    fn visit(&mut self, asset: &str, index: usize, depth: usize) -> Result<(), Error> {
        if depth > MAX_GRAPH_DEPTH || self.visited >= MAX_GRAPH_OBJECTS {
            return Err(Error::Content(
                "scene composition exceeds its graph bound".into(),
            ));
        }
        let key = (asset.to_owned(), index);
        if !self.active.insert(key.clone()) {
            return Err(Error::Content(format!(
                "scene composition cycle in {asset}"
            )));
        }
        self.visited += 1;
        let object = self
            .assets
            .objects(asset)?
            .get(index)
            .cloned()
            .ok_or_else(|| Error::Content(format!("object index out of range in {asset}")))?;
        let kind = self.rules.classes.get(&object.class).ok_or_else(|| {
            Error::Content(format!(
                "unsupported scene class {} in {asset}",
                object.class
            ))
        })?;
        if let Some(kind) = kind {
            if self.kinds.len() >= MAX_SERIALIZERS {
                return Err(Error::Content("scene serializer bound".into()));
            }
            if self.rules.launchers.contains(&object.class) {
                self.launchers.push(self.kinds.len());
            }
            self.kinds.push(*kind);
        }
        // Composition order is independent of the field table's storage order.
        // Other references (event/property links, parent links, assets) are not
        // construction edges and must not contribute additional instances.
        for field in ["Objects", "Object", "Components", "Blueprint"] {
            if let Some(value) = object.fields.get(field) {
                match value {
                    Value::Pointer(pointer) => self.edge(asset, *pointer, depth)?,
                    Value::Array(values) => {
                        if values.len() > MAX_GRAPH_OBJECTS {
                            return Err(Error::Content("scene edge bound".into()));
                        }
                        for value in values {
                            self.edge(
                                asset,
                                value.as_pointer().ok_or_else(|| {
                                    Error::Content(format!("non-pointer scene field {field}"))
                                })?,
                                depth,
                            )?;
                        }
                    }
                    _ => return Err(Error::Content(format!("non-pointer scene field {field}"))),
                }
            }
        }
        self.active.remove(&key);
        Ok(())
    }

    fn edge(&mut self, source: &str, pointer: Pointer, depth: usize) -> Result<(), Error> {
        if let Some((name, index)) = self.assets.target(source, pointer)? {
            self.visit(&name, index, depth + 1)?;
        }
        Ok(())
    }
}

fn profile(assets: &mut Assets, rules: &Rules, name: &str) -> Result<Scene, Error> {
    let name = name.to_lowercase();
    let root = match assets.objects(&name)?.first().map(|o| o.class.as_str()) {
        Some("LevelData") => true,
        Some("SubWorldData") => false,
        _ => return Err(Error::Content(format!("{name} is not a scene"))),
    };
    let mut walk = Walk {
        assets,
        rules,
        active: BTreeSet::new(),
        visited: 0,
        kinds: vec![Kind::Noop],
        launchers: Vec::new(),
    };
    walk.visit(&name, 0, 0)?;
    Ok(Scene {
        profile: Profile::new(root, &walk.kinds)
            .map_err(|_| Error::Content("invalid scene profile".into()))?,
        launchers: walk.launchers,
    })
}

impl AssetIndex {
    /// Derive one LevelData or SubWorldData asset's server serializer order.
    pub fn scene(&mut self, name: &str) -> Result<Scene, Error> {
        profile(&mut self.assets, &self.identity.scenes, name)
    }
}

#[cfg(test)]
mod tests;
