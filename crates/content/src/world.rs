// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Startup sub-levels: the names a world host lists and registers when a level
//! starts, derived from the level's data.
//!
//! - Sub-levels: `SubWorldReferenceObjectData` entries with `AutoLoad` and
//!   `IsWin32SubLevel`, walked breadth-first from the level, siblings in the
//!   order the parent scene's root lists them in its `Objects` array, each
//!   with its parent (`None` for the level root).
//! - Then bundle preloads (`BundlePreLoadEntityData.BundlesToLoad`) and
//!   blueprint bundles (`BlueprintBundleEntityData.Bundle`) found in the layers
//!   (`WorldPartReferenceObjectData`) of those scenes, in the same scene order;
//!   both are children of the root.
//!
//! The sibling order is not a free choice. The client builds the auto-loaded
//! sub-levels in registration order, and entities resolve their cross-scene
//! links once, when their own scene is built: the music director in
//! `levels/Genesis01/Audio` attaches to garage states in
//! `Levels/Genesis01/ParticipantStateMachine` through a link declared in their
//! parent, `levels/Genesis01/Gameplay`, and finds nothing when Audio is built
//! first. The official host registers siblings in the parent root's `Objects`
//! order with the non-auto-loaded references skipped (the eight Genesis01 root
//! children and the twenty Gameplay children both match it), which puts the
//! state machine before Audio; the partition's object order does not.
use crate::Error;
use crate::assets::AssetIndex;
use nfs_frostbite::ebx::{Object, Pointer, Value};
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

fn is_sub_level(object: &Object) -> bool {
    object.class == "SubWorldReferenceObjectData"
        && flag(object, "AutoLoad")
        && flag(object, "IsWin32SubLevel")
}

/// Indexes of a scene's auto-loaded sub-world references in the order the
/// scene's root object lists them (`Objects`), which is the order the official
/// host registers them in. A qualifying reference the root does not list is a
/// content shape this builder does not understand.
fn sub_level_order(asset: &str, objects: &[Object]) -> Result<Vec<usize>, Error> {
    let root = objects
        .first()
        .ok_or_else(|| Error::Content(format!("{asset}: empty scene")))?;
    let listed = root
        .fields
        .get("Objects")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Content(format!("{asset}: scene root lists no objects")))?;
    let mut order = Vec::new();
    for value in listed {
        if let Value::Pointer(Pointer::Internal(index)) = value
            && objects.get(*index).is_some_and(is_sub_level)
            && !order.contains(index)
        {
            order.push(*index);
        }
    }
    if objects
        .iter()
        .enumerate()
        .any(|(index, object)| is_sub_level(object) && !order.contains(&index))
    {
        return Err(Error::Content(format!(
            "{asset}: a sub-level reference is outside the scene's object list"
        )));
    }
    Ok(order)
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
    /// Installed startup order under the selected level.
    pub fn startup_entries(&mut self, level: &str) -> Result<Vec<StartupEntry>, Error> {
        startup_entries(&mut self.assets, level)
    }
}

/// Startup entries of the level whose `LevelData` asset is `level`
/// (for example `levels/Genesis01/Genesis01`).
pub(crate) fn startup_entries(
    assets: &mut crate::scan::Assets,
    level: &str,
) -> Result<Vec<StartupEntry>, Error> {
    let root = level.to_lowercase();
    let root_class = assets.objects(&root)?.first().map(|o| o.class.clone());
    if root_class.as_deref() != Some("LevelData") {
        return Err(Error::Content(format!("{level} is not a level")));
    }
    let mut entries: Vec<StartupEntry> = Vec::new();
    let mut scenes = Vec::new();
    let mut visited = BTreeSet::from([root.clone()]);
    let mut queue = VecDeque::from([(root, None)]);
    while let Some((asset, me)) = queue.pop_front() {
        scenes.push(asset.clone());
        let objects = assets.objects(&asset)?.to_vec();
        for index in sub_level_order(&asset, &objects)? {
            let object = &objects[index];
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
        for object in assets.objects(scene)?.to_vec() {
            if object.class != "WorldPartReferenceObjectData" {
                continue;
            }
            let (layer, _) =
                assets.resolve(object.fields.get("Blueprint").and_then(Value::as_pointer))?;
            for item in assets.objects(&layer)? {
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

    #[test]
    fn siblings_follow_the_parent_root_object_list_not_the_partition_order() {
        let dir =
            std::env::temp_dir().join(format!("nfs-content-world-listed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut assets = synthetic::assets();
        assets.retain(|a| a.name != synthetic::LEVEL.to_lowercase());
        assets.extend(
            synthetic::level_listed_out_of_object_order()
                .into_iter()
                .filter(|a| a.name == synthetic::LEVEL.to_lowercase()),
        );
        let (options, _) = synthetic::write(&dir, &assets).unwrap();
        let mut index = AssetIndex::open(&dir, &options).unwrap();
        let entries = index.startup_entries(synthetic::LEVEL).unwrap();
        let sub_levels: Vec<(&str, Option<usize>)> = entries
            .iter()
            .filter(|e| e.kind == StartupKind::SubLevel)
            .map(|e| (e.name.as_str(), e.parent))
            .collect();
        assert_eq!(
            sub_levels,
            [
                ("levels/Synth/Beta", None),
                ("Levels/Synth/Alpha", None),
                ("Levels/Synth/Gamma", Some(1)),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_auto_loaded_sub_level_outside_the_object_list_is_refused() {
        let dir =
            std::env::temp_dir().join(format!("nfs-content-world-unlisted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut assets = synthetic::assets();
        assets.retain(|a| a.name != synthetic::LEVEL.to_lowercase());
        assets.extend(
            synthetic::level_with_unlisted_sub_level()
                .into_iter()
                .filter(|a| a.name == synthetic::LEVEL.to_lowercase()),
        );
        let (options, _) = synthetic::write(&dir, &assets).unwrap();
        let mut index = AssetIndex::open(&dir, &options).unwrap();
        let error = index.startup_entries(synthetic::LEVEL).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outside the scene's object list"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
