// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Kind;

/// Bounded, metadata-only progress from actual preparation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Fingerprinting,
    CacheCheck,
    CacheReused,
    CacheRebuilding,
    Scanning,
    Scanned,
    Extracting(Kind),
    Extracted { kind: Kind, bytes: usize },
    Publishing,
    Ready,
}

impl Kind {
    pub fn description(self) -> &'static str {
        match self {
            Self::ItemContent => "cars, inventory definitions and starter items",
            Self::PersistentContent => "save-data table definitions",
            Self::StatDefinitions => "player statistics definitions",
            Self::ItemLicenses => "installed item license names (no entitlement grants)",
            Self::ControlCatalogs => "menu catalogs and statistics scopes",
            Self::ReputationThresholds => "REP levels and reward thresholds",
            Self::SceneCatalog => "world scene and object definitions",
            Self::WorldContent => "world startup and level registrations",
            Self::GarageLayout => "garage slots and parking positions",
            Self::WorldMacTemplate => "world connection data from NFS16.exe",
            Self::VehicleParts => "vehicle parts, customization and upgrade definitions",
            Self::VehicleProfiles => "vehicle composition, physics and appearance",
            Self::ProgressionContent => "progression objectives and scene bindings",
            Self::SequenceContent => "garage entry and exit sequences",
        }
    }
}

impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fingerprinting => f.write_str("Checking NFS16.exe and fingerprinting installed content indexes..."),
            Self::CacheCheck => f.write_str("Verifying cached content and output hashes..."),
            Self::CacheReused => f.write_str("Verified content cache reused; extraction skipped."),
            Self::CacheRebuilding => f.write_str("No reusable content cache. Building from your installation..."),
            Self::Scanning => f.write_str("Reading installed Frostbite containers and indexing game assets (this can take a while)..."),
            Self::Scanned => f.write_str("Installed asset index ready."),
            Self::Extracting(kind) => write!(f, "Extracting {}...", kind.description()),
            Self::Extracted { kind, bytes } => write!(f, "Done: {} ({bytes} bytes generated).", kind.description()),
            Self::Publishing => f.write_str("Writing verified content and its manifest to the local cache..."),
            Self::Ready => f.write_str("Content preparation complete."),
        }
    }
}
