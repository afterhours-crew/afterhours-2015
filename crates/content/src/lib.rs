// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Builds deployment content from a player's own game installation, once, and
//! caches it. No game content is distributed with the server.
//!
//! [`prepare`] checks that the executable is the supported build, fingerprints
//! the installation's container indexes and returns `<cache>/<fingerprint>/`
//! when its manifest and every output verify. Otherwise it reads the
//! installation, builds every output in memory, validates each through the
//! loader that will consume it, writes a temporary directory and publishes it
//! with one rename. A patched or repaired installation yields a new
//! fingerprint and therefore a new entry.
//!
//! The library is synchronous and owns no runtime: a server or launcher calls
//! it before starting, or from a blocking worker.
pub mod ai;
pub mod assets;
pub mod builds;
mod cache;
mod control;
pub mod events;
mod executable;
mod fingerprint;
pub mod garage;
mod graph;
mod items;
mod licenses;
pub mod logic_events;
pub mod nitrous;
pub mod participant_logic;
mod progress;
pub mod progression;
pub mod prologue;
mod reputation;
pub mod rivals;
pub mod road_network;
mod scan;
pub mod scenes;
pub mod scoring;
pub mod sequences;
mod startup;
mod stats;
#[cfg(any(test, feature = "synthetic"))]
pub mod synthetic;
mod tables;
pub mod traffic;
pub use progress::Progress;
pub mod vehicle_parts;
pub mod vehicle_profiles;
pub mod world;
mod world_services;

pub use fingerprint::{Fingerprint, Input, InputKind, fingerprint};
pub use nfs_frostbite::Limits;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Manifest format written beside the outputs.
pub const FORMAT: &str = "nfs-install-content";
pub const VERSION: u64 = 1;
/// Increment whenever a builder's output for an unchanged installation changes,
/// so existing cache entries are rebuilt.
pub const BUILDER_REVISION: u64 = 39;
/// Executable file name relative to the installation root.
pub const EXECUTABLE: &str = "NFS16.exe";

/// Build-specific identifiers of one executable (see `builds/`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildIdentity {
    pub executable_sha256: String,
    /// Relative virtual address of the 64-byte world MAC template.
    pub mac_template_rva: u32,
    pub mac_template_sha256: String,
    /// Runtime class IDs of Blueprint classes, by class name. The catalog uses them
    /// only to order a bundle registry; they are never sent. All are observed except
    /// SpatialPrefabBlueprint, whose ID is inferred from that order (E995: every
    /// decoded asset index puts it between LogicPrefab and GameMode).
    pub blueprint_class_ids: BTreeMap<String, u32>,
    /// Server scene construction rules tied to this executable.
    pub scenes: scenes::Rules,
    pub garage_links: garage::Links,
    pub vehicles: vehicle_profiles::Rules,
    pub progression: progression::Rules,
    pub prologue_links: prologue::Links,
    pub sequences: sequences::Rules,
    pub participant_logic: participant_logic::Rules,
    pub traffic_spline_class_id: u16,
    pub traffic_links: traffic::Links,
    pub rival_links: rivals::Links,
    pub ai_links: ai::Links,
}

impl BuildIdentity {
    /// The profile of the build the content loaders accept
    /// (`nfs_services::SUPPORTED_BUILD_SHA256`). The template bytes are read
    /// from the player's executable; only their address and digest are known.
    pub fn supported() -> Self {
        // Embedded profiles are validated by the builds tests.
        builds::profile_for(nfs_services::SUPPORTED_BUILD_SHA256)
            .ok()
            .flatten()
            .expect("the supported build has a valid embedded profile")
    }
}

/// One generated output.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    /// `nfs-item-definitions` version 2: definitions, defaults and starter list.
    ItemContent,
    /// Persistent-table schemas for `nfs_services::persistent::Catalog`.
    PersistentContent,
    /// Stats group definitions for the local progression and reputation stores.
    StatDefinitions,
    /// Known client license names; no account grants or awards.
    ItemLicenses,
    /// Installed Stats scopes and default AllDrive-only menu policy.
    ControlCatalogs,
    /// Level thresholds used by reputation, Stats and awards.
    ReputationThresholds,
    /// Installed scene serializer profiles and launcher endpoints.
    SceneCatalog,
    /// Startup descriptor, registrations and role bindings with AllDrive policy.
    WorldContent,
    /// Garage slots, parking transforms and inventory slot bindings.
    GarageLayout,
    /// The 64-byte world MAC template.
    WorldMacTemplate,
    /// Vehicle customization, wheel, wrap, NOS and mesh item definitions.
    VehicleParts,
    /// Vehicle composition, static physics, appearance, mesh and health inputs.
    VehicleProfiles,
    /// Objective defaults, entity shapes and installed scene-channel bindings.
    ProgressionContent,
    SequenceContent,
    TrafficContent,
    RivalContent,
}

