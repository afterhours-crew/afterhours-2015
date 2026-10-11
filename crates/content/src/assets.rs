// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Asset identities the client derives from installed bundles.
//!
//! - A scene content key is the djb2-xor hash of the lower-cased bundle path
//!   without its `win32/` prefix.
//! - A bundle's asset catalog counts the Blueprint objects its
//!   `<bundle>_networkregistry_win32` asset lists, by runtime class ID
//!   (from the build profile), in ascending class ID order.
//! - An asset reference is a class ID plus the asset's position among that
//!   class's objects in registry order.
use crate::scan::Assets;
use crate::{BuildIdentity, Error, Options};
use nfs_frostbite::ebx::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const REGISTRY_SUFFIX: &str = "_networkregistry_win32";
const REGISTRY_CLASS: &str = "NetworkRegistryAsset";

/// Scene content key of a bundle path such as `levels/Genesis01/Garage`.
pub fn scene_key(bundle: &str) -> u32 {
    let lower = bundle.to_lowercase();
    nfs_frostbite::name_hash(lower.trim_start_matches("win32/").as_bytes())
}

/// A Blueprint asset's identity within one bundle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetReference {
    pub class_id: u32,
    pub local_index: u32,
}

/// Blueprint asset names of one bundle registry, grouped by class in order.
type Groups = BTreeMap<String, Vec<String>>;

/// Read access to an installation's bundle registries.
pub struct AssetIndex {
    assets: Assets,
    identity: BuildIdentity,
    registries: HashMap<String, Groups>,
}

impl AssetIndex {
    /// Open and index an installation after checking its executable.
    pub fn open(game_root: &Path, options: &Options) -> Result<Self, Error> {
        crate::fingerprint(game_root, options)?;
        let install = nfs_frostbite::install::Installation::open(game_root, options.limits)?;
        Ok(Self {
            assets: Assets::scan(&install)?,
            identity: options.identity.clone(),
            registries: HashMap::new(),
        })
    }

