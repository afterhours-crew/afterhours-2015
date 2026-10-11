// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};
const SCALAR_KEYS: &[&str] = &[
    "associationListSkipInitialSet",
    "autoReconnectEnabled",
    "cashToBeAwardedForBeatingARecommendation",
    "connIdleTimeout",
    "defaultRequestTimeout",
    "gameManagerPreventMultipleGameMembership",
    "kickbackRewardLevelRep",
    "kickbackRewardLevels",
    "maximumNumberOfVehiclesInWarehouse",
    "minimalDownstreamBitsPerSecondForNewsImages",
    "minimalDownstreamKBPerSecondForNewsImages",
    "minimalUploadStreamKBForSnapShotUpload",
    "NPSSurveySessionDays",
    "NPSSurveyTargetPercentage",
    "pingPeriod",
    "playersYouMayKnowScreenUpdateRate",
    "providerOnlineUserRefresh",
    "recommendationsUpdateRate",
    "snapshotLoadTimeDelay",
    "snapshotMostLikedREPAmount",
    "statsAndAwardsCacheSecondsEveryoneElse",
    "statsAndAwardsCacheSecondsLocalPlayer",
    "userManagerMaxCachedUsers",
    "voipHeadsetUpdateRate",
    "xlspConnectionIdleTimeout",
];
const TABLES: &[&str] = &[
    "AchievementsStats",
    "CollectiblesTable",
    "DynamicMessage",
    "GameplayStatsTable",
    "GarageItemsTable",
    "PersistentDataTable",
    "PoiIdTable",
    "ProgressionObjective",
    "ProgressionStats",
    "RepValuesTable",
    "SpeedListStatsTable",
    "StaticMessage",
    "TicketTable",
    "TutorialTable",
    "UgStaticMessages",
];
pub(super) const URL_KEYS: &[&str] = &[
    "nucleusConnect",
    "nucleusConnectTrusted",
    "nucleusPortal",
    "nucleusProxy",
    "xboxOneStringValidationUri",
    "telemetryPinServerAddress",
    "NPSSurveyURL",
    "NPSSurveyURL_br",
    "NPSSurveyURL_de",
    "NPSSurveyURL_en",
    "NPSSurveyURL_es",
    "NPSSurveyURL_fr",
    "NPSSurveyURL_it",
    "NPSSurveyURL_ja",
    "NPSSurveyURL_pl",
    "NPSSurveyURL_ru",
    "NPSSurveyURL_zh",
];

fn scalar_key(key: &str) -> bool {
    SCALAR_KEYS.contains(&key)
        || ["_Time", "_PendingChanges"].iter().any(|suffix| {
            key.strip_suffix(suffix)
                .is_some_and(|table| TABLES.contains(&table))
        })
}

fn scalar_value(value: &[u8]) -> bool {
    // Retain configured numeric/duration/list spelling without interpreting
    // units. This lexical guard does not establish timing or game limits.
    !value.is_empty()
        && value.len() <= 128
        && (matches!(value, b"true" | b"false")
            || (value.iter().any(u8::is_ascii_digit)
                && value
                    .iter()
                    .all(|v| v.is_ascii_digit() || b"-+.,: dhmsu".contains(v))))
}

