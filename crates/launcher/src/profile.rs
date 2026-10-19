// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only driver profiles from the account saves in a data directory.
//!
//! A save is read through [`nfs_storage::SqliteRepository::inspect`]: a
//! read-only connection, one read transaction, a bounded lock wait and no
//! write of any kind, so a profile can be shown while the server runs. The
//! profile never takes the data-directory lock. Stored values are decoded
//! by the server's own domain code in `nfs-services`, with schemas, REP
//! thresholds and car names from the prepared installation content.
//!
//! Stored integers are reported exactly as stored. The game's units for
//! distances, times and speeds are not confirmed, so none is applied. Only
//! values whose meaning the server itself defines are formatted: bank cash
//! and the save's own millisecond timestamp. 64-bit identifiers are decimal
//! strings so JavaScript consumers keep every digit.

pub mod content;

use crate::{
    clock,
    exit::{Failure, Result},
    paths, records,
};
use content::{Content, Need, Source, Vehicle};
use nfs_services::{
    authentication::Identity,
    bank::Balance,
    persistent::{Loaded, key},
    reputation::{Field, Scores, Thresholds},
};
use nfs_storage::{AccountId, Inspection, ItemId, Snapshot, SqliteRepository, account::Kind};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Serialize, Serializer};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

/// Version of the JSON objects below. Additive fields keep the version.
pub const FORMAT_VERSION: u32 = 1;
/// Accounts listed; more saves set `truncated`.
pub const MAX_ACCOUNTS: usize = 64;
/// Directory entries inspected while looking for saves.
pub const MAX_ACCOUNT_ENTRIES: usize = 4096;
/// Latest millisecond timestamp formatted as a date: 9999-12-31T23:59:59Z.
const MAX_FORMATTED_MS: u64 = 253_402_300_799_999;

/// Single-row tables and the columns reported from each, as named by the
/// installed persistent-table asset.
const STORE_TABLES: [(&str, &[&str]); 5] = [
    (
        "RepValuesTable",
        &[
            "RepScore",
            "RepLevel",
            "BuildScore",
            "CrewScore",
            "OutlawScore",
            "SpeedScore",
            "StyleScore",
        ],
    ),
    (
        "PrestigeMedalTable",
        &[
            "BuildMedal",
            "CrewMedal",
            "OutlawMedal",
            "SpeedMedal",
            "StyleMedal",
            "FinalEventMedal",
        ],
    ),
    (
        "GameplayStatsTable",
        &[
            "DistanceDriven",
            "CashEarned",
            "NumberOfEventsCompleted",
            "LargestFineEscaped",
            "LargestFineBusted",
            "TotalTimePlayed",
            "FavoriteVehicleId",
            "TopSpeed",
            "DistanceDrifted",
        ],
    ),
    (
        "SpeedListStatsTable",
        &[
            "Started", "Finished", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8",
        ],
    ),
    ("ProgressionStats", &["IntroCompleted"]),
];

/// One stored integer: the value the server serves (the schema default when
/// the save has no cell) and whether the save holds the cell itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Cell {
    pub value: i32,
    pub stored: bool,
}

