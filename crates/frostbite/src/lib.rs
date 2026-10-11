// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only access to an installed game's Frostbite 2014.4 content.
//!
//! The chain is `Data/layout.toc` → per-superbundle `.toc` → `.sb` bundle
//! manifests → `cas.cat` → `cas_NN.cas` block records (stored, zlib or LZ4,
//! optionally rewritten by a delta stream) → self-describing EBX partitions.
//! Every reader is bounded by [`Limits`]; nothing is written to the
//! installation. The layouts were recovered from the supported installation;
//! no third-party implementation is included.
pub mod cas;
pub mod dbobject;
pub mod ebx;
pub mod install;
#[cfg(any(test, feature = "synthetic"))]
pub mod synthetic;

/// djb2-xor (`h = h * 33 ^ byte`, seed 5381) used for EBX type and field
/// names, persistent-table names and (over lower-cased paths) bundle keys.
pub fn name_hash(bytes: &[u8]) -> u32 {
    bytes.iter().fold(5381u32, |hash, byte| {
        hash.wrapping_mul(33) ^ u32::from(*byte)
    })
}

/// Resource bounds applied to every container, record and partition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Largest DbObject container (layout, TOC, one bundle manifest, catalog).
    pub max_container_bytes: usize,
    /// Largest decompressed asset and largest raw cas record.
    pub max_asset_bytes: usize,
    /// Deepest DbObject or EBX nesting.
    pub max_depth: usize,
    /// Most elements in one DbObject container or EBX array.
    pub max_elements: usize,
    /// Most decoded EBX values in one partition.
    pub max_values: usize,
    /// Most bundles across all superbundles.
    pub max_bundles: usize,
    /// Most EBX entries across all bundles.
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_container_bytes: 64 << 20,
            max_asset_bytes: 64 << 20,
            max_depth: 64,
            max_elements: 1 << 20,
            max_values: 4 << 20,
            max_bundles: 100_000,
            max_entries: 1_000_000,
        }
    }
}

/// Reader failures. Messages name the violated rule, never file contents.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// Input does not follow the expected layout.
    Malformed(&'static str),
    /// Input exceeds a configured [`Limits`] bound.
    Bound(&'static str),
    /// A valid but unimplemented variant (for example an unknown codec).
    Unsupported(&'static str),
    /// A referenced file, record or asset is absent.
    Missing(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "content I/O: {error}"),
            Self::Malformed(what) => write!(f, "malformed content: {what}"),
            Self::Bound(what) => write!(f, "content bound exceeded: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported content: {what}"),
            Self::Missing(what) => write!(f, "missing content: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Lower-case hexadecimal, used for GUID and digest text.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hash_matches_known_keys() {
        // Keys observed in persistent-table and scene registrations.
        assert_eq!(name_hash(b"ProgressionObjective"), 2_251_697_951);
        assert_eq!(name_hash(b"Active"), 2_484_178_249);
        assert_eq!(name_hash(b"levels/genesis01/gameplay"), 1_286_940_107);
        assert_eq!(name_hash(b""), 5381);
    }

    #[test]
    fn hex_is_lower_case() {
        assert_eq!(hex(&[0x00, 0xab, 0x7f]), "00ab7f");
    }
}