pub(super) const METADATA: [&str; 10] = [
    "authentication_source",
    "client_id",
    "entitlement_source",
    "service_name",
    "persona_namespace",
    "legal_doc_game_identifier",
    "platform",
    "registration_source",
    "server_version",
    "identity_display",
];
pub struct Config {
    pub(super) metadata: BTreeMap<String, String>,
    pub(super) components: Vec<u16>,
    pub(super) underage: bool,
    pub(super) settings: Vec<(String, Option<String>)>,
    pub(super) sites: Vec<(String, String)>,
    pub(super) probes: u16,
    pub(super) service_id: u32,
    pub(super) timeout: i64,
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapConfig")
            .field("settings", &self.settings.len())
            .finish_non_exhaustive()
    }
}
fn fields<'a>(v: &'a Json, names: &[&str]) -> Result<&'a Json, ContentError> {
    let object = v.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(v)
}
fn string(v: &Json) -> Result<String, ContentError> {
    v.as_str()
        .filter(|s| s.len() <= 256 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(ContentError::Invalid)
}
fn array(v: &Json) -> Result<&[Json], ContentError> {
    v.as_array()
        .filter(|v| v.len() <= 128)
        .map(Vec::as_slice)
        .ok_or(ContentError::Invalid)
}
fn unsigned<T: TryFrom<u64>>(v: &Json) -> Result<T, ContentError> {
    v.as_u64()
        .and_then(|v| v.try_into().ok())
        .ok_or(ContentError::Invalid)
}
impl Config {
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let file = File::open(path).map_err(|_| ContentError::Io)?;
        const MAX: u64 = 128 * 1024;
        if file.metadata().map_err(|_| ContentError::Io)?.len() > MAX {
            return Err(ContentError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > MAX as usize {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    /// Versioned product/deployment configuration. Dynamic endpoint entries have
    /// null values; no captured hosts, machine identifiers, credentials, player
    /// data or encoded reply bodies are accepted in this document.
    pub fn from_json(v: &Json) -> Result<Self, ContentError> {
        fields(
            v,
            &[
                "version",
                "build_sha256",
                "metadata",
                "components",
                "underage_supported",
                "settings",
                "qos",
            ],
        )?;
        if v["version"] != 1 || v["build_sha256"] != SUPPORTED_BUILD_SHA256 {
            return Err(ContentError::Invalid);
        }
        let mut metadata = BTreeMap::new();
        fields(&v["metadata"], &METADATA)?;
        for name in METADATA {
            metadata.insert(name.to_owned(), string(&v["metadata"][name])?);
        }
        if metadata["service_name"].is_empty() || metadata["client_id"].is_empty() {
            return Err(ContentError::Invalid);
        }
        let mut components = Vec::new();
        let mut seen = BTreeSet::new();
        for n in array(&v["components"])? {
            let id = unsigned(n)?;
            if !seen.insert(id) {
                return Err(ContentError::Invalid);
            }
            components.push(id);
        }
        if components.is_empty() {
            return Err(ContentError::Invalid);
        }
        let underage = v["underage_supported"]
            .as_bool()
            .ok_or(ContentError::Invalid)?;
        let mut settings = Vec::new();
        let mut names = BTreeSet::new();
        let mut endpoints = 0;
        for row in array(&v["settings"])? {
            fields(row, &["name", "value"])?;
            let name = string(&row["name"])?;
            if !names.insert(name.clone()) {
                return Err(ContentError::Invalid);
            }
            let value = if scalar_key(&name) {
                let value = string(&row["value"])?;
                if !scalar_value(value.as_bytes()) {
                    return Err(ContentError::Invalid);
                }
                Some(value)
            } else if URL_KEYS.contains(&name.as_str())
                || matches!(
                    name.as_str(),
                    "bytevaultHostname"
                        | "bytevaultPort"
                        | "bytevaultSecure"
                        | "telemetryPinServerPort"
                        | "xblTokenUrn"
                )
            {
                if !row["value"].is_null() {
                    return Err(ContentError::Invalid);
                }
                endpoints += 1;
                None
            } else {
                return Err(ContentError::Invalid);
            };
            settings.push((name, value));
        }
        if settings.len() != 77 || endpoints != 22 {
            return Err(ContentError::Invalid);
        }
        let qos = fields(&v["qos"], &["probes", "service_id", "timeout", "sites"])?;
        let probes = unsigned(&qos["probes"])?;
        let service_id = unsigned(&qos["service_id"])?;
        let timeout = qos["timeout"].as_i64().ok_or(ContentError::Invalid)?;
        let mut sites = Vec::new();
        names.clear();
        for site in array(&qos["sites"])? {
            fields(site, &["alias", "name"])?;
            let alias = string(&site["alias"])?;
            let name = string(&site["name"])?;
            if alias.is_empty() || !names.insert(alias.clone()) {
                return Err(ContentError::Invalid);
            }
            sites.push((alias, name));
        }
        let config = Self {
            metadata,
            components,
            underage,
            settings,
            sites,
            probes,
            service_id,
            timeout,
        };
        let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
        Profile::new(&config, endpoint, endpoint).map_err(|_| ContentError::Invalid)?;
        Ok(config)
    }
}

#[cfg(test)]
pub(super) fn constructed() -> Json {
    use serde_json::json;
    let mut names: Vec<_> = SCALAR_KEYS.iter().map(|s| s.to_string()).collect();
    for table in TABLES {
        for suffix in ["_Time", "_PendingChanges"] {
            names.push(format!("{table}{suffix}"));
        }
    }
    let mut settings: Vec<_> = names
        .iter()
        .map(|name| json!({"name":name,"value":"12"}))
        .collect();
    settings.extend(
        URL_KEYS
            .iter()
            .copied()
            .chain([
                "bytevaultHostname",
                "bytevaultPort",
                "bytevaultSecure",
                "telemetryPinServerPort",
                "xblTokenUrn",
            ])
            .map(|name| json!({"name":name,"value":null})),
    );
    settings.sort_by_key(|v| v["name"].as_str().unwrap().to_owned());
    let metadata: BTreeMap<_, _> = METADATA
        .into_iter()
        .map(|name| {
            (
                name,
                if name == "server_version" {
                    "local\nserver"
                } else {
                    "local"
                },
            )
        })
        .collect();
    json!({"version":1,"build_sha256":SUPPORTED_BUILD_SHA256,"metadata":metadata,"components":[9,1],"underage_supported":false,"settings":settings,"qos":{"probes":3,"service_id":8,"timeout":17,"sites":[{"alias":"local","name":"loopback"}]}})
}