impl Kind {
    pub const ALL: [Kind; 16] = [
        Kind::ItemContent,
        Kind::PersistentContent,
        Kind::StatDefinitions,
        Kind::ItemLicenses,
        Kind::ControlCatalogs,
        Kind::ReputationThresholds,
        Kind::SceneCatalog,
        Kind::WorldContent,
        Kind::GarageLayout,
        Kind::WorldMacTemplate,
        Kind::VehicleParts,
        Kind::VehicleProfiles,
        Kind::ProgressionContent,
        Kind::SequenceContent,
        Kind::TrafficContent,
        Kind::RivalContent,
    ];
    /// Manifest key.
    pub fn key(self) -> &'static str {
        match self {
            Self::ItemContent => "item_content",
            Self::PersistentContent => "persistent_content",
            Self::StatDefinitions => "stat_definitions",
            Self::ItemLicenses => "item_licenses",
            Self::ControlCatalogs => "control_catalogs",
            Self::ReputationThresholds => "reputation_thresholds",
            Self::SceneCatalog => "scene_catalog",
            Self::WorldContent => "world_content",
            Self::GarageLayout => "garage_layout",
            Self::WorldMacTemplate => "world_mac_template",
            Self::VehicleParts => "vehicle_parts",
            Self::VehicleProfiles => "vehicle_profiles",
            Self::ProgressionContent => "progression_content",
            Self::SequenceContent => "sequence_content",
            Self::TrafficContent => "traffic_content",
            Self::RivalContent => "rival_content",
        }
    }
    /// File name inside a cache entry.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::ItemContent => "item-content.json",
            Self::PersistentContent => "persistent-content.json",
            Self::StatDefinitions => "stat-definitions.json",
            Self::ItemLicenses => "item-licenses.json",
            Self::ControlCatalogs => "control-catalogs.json",
            Self::ReputationThresholds => "reputation-thresholds.json",
            Self::SceneCatalog => "scene-catalog.json",
            Self::WorldContent => "world-content.json",
            Self::GarageLayout => "garage-layout.json",
            Self::WorldMacTemplate => "world-mac-template.bin",
            Self::VehicleParts => "vehicle-parts.json",
            Self::VehicleProfiles => "vehicle-profiles.json",
            Self::ProgressionContent => "progression-content.json",
            Self::SequenceContent => "sequence-content.json",
            Self::TrafficContent => "traffic-content.json",
            Self::RivalContent => "rival-content.json",
        }
    }
}

/// Build policy. [`Options::default`] targets the supported build.
#[derive(Clone, Debug)]
pub struct Options {
    pub identity: BuildIdentity,
    pub limits: Limits,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            identity: BuildIdentity::supported(),
            limits: Limits::default(),
        }
    }
}

/// A verified cache entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prepared {
    pub directory: PathBuf,
    pub fingerprint: Fingerprint,
    /// Whether this call built the entry (false when an existing one verified).
    pub built: bool,
}

impl Prepared {
    pub fn path(&self, kind: Kind) -> PathBuf {
        self.directory.join(kind.file_name())
    }
}