/// Ordered columns, serialized as an object. A column the installed schema
/// lacks is `null`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Columns(pub Vec<(&'static str, Option<Cell>)>);
impl Serialize for Columns {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(name, cell)| (name, cell)))
    }
}
impl JsonSchema for Columns {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Columns".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let cell = generator.subschema_for::<Option<Cell>>();
        json_schema!({
            "description": "Stored cells by column name, in schema order. A column the installed schema lacks is `null`.",
            "type": "object",
            "additionalProperties": cell,
        })
    }
}
impl Columns {
    pub fn get(&self, name: &str) -> Option<Cell> {
        self.0
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, c)| *c)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Driver {
    pub display_name: String,
    pub persona_id: String,
    pub account_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Bank {
    pub balance: u32,
    pub formatted: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Categories {
    pub build: Option<Cell>,
    pub crew: Option<Cell>,
    pub outlaw: Option<Cell>,
    pub speed: Option<Cell>,
    pub style: Option<Cell>,
}

/// REP as `nfs_services::reputation` presents it. `level` and the
/// thresholds are evaluated from `score`; `stored_level` is the saved
/// `RepLevel` cell, which is not an input to that evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Reputation {
    pub score: Option<Cell>,
    pub stored_level: Option<Cell>,
    pub categories: Categories,
    pub level: Option<i32>,
    pub current_threshold: Option<i32>,
    pub next_threshold: Option<i32>,
    pub maximum_level: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Item {
    pub id: String,
    /// Definition GUID as 32 lowercase hexadecimal characters.
    pub definition: String,
    /// Definition class from the installed item content.
    pub class: Option<&'static str>,
    /// Blueprint from the installed vehicle content, for cars.
    pub vehicle: Option<Vehicle>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Slot {
    pub index: usize,
    pub primary: bool,
    pub item: Option<Item>,
}

/// The five ordered garage slots; slot 0 is the primary vehicle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Garage {
    pub slots: Vec<Slot>,
    pub primary_vehicle: Option<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Counts {
    pub items: usize,
    /// Owned items per definition class; `null` without item content.
    pub items_by_class: Option<BTreeMap<&'static str, usize>>,
    /// Owned items whose definition the item content does not list.
    pub unresolved_items: Option<usize>,
    pub livery_records: usize,
    pub livery_bytes: usize,
    /// Saved rows; `null` when the save has no such table yet.
    pub collectibles: Option<usize>,
    pub activities: Option<usize>,
    pub points_of_interest: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Domain {
    pub revision: u64,
    pub bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Save {
    pub path: PathBuf,
    pub schema_version: i64,
    pub generation: u64,
    /// Milliseconds since the Unix epoch, from the server's clock.
    pub updated_at_ms: u64,
    pub updated_at_utc: Option<String>,
    /// Main database file only; recent commits may still be in its log.
    pub file_bytes: u64,
    /// Present account domains and their revisions.
    pub domains: BTreeMap<&'static str, Domain>,
}

/// `profile --json`: one saved driver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Profile {
    pub version: u32,
    pub account_id: String,
    /// Whether `account.json` selects this account.
    pub active: bool,
    pub identity: Option<Driver>,
    pub bank: Option<Bank>,
    pub reputation: Option<Reputation>,
    /// Single-row tables by stored table and column name.
    pub tables: Option<BTreeMap<&'static str, Columns>>,
    pub garage: Option<Garage>,
    pub counts: Counts,
    pub save: Save,
    pub content: Option<Source>,
    pub warnings: Vec<String>,
}

/// One line of the account list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(rename = "ProfileEntry")]
pub struct Entry {
    pub account_id: String,
    pub active: bool,
    pub display_name: Option<String>,
    pub level: Option<i32>,
    pub stored_level: Option<i32>,
    pub score: Option<i32>,
    pub bank: Option<Bank>,
    pub updated_at_ms: Option<u64>,
    pub updated_at_utc: Option<String>,
    /// Why this save could not be read; the other fields are then `null`.
    pub error: Option<String>,
}

/// `profile --list --json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(rename = "ProfileList")]
pub struct Listing {
    pub version: u32,
    pub accounts: Vec<Entry>,
    pub truncated: bool,
    pub content: Option<Source>,
    pub warnings: Vec<String>,
}

/// `1234567` as `1,234,567`.
pub fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn utc(ms: u64) -> Option<String> {
    if ms == 0 || ms > MAX_FORMATTED_MS {
        return None;
    }
    UNIX_EPOCH
        .checked_add(Duration::from_millis(ms))
        .map(clock::iso_utc)
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Identity => "identity",
        Kind::Entitlements => "entitlements",
        Kind::Kickback => "kickback",
        Kind::Speedwall => "speedwall",
        Kind::Settings => "settings",
    }
}

/// Human message for a save that cannot be inspected. Nothing was changed.
pub fn storage_failure(error: nfs_storage::Error) -> Failure {
    use nfs_storage::Error as E;
    match error {
        E::Absent => {
            Failure::no_session("This account has no saved database in the data directory.")
        }
        E::Version => Failure::refused(
            "This save uses a schema version this launcher does not read; it was not changed.",
        ),
        E::Identity => Failure::refused(
            "This database belongs to a different account than its file name; it was not changed.",
        ),
        E::Busy => Failure::internal("The save stayed locked by another process; try again."),
        E::Storage => Failure::internal("The account database could not be read."),
        _ => Failure::refused(
            "The account database is damaged or exceeds its bounds; it was not changed.",
        ),
    }
}

fn cell(loaded: &Loaded, table: &str, column: &str) -> Option<Cell> {
    let (table, column) = (key(table), key(column));
    loaded.stores().get(&(table, column)).map(|value| Cell {
        value: *value,
        stored: loaded.has_cell(table, 0, column),
    })
}

fn reputation(loaded: &Loaded, thresholds: Option<&Thresholds>) -> Reputation {
    let rep = |column| cell(loaded, "RepValuesTable", column);
    let values = thresholds.and_then(|thresholds| {
        Scores::load(loaded)
            .ok()
            .map(|scores| (thresholds.evaluate(scores), thresholds.maximum_level()))
    });
    Reputation {
        score: rep("RepScore"),
        stored_level: rep("RepLevel"),
        categories: Categories {
            build: rep("BuildScore"),
            crew: rep("CrewScore"),
            outlaw: rep("OutlawScore"),
            speed: rep("SpeedScore"),
            style: rep("StyleScore"),
        },
        level: values.map(|(v, _)| v.get(Field::Level)),
        current_threshold: values.map(|(v, _)| v.get(Field::CurrentThreshold)),
        next_threshold: values.map(|(v, _)| v.get(Field::NextThreshold)),
        maximum_level: values.map(|(_, maximum)| maximum),
    }
}

fn item(snapshot: &Snapshot, id: ItemId, content: &Content) -> Option<Item> {
    let record = snapshot.items.get(&id)?;
    let definition = record.definition.0;
    Some(Item {
        id: id.get().to_string(),
        definition: hex(&definition),
        class: content
            .items
            .as_ref()
            .and_then(|items| items.catalog.class(&definition).ok())
            .map(|class| class.name()),
        vehicle: content
            .vehicles
            .as_ref()
            .and_then(|vehicles| vehicles.get(&definition))
            .cloned(),
    })
}

fn identity(
    inspection: &Inspection,
    account: AccountId,
) -> std::result::Result<Driver, &'static str> {
    let document = inspection
        .documents
        .get(&Kind::Identity)
        .ok_or("The save has no identity record.")?;
    let value: serde_json::Value =
        serde_json::from_slice(&document.bytes).map_err(|_| "The saved identity is invalid.")?;
    let identity = Identity::from_json(&value).map_err(|_| "The saved identity is invalid.")?;
    if identity.storage_account() != account {
        return Err("The saved identity belongs to another account.");
    }
    Ok(Driver {
        display_name: identity.name().to_owned(),
        persona_id: identity.persona_id().to_string(),
        account_id: account.hex(),
    })
}

/// Interpret one inspected save. Never fails: a domain that cannot be
/// decoded is `null` with a warning.
pub fn build(
    account: AccountId,
    active: bool,
    path: PathBuf,
    inspection: &Inspection,
    content: &Content,
) -> Profile {
    let snapshot = &inspection.snapshot;
    let mut warnings = content.warnings.clone();
    let identity = identity(inspection, account)
        .map_err(|warning| warnings.push(warning.into()))
        .ok();
    let bank = match Balance::from_snapshot(snapshot) {
        Ok(Balance(balance)) => Some(Bank {
            balance,
            formatted: format!("${}", grouped(i64::from(balance))),
        }),
        Err(_) => {
            warnings.push("The saved bank record is invalid.".into());
            None
        }
    };
    let loaded = content.tables.as_ref().and_then(|catalog| {
        catalog
            .view(snapshot)
            .map_err(|_| {
                warnings.push(
                    "The saved tables are not initialized or do not match the installed schema; start the server once to initialize them."
                        .into(),
                )
            })
            .ok()
    });
    let reputation = loaded
        .as_ref()
        .map(|loaded| reputation(loaded, content.thresholds.as_ref()));
    let tables = loaded.as_ref().map(|loaded| {
        STORE_TABLES
            .iter()
            .map(|(table, columns)| {
                (
                    *table,
                    Columns(
                        columns
                            .iter()
                            .map(|column| (*column, cell(loaded, table, column)))
                            .collect(),
                    ),
                )
            })
            .collect()
    });
    let garage = snapshot.garage.map(|slots| {
        let slots: Vec<Slot> = slots
            .0
            .iter()
            .enumerate()
            .map(|(index, id)| Slot {
                index,
                primary: index == 0,
                item: id.and_then(|id| item(snapshot, id, content)),
            })
            .collect();
        Garage {
            primary_vehicle: slots[0].item.clone(),
            slots,
        }
    });
    if garage.is_none() {
        warnings.push("The save has no garage yet.".into());
    }
    let classes = content.items.as_ref().map(|items| {
        let mut by_class = BTreeMap::new();
        let mut unresolved = 0;
        for record in snapshot.items.values() {
            match items.catalog.class(&record.definition.0) {
                Ok(class) => *by_class.entry(class.name()).or_insert(0) += 1,
                Err(_) => unresolved += 1,
            }
        }
        (by_class, unresolved)
    });
    let rows = |table: &str| snapshot.tables.get(&key(table)).map(|t| t.rows.len());
    Profile {
        version: FORMAT_VERSION,
        account_id: account.hex(),
        active,
        identity,
        bank,
        reputation,
        tables,
        garage,
        counts: Counts {
            items: snapshot.items.len(),
            unresolved_items: classes.as_ref().map(|(_, unresolved)| *unresolved),
            items_by_class: classes.map(|(by_class, _)| by_class),
            livery_records: inspection.livery.records,
            livery_bytes: inspection.livery.bytes,
            collectibles: rows("CollectiblesTable"),
            activities: rows("ActivitiesTable"),
            points_of_interest: rows("PoiIdTable"),
        },
        save: Save {
            path,
            schema_version: inspection.schema_version,
            generation: snapshot.generation,
            updated_at_ms: snapshot.updated_at.0,
            updated_at_utc: utc(snapshot.updated_at.0),
            file_bytes: inspection.file_bytes,
            domains: inspection
                .documents
                .iter()
                .map(|(kind, document)| {
                    (
                        kind_name(*kind),
                        Domain {
                            revision: document.revision,
                            bytes: document.bytes.len(),
                        },
                    )
                })
                .collect(),
        },
        content: content.source.clone(),
        warnings,
    }
}

/// An existing component of the data directory that is not a link.
fn ordinary_dir(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if paths::is_link(&meta) || !meta.is_dir() => Err(Failure::refused(
            "The account directory must be an ordinary directory, not a link.",
        )),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// The saves of one data directory, opened for reading only.
pub struct Saves {
    data: PathBuf,
    /// `artifacts/account` and its read-only repository, when present.
    repository: Option<(PathBuf, SqliteRepository)>,
    active: Option<String>,
}

impl Saves {
    /// Validate the data path and find `artifacts/account`. A data directory
    /// without saves is not an error here; callers decide.
    pub fn open(data: &Path) -> Result<Self> {
        let data = paths::checked_data_path(data, None)?;
        let artifacts = data.join("artifacts");
        let directory = artifacts.join("account");
        let present =
            ordinary_dir(&data)? && ordinary_dir(&artifacts)? && ordinary_dir(&directory)?;
        let repository = if present {
            let repository =
                SqliteRepository::open_existing_directory(&directory).map_err(storage_failure)?;
            Some((directory, repository))
        } else {
            None
        };
        let pointer = data.join("account.json");
        let active = if pointer.exists() {
            let record: records::AccountRecord = records::read_json(&pointer)?;
            record.validate()?;
            Some(record.local_account)
        } else {
            None
        };
        Ok(Self {
            data,
            repository,
            active,
        })
    }

    pub fn data(&self) -> &Path {
        &self.data
    }

    /// The account `account.json` selects, if any.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    fn inspect(&self, id: &str) -> Result<(AccountId, PathBuf, Inspection)> {
        let account = parse_account(id)?;
        let (_, repository) = self.repository.as_ref().ok_or_else(|| {
            Failure::no_session("This data directory has no saved accounts yet; run setup first.")
        })?;
        let inspection = repository.inspect(account).map_err(storage_failure)?;
        Ok((account, repository.path(account), inspection))
    }

    /// One account's profile.
    pub fn profile(&self, id: &str, content: &Content) -> Result<Profile> {
        let (account, path, inspection) = self.inspect(id)?;
        Ok(build(
            account,
            self.active() == Some(id),
            path,
            &inspection,
            content,
        ))
    }

    /// Every save in the account directory, in account-ID order. A save
    /// that cannot be read is listed with its error.
    pub fn list(&self, content: &Content) -> Result<Listing> {
        let mut ids = Vec::new();
        if let Some((directory, _)) = &self.repository {
            for entry in std::fs::read_dir(directory)?.take(MAX_ACCOUNT_ENTRIES) {
                let name = entry?.file_name();
                if let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".sqlite"))
                    && records::is_id(id)
                {
                    ids.push(id.to_owned());
                }
            }
        }
        ids.sort();
        let truncated = ids.len() > MAX_ACCOUNTS;
        ids.truncate(MAX_ACCOUNTS);
        let accounts = ids
            .into_iter()
            .map(|id| {
                let active = self.active() == Some(id.as_str());
                match self.profile(&id, content) {
                    Ok(profile) => {
                        let reputation = profile.reputation.as_ref();
                        Entry {
                            account_id: id,
                            active,
                            display_name: profile.identity.map(|d| d.display_name),
                            level: reputation.and_then(|r| r.level),
                            stored_level: reputation.and_then(|r| r.stored_level).map(|c| c.value),
                            score: reputation.and_then(|r| r.score).map(|c| c.value),
                            bank: profile.bank,
                            updated_at_ms: Some(profile.save.updated_at_ms),
                            updated_at_utc: profile.save.updated_at_utc,
                            error: None,
                        }
                    }
                    Err(failure) => Entry {
                        account_id: id,
                        active,
                        display_name: None,
                        level: None,
                        stored_level: None,
                        score: None,
                        bank: None,
                        updated_at_ms: None,
                        updated_at_utc: None,
                        error: Some(failure.message),
                    },
                }
            })
            .collect();
        Ok(Listing {
            version: FORMAT_VERSION,
            accounts,
            truncated,
            content: content.source.clone(),
            warnings: content.warnings.clone(),
        })
    }
}

/// A 32-character lowercase account ID as a storage key.
pub fn parse_account(id: &str) -> Result<AccountId> {
    let invalid = || Failure::refused("An account ID is 32 lowercase hexadecimal characters.");
    if !records::is_id(id) {
        return Err(invalid());
    }
    let bytes = content::guid(id).ok_or_else(invalid)?;
    AccountId::from_owned_config(bytes).map_err(|_| invalid())
}

/// Load content for `need` from the explicit directory or the data
/// directory's own records.
pub fn content(data: &Path, explicit: Option<&Path>, need: Need) -> Result<Content> {
    Ok(Content::load(content::locate(data, explicit)?, need))
}

#[cfg(test)]
mod tests;
