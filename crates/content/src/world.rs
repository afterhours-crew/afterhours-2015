// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Startup sub-levels: the names a world host lists and registers when a level
//! starts, derived from the level's data.
//!
//! - Sub-levels: `SubWorldReferenceObjectData` entries with `AutoLoad` and
//!   `IsWin32SubLevel`, walked breadth-first from the level, siblings in object
//!   order, each with its parent (`None` for the level root).
//! - Then bundle preloads (`BundlePreLoadEntityData.BundlesToLoad`) and
//!   blueprint bundles (`BlueprintBundleEntityData.Bundle`) found in the layers
//!   (`WorldPartReferenceObjectData`) of those scenes, in the same scene order;
//!   both are children of the root.
//!
//! The client accepts any sibling order that is used consistently, so this
//! order is a choice, not a reproduction of another host's order.
use crate::Error;
use crate::assets::AssetIndex;
use nfs_frostbite::ebx::{Object, Value};
use std::collections::{BTreeSet, VecDeque};

/// Most startup entries a level may produce.
pub const MAX_STARTUP_ENTRIES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupKind {
    SubLevel,
    PreloadBundle,
    BlueprintBundle,
}

/// One listed name; `parent` indexes an earlier sub-level entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupEntry {
    pub name: String,
    pub parent: Option<usize>,
    pub kind: StartupKind,
}

fn flag(object: &Object, field: &str) -> bool {
    object.fields.get(field).and_then(Value::as_bool) == Some(true)
}

fn names_of(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Struct(fields)) => fields
            .get("Name")
            .and_then(Value::as_str)
            .map(|n| vec![n.to_owned()])
            .unwrap_or_default(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_fields()?.get("Name")?.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

impl AssetIndex {
    /// Startup entries of the level whose `LevelData` asset is `level`
    /// (for example `levels/Genesis01/Genesis01`).
    pub fn startup_entries(&mut self, level: &str) -> Result<Vec<StartupEntry>, Error> {
        let root = level.to_lowercase();
        let root_class = self.objects(&root)?.first().map(|o| o.class.clone());
        if root_class.as_deref() != Some("LevelData") {
            return Err(Error::Content(format!("{level} is not a level")));
        }
        let mut entries: Vec<StartupEntry> = Vec::new();
        let mut scenes = Vec::new();
        let mut visited = BTreeSet::from([root.clone()]);
        let mut queue = VecDeque::from([(root, None)]);
        while let Some((asset, me)) = queue.pop_front() {
            scenes.push(asset.clone());
            for object in self.objects(&asset)? {
                if object.class != "SubWorldReferenceObjectData"
                    || !flag(&object, "AutoLoad")
                    || !flag(&object, "IsWin32SubLevel")
                {
                    continue;
                }
                let name = object
                    .fields
                    .get("BundleName")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| Error::Content(format!("{asset}: sub-level without a name")))?
                    .to_owned();
                let key = name.to_lowercase();
                if !visited.insert(key.clone()) {
                    return Err(Error::Content(format!("{name} is referenced twice")));
                }
                entries.push(StartupEntry {
                    name,
                    parent: me,
                    kind: StartupKind::SubLevel,
                });
                if entries.len() > MAX_STARTUP_ENTRIES {
                    return Err(Error::Content("too many startup entries".into()));
                }
                queue.push_back((key, Some(entries.len() - 1)));
            }
        }
        let (mut preloads, mut blueprints) = (Vec::new(), Vec::new());
        for scene in &scenes {
            for object in self.objects(scene)? {
                if object.class != "WorldPartReferenceObjectData" {
                    continue;
                }
                let (layer, _) = self.resolve_import(object.fields.get("Blueprint"))?;
                for item in self.objects(&layer)? {
                    match item.class.as_str() {
                        "BundlePreLoadEntityData" => {
                            preloads.extend(names_of(item.fields.get("BundlesToLoad")))
                        }
                        "BlueprintBundleEntityData" => {
                            blueprints.extend(names_of(item.fields.get("Bundle")))
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut seen: BTreeSet<String> = entries.iter().map(|e| e.name.to_lowercase()).collect();
        for (names, kind) in [
            (preloads, StartupKind::PreloadBundle),
            (blueprints, StartupKind::BlueprintBundle),
        ] {
            for name in names {
                if seen.insert(name.to_lowercase()) {
                    entries.push(StartupEntry {
                        name,
                        parent: None,
                        kind,
                    });
                }
            }
        }
        if entries.len() > MAX_STARTUP_ENTRIES {
            return Err(Error::Content("too many startup entries".into()));
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;

    #[test]
    fn sub_levels_are_breadth_first_then_preloads_then_blueprint_bundles() {
        let dir = std::env::temp_dir().join(format!("nfs-content-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (options, _) = synthetic::write(&dir, &synthetic::assets()).unwrap();
        let mut index = AssetIndex::open(&dir, &options).unwrap();
        let entries = index.startup_entries(synthetic::LEVEL).unwrap();
        let summary: Vec<(&str, Option<usize>, StartupKind)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.parent, e.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("Levels/Synth/Alpha", None, StartupKind::SubLevel),
                ("levels/Synth/Beta", None, StartupKind::SubLevel),
                ("Levels/Synth/Gamma", Some(0), StartupKind::SubLevel),
                (
                    "vehicles/Shared/Parts_Bundle",
                    None,
                    StartupKind::PreloadBundle
                ),
                (
                    "Vehicles/Traffic/One_Bundle",
                    None,
                    StartupKind::BlueprintBundle
                ),
                (
                    "Vehicles/Traffic/Two_Bundle",
                    None,
                    StartupKind::BlueprintBundle
                ),
            ]
        );
        assert!(index.startup_entries("levels/synth/alpha").is_err());
        assert!(index.startup_entries("levels/absent").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
