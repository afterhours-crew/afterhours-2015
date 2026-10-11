// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Content generated from the player's installation on first start.
//!
//! `--game-dir` replaces `--item-content`, `--persistent-content` and
//! `--world-mac-template` with files `nfs-content` builds from the game and
//! caches (by default under `artifacts/content`). Later starts reuse the
//! verified cache entry; a patched installation gets a new one. Resolution
//! blocks and runs before the async runtime starts.
//! It also builds Stats definitions, license vocabulary, menu catalogs and reputation thresholds;
//! the CLI uses those unless overridden by operator configuration.
use nfs_content::{Kind, Options, Prepared};
use std::path::{Path, PathBuf};

/// Cache directory, relative to the deployment root, when none is given.
pub const DEFAULT_CACHE: &str = "artifacts/content";

/// Generated content paths and the cache entry they come from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generated {
    pub item_content: PathBuf,
    pub persistent_content: PathBuf,
    pub stat_definitions: PathBuf,
    pub item_licenses: PathBuf,
    pub control_catalogs: PathBuf,
    pub reputation_thresholds: PathBuf,
    pub scene_catalog: PathBuf,
    pub world_content: PathBuf,
    pub world_mac_template: PathBuf,
    pub prepared: Prepared,
}

impl Generated {
    /// Summary for the readiness line; contains no content.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "fingerprint": self.prepared.fingerprint.digest,
            "built": self.prepared.built,
            "directory": self.prepared.directory.display().to_string(),
        })
    }
}

/// Prepare content from `game_dir` into `cache` (default [`DEFAULT_CACHE`]).
/// Relative paths follow `base`.
pub fn resolve(
    base: &Path,
    game_dir: &Path,
    cache: Option<&Path>,
    options: &Options,
) -> Result<Generated, nfs_content::Error> {
    // The CLI resolves content paths against its root again when loading them.
    // Make generated paths absolute so relative deployment roots are applied once.
    let base = std::path::absolute(if base.as_os_str().is_empty() {
        Path::new(".")
    } else {
        base
    })?;
    let cache = base.join(cache.unwrap_or(Path::new(DEFAULT_CACHE)));
    let prepared = nfs_content::prepare(&base.join(game_dir), &cache, options)?;
    Ok(Generated {
        item_content: prepared.path(Kind::ItemContent),
        persistent_content: prepared.path(Kind::PersistentContent),
        stat_definitions: prepared.path(Kind::StatDefinitions),
        item_licenses: prepared.path(Kind::ItemLicenses),
        control_catalogs: prepared.path(Kind::ControlCatalogs),
        reputation_thresholds: prepared.path(Kind::ReputationThresholds),
        scene_catalog: prepared.path(Kind::SceneCatalog),
        world_content: prepared.path(Kind::WorldContent),
        world_mac_template: prepared.path(Kind::WorldMacTemplate),
        prepared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_handshake;
    use nfs_content::synthetic;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nfs-server-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn generated_content_loads_and_is_reused() {
        let dir = TempDir::new("install-content");
        let (options, _) = synthetic::write(&dir.0.join("game"), &synthetic::assets()).unwrap();
        let first = resolve(&dir.0, Path::new("game"), None, &options).unwrap();
        assert!(first.prepared.built);
        assert!(first.item_content.starts_with(dir.0.join(DEFAULT_CACHE)));
        let items = crate::item_content::ItemContent::load(&first.item_content).unwrap();
        assert!(items.inventory().is_some());
        crate::persistent::Catalog::load(&first.persistent_content).unwrap();
        nfs_services::stats::Catalog::load(&first.stat_definitions).unwrap();
        nfs_services::item_licenses::Content::load(&first.item_licenses).unwrap();
        nfs_services::control_catalogs::Catalog::load(&first.control_catalogs).unwrap();
        let scenes = nfs_content::scenes::Catalog::load(&first.scene_catalog).unwrap();
        let world =
            crate::content::WorldContent::load_with_scenes(&first.world_content, Some(&scenes))
                .unwrap();
        world.validate_runtime().unwrap();
        assert_eq!(std::fs::read(&first.world_mac_template).unwrap().len(), 64);
        let second = resolve(&dir.0, Path::new("game"), None, &options).unwrap();
        assert!(!second.prepared.built);
        assert_eq!(second.item_content, first.item_content);
        assert_eq!(second.summary()["built"], false);
        // The synthetic template is not the supported build's, so the
        // handshake's digest check rejects it.
        let bytes = std::fs::read(&first.world_mac_template).unwrap();
        assert!(world_handshake::validate_template(&bytes).is_err());
    }

    #[test]
    fn supported_identity_matches_the_handshake_template_digest() {
        let digest: String = world_handshake::TEMPLATE_HASH
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(digest, Options::default().identity.mac_template_sha256);
        assert_eq!(
            Options::default().identity.executable_sha256,
            nfs_services::SUPPORTED_BUILD_SHA256
        );
    }

    #[test]
    fn explicit_cache_and_unsupported_build_are_honoured() {
        let dir = TempDir::new("install-content-cache");
        let (options, _) = synthetic::write(&dir.0.join("game"), &synthetic::assets()).unwrap();
        let generated = resolve(
            &dir.0,
            Path::new("game"),
            Some(Path::new("elsewhere")),
            &options,
        )
        .unwrap();
        assert!(generated.item_content.starts_with(dir.0.join("elsewhere")));
        assert!(matches!(
            resolve(&dir.0, Path::new("game"), None, &Options::default()),
            Err(nfs_content::Error::UnsupportedExecutable { .. })
        ));
    }

    #[test]
    fn generated_paths_load_when_the_cli_applies_a_relative_root() {
        let dir = TempDir(PathBuf::from(format!(
            "target/install-relative-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
        let (options, _) = synthetic::write(&dir.0.join("game"), &synthetic::assets()).unwrap();
        for cache in [None, Some(Path::new("custom-cache"))] {
            let generated = resolve(&dir.0, Path::new("game"), cache, &options).unwrap();
            assert!(generated.item_content.is_absolute());
            nfs_services::control_catalogs::Catalog::load(&dir.0.join(generated.control_catalogs))
                .unwrap();
            crate::item_content::ItemContent::load(&dir.0.join(generated.item_content)).unwrap();
            crate::persistent::Catalog::load(&dir.0.join(generated.persistent_content)).unwrap();
            nfs_services::stats::Catalog::load(&dir.0.join(generated.stat_definitions)).unwrap();
            nfs_services::item_licenses::Content::load(&dir.0.join(generated.item_licenses))
                .unwrap();
            assert_eq!(
                std::fs::read(dir.0.join(generated.world_mac_template))
                    .unwrap()
                    .len(),
                64
            );
        }
    }
}
