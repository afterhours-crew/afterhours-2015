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
            Self::ItemContent => "cars and starter garage items",
            Self::PersistentContent => "save-data definitions",
            Self::StatDefinitions => "player statistics definitions",
            Self::ItemLicenses => "installed item identifiers",
            Self::ControlCatalogs => "menus and statistics categories",
            Self::ReputationThresholds => "REP levels and reward thresholds",
            Self::SceneCatalog => "world locations and objects",
            Self::WorldContent => "world startup definitions",
            Self::GarageLayout => "garage slots and parking positions",
            Self::WorldMacTemplate => "world connection data from NFS16.exe",
            Self::VehicleParts => "vehicle parts, customization and upgrade definitions",
            Self::VehicleProfiles => "car components, handling and appearance",
            Self::ProgressionContent => "career objectives and world links",
            Self::SequenceContent => "garage entry and exit sequences",
            Self::TrafficContent => "traffic vehicles and road splines",
        }
    }
}

impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fingerprinting => f.write_str("Checking your game files for changes..."),
            Self::CacheCheck => f.write_str("Checking previously prepared game data..."),
            Self::CacheReused => f.write_str("Verified content cache reused; extraction skipped."),
            Self::CacheRebuilding => {
                f.write_str("Preparing a new local cache from your game installation...")
            }
            Self::Scanning => {
                f.write_str("Reading the installed game archives. This can take a few minutes...")
            }
            Self::Scanned => f.write_str("Game file index ready."),
            Self::Extracting(kind) => write!(f, "Extracting {}...", kind.description()),
            Self::Extracted { kind, bytes } => {
                write!(f, "Done: {} ({bytes} bytes generated).", kind.description())
            }
            Self::Publishing => {
                f.write_str("Saving the prepared game data for faster launches next time...")
            }
            Self::Ready => f.write_str("Content preparation complete."),
        }
    }
}