/// Preparation failures. Messages never include file contents.
#[derive(Debug)]
pub enum Error {
    /// The installation's executable is not the build this content targets.
    UnsupportedExecutable {
        sha256: String,
    },
    /// A container, record or partition could not be read.
    Container(nfs_frostbite::Error),
    /// Installed content lacks an expected asset or field.
    Content(String),
    Io(std::io::Error),
    /// A cache entry could not be verified, replaced or published.
    Cache(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedExecutable { sha256 } => {
                write!(f, "unsupported game executable (SHA-256 {sha256})")
            }
            Self::Container(error) => write!(f, "{error}"),
            Self::Content(what) => write!(f, "installed content: {what}"),
            Self::Io(error) => write!(f, "content cache I/O: {error}"),
            Self::Cache(what) => write!(f, "content cache: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Container(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<nfs_frostbite::Error> for Error {
    fn from(error: nfs_frostbite::Error) -> Self {
        Self::Container(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Return a verified cache entry for the installation at `game_root`,
/// building it under `cache_root` when absent or invalid.
pub fn prepare(game_root: &Path, cache_root: &Path, options: &Options) -> Result<Prepared, Error> {
    prepare_with_progress(game_root, cache_root, options, &mut |_| {})
}

/// Prepare with metadata-only progress. Completion is reported only after
/// the corresponding work succeeds; a cache hit never reports extraction.
pub fn prepare_with_progress(
    game_root: &Path,
    cache_root: &Path,
    options: &Options,
    report: &mut dyn FnMut(Progress),
) -> Result<Prepared, Error> {
    report(Progress::Fingerprinting);
    let fingerprint = fingerprint(game_root, options)?;
    let prepared = cache::prepare(
        cache_root,
        fingerprint,
        |fingerprint, report| build_with_progress(game_root, fingerprint, options, report),
        report,
    )?;
    report(Progress::Ready);
    Ok(prepared)
}

/// Build every output in memory without touching a cache.
pub fn build(
    game_root: &Path,
    fingerprint: &Fingerprint,
    options: &Options,
) -> Result<BTreeMap<Kind, Vec<u8>>, Error> {
    build_with_progress(game_root, fingerprint, options, &mut |_| {})
}

fn build_with_progress(
    game_root: &Path,
    fingerprint: &Fingerprint,
    options: &Options,
    report: &mut dyn FnMut(Progress),
) -> Result<BTreeMap<Kind, Vec<u8>>, Error> {
    report(Progress::Scanning);
    let install = nfs_frostbite::install::Installation::open(game_root, options.limits)?;
    let mut assets = scan::Assets::scan(&install)?;
    report(Progress::Scanned);
    let mut outputs = BTreeMap::new();
    macro_rules! extract {
        ($kind:expr, $work:expr) => {{
            let kind = $kind;
            report(Progress::Extracting(kind));
            let bytes = $work?;
            report(Progress::Extracted {
                kind,
                bytes: bytes.len(),
            });
            bytes
        }};
    }
    outputs.insert(
        Kind::ItemContent,
        extract!(Kind::ItemContent, items::build(&mut assets)),
    );
    let item_content = nfs_services::item_content::ItemContent::from_json(
        &serde_json::from_slice(&outputs[&Kind::ItemContent])
            .map_err(|_| Error::Content("item JSON".into()))?,
    )
    .map_err(|_| Error::Content("item catalog".into()))?;
    outputs.insert(
        Kind::VehicleParts,
        extract!(
            Kind::VehicleParts,
            vehicle_parts::build(&mut assets, &item_content.catalog)
        ),
    );
    let tables = extract!(
        Kind::PersistentContent,
        tables::build(&mut assets, &fingerprint.digest)
    );
    outputs.insert(
        Kind::StatDefinitions,
        extract!(Kind::StatDefinitions, stats::build(&tables)),
    );
    outputs.insert(Kind::PersistentContent, tables);
    outputs.insert(
        Kind::ItemLicenses,
        extract!(Kind::ItemLicenses, licenses::build(&mut assets)),
    );
    outputs.insert(
        Kind::ControlCatalogs,
        extract!(Kind::ControlCatalogs, control::build(&mut assets)),
    );
    outputs.insert(
        Kind::ReputationThresholds,
        extract!(Kind::ReputationThresholds, reputation::build(&mut assets)),
    );
    outputs.insert(
        Kind::SceneCatalog,
        extract!(
            Kind::SceneCatalog,
            scenes::build(&mut assets, &options.identity.scenes)
        ),
    );
    let catalog = scenes::Catalog::from_json(
        &serde_json::from_slice(&outputs[&Kind::SceneCatalog])
            .map_err(|_| Error::Content("scene catalog JSON".into()))?,
    )?;
    let mut index = assets::AssetIndex::from_assets(assets, options.identity.clone());
    outputs.insert(
        Kind::WorldContent,
        extract!(Kind::WorldContent, startup::build(&mut index, &catalog)),
    );
    outputs.insert(
        Kind::GarageLayout,
        extract!(Kind::GarageLayout, garage::build(&mut index)),
    );
    outputs.insert(
        Kind::ProgressionContent,
        extract!(
            Kind::ProgressionContent,
            progression::build(&mut index, &fingerprint.digest)
        ),
    );
    outputs.insert(
        Kind::VehicleProfiles,
        extract!(
            Kind::VehicleProfiles,
            vehicle_profiles::build(&mut index, &item_content.catalog)
        ),
    );
    outputs.insert(
        Kind::WorldMacTemplate,
        extract!(
            Kind::WorldMacTemplate,
            executable::mac_template(&game_root.join(EXECUTABLE), &options.identity)
        ),
    );
    outputs.insert(
        Kind::SequenceContent,
        extract!(Kind::SequenceContent, sequences::build(&mut index)),
    );
    outputs.insert(
        Kind::TrafficContent,
        extract!(Kind::TrafficContent, traffic::build(&mut index)),
    );
    outputs.insert(
        Kind::RivalContent,
        extract!(Kind::RivalContent, rivals::build(&mut index)),
    );
    Ok(outputs)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    nfs_frostbite::hex(&sha2::Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
