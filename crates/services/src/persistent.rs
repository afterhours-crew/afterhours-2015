// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned provider for the sixteen persistent tables.
//! Static schemas are asset data. Rows belong to an account's repository;
//! GarageItemsTable is a projection of its canonical garage slots, never a
//! second copy of the vehicle IDs. No captured progression values are loaded.
use crate::{ContentError, SUPPORTED_BUILD_SHA256 as BUILD};
use nfs_storage::{
    AccountId, Batch, Error, InventoryRepository, Op, Snapshot, Timestamp,
    tables::{MAX_COLUMNS, MAX_STRING_BYTES, Row, Table, Value},
};
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

pub const INITIAL_BATCH: u64 = u64::MAX - 1;
const MAX_CONTENT_BYTES: u64 = 128 * 1024;
const GARAGE: &str = "GarageItemsTable";
const GARAGE_COLUMNS: [&str; 5] = [
    "PrimaryVehicleItem",
    "Vehicle1",
    "Vehicle2",
    "Vehicle3",
    "Vehicle4",
];
// Required table names, secondary-key metadata and column counts.
// A missing table cannot silently become a successful partial startup load.
const REQUIRED: [(&str, Option<&str>, usize); 16] = [
    ("ProgressionObjective", Some("ObjectiveID"), 2),
    ("PersistentDataTable", Some("ValueId"), 3),
    ("CollectiblesTable", Some("CollectibleID"), 1),
    ("RepValuesTable", None, 7),
    ("DynamicMessage", Some("Index"), 3),
    ("StaticMessage", Some("Index"), 2),
    ("GameplayStatsTable", None, 9),
    ("UgStaticMessages", Some("Index"), 2),
    ("PoiIdTable", Some("PoiId"), 1),
    (GARAGE, Some("GarageVehicles"), 5),
    ("TutorialTable", Some("ValueId"), 1),
    ("TicketTable", Some("Ticketd"), 1),
    ("ProgressionStats", None, 1),
    ("ActivitiesTable", Some("ActivityID"), 1),
    ("SpeedListStatsTable", None, 10),
    ("PrestigeMedalTable", None, 6),
];

/// Name, secondary-key column (if any) and column count of every required
/// table, in canonical order. Content builders select and describe tables
/// with it; the loader accepts exactly this set.
pub fn required_tables() -> &'static [(&'static str, Option<&'static str>, usize)] {
    &REQUIRED
}

/// Case-sensitive djb2-xor, unlike the content-name hash.
pub fn key(name: &str) -> u32 {
    name.bytes().fold(5381u32, |hash, byte| {
        hash.wrapping_mul(33) ^ u32::from(byte)
    })
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Schema {
    defaults: Row,
    using_stores: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    schemas: BTreeMap<u32, Schema>,
}

/// Constructed only after every required table, row type and garage reference
/// validates. The async edge retains this owned view for this connection.
#[derive(Clone, Eq, PartialEq)]
pub struct Loaded {
    generation: u64,
    tables: BTreeMap<u32, Table>,
    presence: BTreeMap<u32, BTreeMap<u64, BTreeSet<u32>>>,
    stores: BTreeMap<(u32, u32), i32>,
    garage: [Option<u64>; 5],
}
impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedTables")
            .field("generation", &self.generation)
            .field("tables", &self.tables.len())
            .field("stores", &self.stores.len())
            .finish()
    }
}
impl Loaded {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn tables(&self) -> &BTreeMap<u32, Table> {
        &self.tables
    }
    /// Persistence distinguishes a saved cell from its typed
    /// read fallback. Keep that distinction when exposing default-expanded rows.
    pub fn has_cell(&self, table: u32, row: u64, column: u32) -> bool {
        self.presence
            .get(&table)
            .and_then(|rows| rows.get(&row))
            .is_some_and(|columns| columns.contains(&column))
    }
    pub fn stores(&self) -> &BTreeMap<(u32, u32), i32> {
        &self.stores
    }
    pub fn garage(&self) -> [Option<u64>; 5] {
        self.garage
    }
}

