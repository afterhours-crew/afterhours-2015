// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Persistent-table schemas from installed `PersistentTableAsset` partitions.
//!
//! Each asset names its table and lists its columns (name, type, default text)
//! and whether rows have a secondary key. Table and column keys are the
//! djb2-xor hashes of the exact names. The secondary-key column names and the
//! set of tables are the service's own policy (`required_tables`).
use crate::Error;
use crate::scan::Assets;
use nfs_frostbite::ebx::{Fields, Value};
use nfs_services::persistent::{Catalog, key, required_tables};
use serde_json::json;
use std::collections::BTreeMap;

pub const TABLE_CLASS: &str = "PersistentTableAsset";

fn text<'a>(fields: &'a Fields, table: &str, name: &str) -> Result<&'a str, Error> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Content(format!("table {table}: {name} is missing")))
}

fn value_type(enumerator: Option<&str>, table: &str) -> Result<&'static str, Error> {
    Ok(match enumerator {
        Some("PersistentPropertyType_Int") => "int",
        Some("PersistentPropertyType_Float") => "float",
        Some("PersistentPropertyType_String") => "string",
        _ => {
            return Err(Error::Content(format!(
                "table {table}: unsupported column type"
            )));
        }
    })
}

/// Build the persistent-table content document; `fingerprint` identifies the
/// installation it was read from.
pub fn build(assets: &mut Assets, fingerprint: &str) -> Result<Vec<u8>, Error> {
    let mut by_table: BTreeMap<String, Fields> = BTreeMap::new();
    for name in assets.named(TABLE_CLASS) {
        let fields = assets
            .objects(&name)?
            .first()
            .map(|o| o.fields.clone())
            .ok_or_else(|| Error::Content(format!("{name} has no table")))?;
        let table = text(&fields, &name, "TableName")?.to_owned();
        if by_table.get(&table).is_some_and(|other| *other != fields) {
            return Err(Error::Content(format!("table {table} is defined twice")));
        }
        by_table.insert(table, fields);
    }
    let mut tables = Vec::new();
    for (table, secondary, count) in required_tables() {
        let fields = by_table
            .get(*table)
            .ok_or_else(|| Error::Content(format!("table {table} is missing")))?;
        let properties = fields
            .get("Properties")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Content(format!("table {table}: Properties is missing")))?;
        if properties.len() != *count {
            return Err(Error::Content(format!(
                "table {table}: {} columns, expected {count}",
                properties.len()
            )));
        }
        if fields.get("HasSecondaryKey").and_then(Value::as_bool) != Some(secondary.is_some()) {
            return Err(Error::Content(format!(
                "table {table}: secondary key differs from policy"
            )));
        }
        if fields.get("WriteWholeRows").and_then(Value::as_bool) != Some(false) {
            return Err(Error::Content(format!(
                "table {table}: whole-row writes are unsupported"
            )));
        }
        let mut columns = Vec::with_capacity(*count);
        for property in properties {
            let property = property
                .as_fields()
                .ok_or_else(|| Error::Content(format!("table {table}: malformed column")))?;
            let name = text(property, table, "Name")?;
            columns.push(json!({
                "name": name,
                "key": key(name),
                "value_type": value_type(property.get("DataType").and_then(Value::as_enum_name), table)?,
                "default": text(property, table, "DefaultValue")?,
            }));
        }
        tables.push(json!({
            "name": table,
            "key": key(table),
            "secondary_key": secondary,
            "using_stores": secondary.is_none(),
            "write_whole_rows": false,
            "columns": columns,
        }));
    }
    let document = json!({
        "version": 1,
        "build_sha256": nfs_services::SUPPORTED_BUILD_SHA256,
        "source": {"path": "installation:PersistentTableAsset", "sha256": fingerprint},
        "tables": tables,
    });
    Catalog::from_json(&document)
        .map_err(|e| Error::Content(format!("persistent tables rejected: {e}")))?;
    serde_json::to_vec(&document).map_err(|_| Error::Content("table encoding".into()))
}