    fn groups(&mut self, bundle: &str) -> Result<&Groups, Error> {
        let key = bundle.to_lowercase();
        if !self.registries.contains_key(&key) {
            let registry = format!("{}{REGISTRY_SUFFIX}", key.trim_start_matches("win32/"));
            let object = self
                .assets
                .objects(&registry)
                .map_err(|_| Error::Content(format!("bundle {bundle} has no network registry")))?
                .first()
                .cloned()
                .filter(|o| o.class == REGISTRY_CLASS)
                .ok_or_else(|| Error::Content(format!("{registry} is not a network registry")))?;
            let mut groups = Groups::new();
            for value in object
                .fields
                .get("Objects")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Content(format!("{registry} lists no objects")))?
            {
                let (name, target) = self.assets.resolve(value.as_pointer())?;
                if target.class.ends_with("Blueprint") {
                    groups.entry(target.class).or_default().push(name);
                }
            }
            self.registries.insert(key.clone(), groups);
        }
        Ok(&self.registries[&key])
    }

    /// Decoded objects of an asset (owned, so callers may keep querying).
    pub(crate) fn objects(
        &mut self,
        asset: &str,
    ) -> Result<Vec<nfs_frostbite::ebx::Object>, Error> {
        Ok(self.assets.objects(asset)?.to_vec())
    }

    /// Asset name and object an import pointer value names.
    pub(crate) fn resolve_import(
        &mut self,
        value: Option<&Value>,
    ) -> Result<(String, nfs_frostbite::ebx::Object), Error> {
        self.assets.resolve(value.and_then(Value::as_pointer))
    }

    fn class_id(&self, class: &str) -> Result<u32, Error> {
        self.identity
            .blueprint_class_ids
            .get(class)
            .copied()
            .ok_or_else(|| {
                Error::Content(format!(
                    "no runtime class ID for {class} in this build profile"
                ))
            })
    }

    /// `(class ID, count)` pairs of a bundle's Blueprint objects, by class ID.
    pub fn catalog(&mut self, bundle: &str) -> Result<Vec<(u32, u32)>, Error> {
        let groups: Vec<(String, usize)> = self
            .groups(bundle)?
            .iter()
            .map(|(class, names)| (class.clone(), names.len()))
            .collect();
        let mut out = groups
            .into_iter()
            .map(|(class, count)| Ok((self.class_id(&class)?, count as u32)))
            .collect::<Result<Vec<_>, Error>>()?;
        out.sort_unstable();
        Ok(out)
    }

    /// Reference of a Blueprint asset (case-insensitive name) within `bundle`.
    pub fn reference(&mut self, bundle: &str, asset: &str) -> Result<AssetReference, Error> {
        let wanted = asset.to_lowercase();
        let found = self.groups(bundle)?.iter().find_map(|(class, names)| {
            names
                .iter()
                .position(|n| n.to_lowercase() == wanted)
                .map(|i| (class.clone(), i))
        });
        let (class, index) = found
            .ok_or_else(|| Error::Content(format!("{asset} is not registered in {bundle}")))?;
        Ok(AssetReference {
            class_id: self.class_id(&class)?,
            local_index: index as u32,
        })
    }

    /// Asset name a reference names within `bundle`.
    pub fn resolve(&mut self, bundle: &str, reference: AssetReference) -> Result<String, Error> {
        let class = self
            .identity
            .blueprint_class_ids
            .iter()
            .find(|(_, id)| **id == reference.class_id)
            .map(|(c, _)| c.clone())
            .ok_or_else(|| Error::Content(format!("unknown class ID {}", reference.class_id)))?;
        self.groups(bundle)?
            .get(&class)
            .and_then(|names| names.get(reference.local_index as usize))
            .cloned()
            .ok_or_else(|| Error::Content(format!("reference out of range in {bundle}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn index() -> (TempDir, AssetIndex) {
        let dir = TempDir(std::env::temp_dir().join(format!(
            "nfs-content-assets-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )));
        let _ = std::fs::remove_dir_all(&dir.0);
        let (options, _) = synthetic::write(&dir.0, &synthetic::assets()).unwrap();
        let index = AssetIndex::open(&dir.0, &options).unwrap();
        (dir, index)
    }

    #[test]
    fn scene_keys_hash_lower_case_paths_without_platform_prefix() {
        assert_eq!(scene_key("levels/Genesis01/Gameplay"), 1_286_940_107);
        assert_eq!(scene_key("win32/levels/genesis01/gameplay"), 1_286_940_107);
    }

    #[test]
    fn catalog_counts_registry_blueprints_by_class_id() {
        let (_dir, mut index) = index();
        assert_eq!(
            index.catalog(synthetic::REGISTRY_BUNDLE).unwrap(),
            [(10, 2), (20, 1)]
        );
        // Bundle names are case-insensitive.
        assert_eq!(
            index
                .catalog(&synthetic::REGISTRY_BUNDLE.to_uppercase())
                .unwrap(),
            [(10, 2), (20, 1)]
        );
    }

    #[test]
    fn references_follow_registry_order_and_round_trip() {
        let (_dir, mut index) = index();
        let bundle = synthetic::REGISTRY_BUNDLE;
        let second = index.reference(bundle, "Prefabs/Second").unwrap();
        assert_eq!(
            second,
            AssetReference {
                class_id: 10,
                local_index: 1
            }
        );
        assert_eq!(index.resolve(bundle, second).unwrap(), "prefabs/second");
        let vehicle = index.reference(bundle, "vehicles/synthetic").unwrap();
        assert_eq!(vehicle.local_index, 0);
        assert!(index.reference(bundle, "prefabs/absent").is_err());
        assert!(
            index
                .resolve(
                    bundle,
                    AssetReference {
                        class_id: 10,
                        local_index: 2
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn unknown_classes_and_missing_registries_fail() {
        let (_dir, mut index) = index();
        assert!(matches!(
            index.catalog(synthetic::UNPROFILED_BUNDLE),
            Err(Error::Content(_))
        ));
        assert!(index.catalog("levels/absent").is_err());
    }
}