fn fields(value: &Json, names: &[&str]) -> Result<(), ContentError> {
    let object = value.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(())
}
fn name(value: &Json) -> Result<&str, ContentError> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or(ContentError::Invalid)
}
fn same_type(a: &Value, b: &Value) -> bool {
    matches!(
        (a, b),
        (Value::Int(_), Value::Int(_))
            | (Value::Float(_), Value::Float(_))
            | (Value::String(_), Value::String(_))
    )
}
impl Catalog {
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take(MAX_CONTENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() as u64 > MAX_CONTENT_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    pub fn from_json(value: &Json) -> Result<Self, ContentError> {
        fields(value, &["version", "build_sha256", "source", "tables"])?;
        fields(&value["source"], &["path", "sha256"])?;
        let sha = value["source"]["sha256"]
            .as_str()
            .ok_or(ContentError::Invalid)?;
        let path = value["source"]["path"]
            .as_str()
            .ok_or(ContentError::Invalid)?;
        if value["version"] != 1
            || value["build_sha256"] != BUILD
            || sha.len() != 64
            || !sha.bytes().all(|b| b.is_ascii_hexdigit())
            || path.is_empty()
            || path.len() > 512
        {
            return Err(ContentError::Invalid);
        }
        let rows = value["tables"].as_array().ok_or(ContentError::Invalid)?;
        if rows.len() != REQUIRED.len() {
            return Err(ContentError::Invalid);
        }
        let mut schemas = BTreeMap::new();
        for table in rows {
            fields(
                table,
                &[
                    "name",
                    "key",
                    "secondary_key",
                    "using_stores",
                    "write_whole_rows",
                    "columns",
                ],
            )?;
            let table_name = name(&table["name"])?;
            let (_, secondary, count) = REQUIRED
                .iter()
                .find(|(n, _, _)| *n == table_name)
                .ok_or(ContentError::Invalid)?;
            if table["key"].as_u64() != Some(u64::from(key(table_name)))
                || table["secondary_key"] != serde_json::json!(secondary)
                || table["using_stores"].as_bool() != Some(secondary.is_none())
                || table["write_whole_rows"] != false
            {
                return Err(ContentError::Invalid);
            }
            let columns = table["columns"].as_array().ok_or(ContentError::Invalid)?;
            if columns.len() != *count || columns.len() > MAX_COLUMNS {
                return Err(ContentError::Invalid);
            }
            let mut defaults = Row::new();
            for column in columns {
                fields(column, &["name", "key", "value_type", "default"])?;
                let column_name = name(&column["name"])?;
                if column["key"].as_u64() != Some(u64::from(key(column_name))) {
                    return Err(ContentError::Invalid);
                }
                let default = column["default"]
                    .as_str()
                    .filter(|s| s.len() <= MAX_STRING_BYTES)
                    .ok_or(ContentError::Invalid)?;
                let value = match column["value_type"].as_str() {
                    // Use a zero fallback for absent/invalid int text.
                    Some("int") => Value::Int(default.parse().unwrap_or(0)),
                    Some("float") => {
                        let v: f32 = if default.is_empty() {
                            0.0
                        } else {
                            default.parse().map_err(|_| ContentError::Invalid)?
                        };
                        if !v.is_finite() {
                            return Err(ContentError::Invalid);
                        }
                        Value::Float(v.to_bits())
                    }
                    Some("string") => Value::String(default.into()),
                    _ => return Err(ContentError::Invalid),
                };
                if secondary.is_none() && !matches!(value, Value::Int(_)) {
                    return Err(ContentError::Invalid);
                }
                if defaults.insert(key(column_name), value).is_some() {
                    return Err(ContentError::Invalid);
                }
            }
            if table_name == GARAGE
                && defaults
                    != GARAGE_COLUMNS
                        .into_iter()
                        .map(|n| (key(n), Value::String("0".into())))
                        .collect()
            {
                return Err(ContentError::Invalid);
            }
            if schemas
                .insert(
                    key(table_name),
                    Schema {
                        defaults,
                        using_stores: secondary.is_none(),
                    },
                )
                .is_some()
            {
                return Err(ContentError::Invalid);
            }
        }
        Ok(Self { schemas })
    }

    fn validate_table(schema: &Schema, table: &Table) -> Result<(), Error> {
        table.validate()?;
        if schema.using_stores && table.rows.keys().any(|key| *key != 0) {
            return Err(Error::Config);
        }
        for row in table.rows.values() {
            for (column, value) in row {
                if !schema
                    .defaults
                    .get(column)
                    .is_some_and(|d| same_type(d, value))
                {
                    return Err(Error::Config);
                }
            }
        }
        Ok(())
    }
    /// Initialize absent tables only. Explicitly empty existing tables survive.
    /// One retry-safe transaction, shared with item/garage generation history.
    pub fn ensure_loaded(
        &self,
        repository: &dyn InventoryRepository,
        account: AccountId,
        mut snapshot: Snapshot,
        now: Timestamp,
    ) -> Result<(Snapshot, Loaded), Error> {
        if snapshot.tables.contains_key(&key(GARAGE)) {
            return Err(Error::Config);
        }
        let mut ops = Vec::new();
        for (&key, schema) in &self.schemas {
            if key == self::key(GARAGE) {
                continue;
            }
            if let Some(table) = snapshot.tables.get(&key) {
                Self::validate_table(schema, table)?;
            } else {
                let rows = if schema.using_stores {
                    BTreeMap::from([(0, schema.defaults.clone())])
                } else {
                    BTreeMap::new()
                };
                ops.push(Op::SetTable(key, Table { rows }));
            }
        }
        if !ops.is_empty() {
            let batch = Batch {
                id: INITIAL_BATCH,
                expected_generation: snapshot.generation,
                ops,
            };
            match repository.apply(account, &batch, now) {
                Ok(_) | Err(Error::Conflict) => {}
                Err(error) => return Err(error),
            }
            snapshot = repository.snapshot(account)?;
        }
        let loaded = self.view(&snapshot)?;
        Ok((snapshot, loaded))
    }
    pub fn view(&self, snapshot: &Snapshot) -> Result<Loaded, Error> {
        let garage = snapshot.garage.ok_or(Error::Config)?;
        garage.validate_items(&snapshot.items)?;
        if snapshot.tables.contains_key(&key(GARAGE)) {
            return Err(Error::Config);
        }
        let garage = garage.0.map(|id| id.map(nfs_storage::ItemId::get));
        let mut tables = BTreeMap::new();
        let mut presence = BTreeMap::new();
        let mut stores = BTreeMap::new();
        for (&table_key, schema) in &self.schemas {
            let mut table = if table_key == key(GARAGE) {
                Table {
                    rows: BTreeMap::from([(
                        0,
                        GARAGE_COLUMNS
                            .into_iter()
                            .zip(garage)
                            .map(|(n, id)| (key(n), Value::String(id.unwrap_or(0).to_string())))
                            .collect(),
                    )]),
                }
            } else {
                snapshot
                    .tables
                    .get(&table_key)
                    .ok_or(Error::Config)?
                    .clone()
            };
            Self::validate_table(schema, &table)?;
            presence.insert(
                table_key,
                table
                    .rows
                    .iter()
                    .map(|(&id, row)| (id, row.keys().copied().collect()))
                    .collect(),
            );
            // Missing fields read through typed asset defaults, without
            // overwriting sparse durable rows. Empty dynamic tables stay empty.
            if schema.using_stores {
                table.rows.entry(0).or_default();
            }
            let mut expanded_bytes = 8usize;
            for row in table.rows.values_mut() {
                expanded_bytes += 10;
                for (column, default) in &schema.defaults {
                    expanded_bytes += 5 + match row.get(column).unwrap_or(default) {
                        Value::Int(_) | Value::Float(_) => 4,
                        Value::String(value) => 2 + value.len(),
                    };
                }
                // Bound default expansion before allocating its strings.
                if expanded_bytes > nfs_storage::tables::MAX_TABLE_BYTES {
                    return Err(Error::Bounds);
                }
                for (&column, default) in &schema.defaults {
                    row.entry(column).or_insert_with(|| default.clone());
                }
            }
            if schema.using_stores {
                for (&column, value) in &table.rows[&0] {
                    let Value::Int(value) = value else {
                        return Err(Error::Config);
                    };
                    stores.insert((table_key, column), *value);
                }
            }
            table.validate()?;
            tables.insert(table_key, table);
        }
        Ok(Loaded {
            generation: snapshot.generation,
            tables,
            presence,
            stores,
            garage,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
