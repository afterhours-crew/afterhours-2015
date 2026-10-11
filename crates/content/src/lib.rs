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
pub mod assets;
pub mod builds;
mod cache;
mod executable;
mod fingerprint;
mod items;
mod scan;
#[cfg(any(test, feature = "synthetic"))]
pub mod synthetic;
mod tables;
pub mod world;

pub use fingerprint::{Fingerprint, Input, fingerprint};
pub use nfs_frostbite::Limits;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Manifest format written beside the outputs.
pub const FORMAT: &str = "nfs-install-content";
pub const VERSION: u64 = 1;
/// Increment whenever a builder's output for an unchanged installation changes,
/// so existing cache entries are rebuilt.
pub const BUILDER_REVISION: u64 = 1;
/// Executable file name relative to the installation root.
pub const EXECUTABLE: &str = "NFS16.exe";

/// Build-specific identifiers of one executable (see `builds/`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildIdentity {
    pub executable_sha256: String,
    /// Relative virtual address of the 64-byte world MAC template.
    pub mac_template_rva: u32,
    pub mac_template_sha256: String,
    /// Runtime class IDs of Blueprint classes, by class name.
    pub blueprint_class_ids: BTreeMap<String, u32>,
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
    /// The 64-byte world MAC template.
    WorldMacTemplate,
}

impl Kind {
    pub const ALL: [Kind; 3] = [
        Kind::ItemContent,
        Kind::PersistentContent,
        Kind::WorldMacTemplate,
    ];
    /// Manifest key.
    pub fn key(self) -> &'static str {
        match self {
            Self::ItemContent => "item_content",
            Self::PersistentContent => "persistent_content",
            Self::WorldMacTemplate => "world_mac_template",
        }
    }
    /// File name inside a cache entry.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::ItemContent => "item-content.json",
            Self::PersistentContent => "persistent-content.json",
            Self::WorldMacTemplate => "world-mac-template.bin",
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
    let fingerprint = fingerprint(game_root, options)?;
    cache::prepare(cache_root, fingerprint, |fingerprint| {
        build(game_root, fingerprint, options)
    })
}

/// Build every output in memory without touching a cache.
pub fn build(
    game_root: &Path,
    fingerprint: &Fingerprint,
    options: &Options,
) -> Result<BTreeMap<Kind, Vec<u8>>, Error> {
    let install = nfs_frostbite::install::Installation::open(game_root, options.limits)?;
    let mut assets = scan::Assets::scan(&install)?;
    let mut outputs = BTreeMap::new();
    outputs.insert(Kind::ItemContent, items::build(&mut assets)?);
    outputs.insert(
        Kind::PersistentContent,
        tables::build(&mut assets, &fingerprint.digest)?,
    );
    outputs.insert(
        Kind::WorldMacTemplate,
        executable::mac_template(&game_root.join(EXECUTABLE), &options.identity)?,
    );
    Ok(outputs)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    nfs_frostbite::hex(&sha2::Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
