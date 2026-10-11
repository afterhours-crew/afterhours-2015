// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Stats groups backed by the installed persistent-table schema. The local
//! service exposes progression and reputation; column names and defaults are
//! read from the installation, while the integer display convention is policy.
use crate::Error;
use serde_json::{Value, json};

pub fn build(tables: &[u8]) -> Result<Vec<u8>, Error> {
    let document: Value = serde_json::from_slice(tables)
        .map_err(|_| Error::Content("persistent tables are not JSON".into()))?;
    let tables = document["tables"]
        .as_array()
        .ok_or_else(|| Error::Content("persistent tables are missing".into()))?;
    let mut groups = Vec::new();
    for table in ["ProgressionStats", "RepValuesTable"] {
        let schema = tables
            .iter()
            .find(|s| s["name"] == table)
            .ok_or_else(|| Error::Content(format!("Stats table {table} is missing")))?;
        if schema["using_stores"] != true {
            return Err(Error::Content(format!(
                "Stats table {table} is not a store"
            )));
        }
        let group = format!("Persistence_{table}");
        let mut stats = Vec::new();
        for column in schema["columns"]
            .as_array()
            .ok_or_else(|| Error::Content(format!("Stats table {table} has no columns")))?
        {
            let name = column["name"]
                .as_str()
                .ok_or_else(|| Error::Content("Stats column name is missing".into()))?;
            if column["value_type"] != "int" {
                return Err(Error::Content(format!(
                    "Stats {table}.{name} is not an integer"
                )));
            }
            let text = column["default"]
                .as_str()
                .ok_or_else(|| Error::Content("Stats column default is missing".into()))?;
            // Match the persistent loader's integer fallback, including empty defaults.
            let default = text.parse::<i32>().unwrap_or(0).to_string();
            stats.push(json!({
                "category": group, "default": default, "derived": false,
                "format": "%d", "kind": "", "long_desc": name, "metadata": "",
                "name": name, "short_desc": name, "stat_type": 0,
            }));
        }
        groups.push(json!({
            "name": group, "table": table, "category": group, "description": group,
            "metadata": "", "entity_type": [30722, 1], "stats": stats,
        }));
    }
    let result = json!({
        "format": "nfs-stat-definitions", "version": 1,
        "build_sha256": nfs_services::SUPPORTED_BUILD_SHA256, "groups": groups,
    });
    nfs_services::stats::Catalog::from_json(&result)
        .map_err(|e| Error::Content(format!("Stats definitions rejected: {e}")))?;
    serde_json::to_vec(&result).map_err(|_| Error::Content("Stats encoding".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> Value {
        json!({"tables": [
            {"name":"ProgressionStats", "using_stores":true, "columns":[
                {"name":"SyntheticStage", "value_type":"int", "default":"7"}]},
            {"name":"RepValuesTable", "using_stores":true, "columns":[
                {"name":"SyntheticPoints", "value_type":"int", "default":""},
                {"name":"SyntheticDebt", "value_type":"int", "default":"-3"}]}
        ]})
    }

    #[test]
    fn names_order_and_defaults_come_from_the_tables() {
        let result = build(&serde_json::to_vec(&tables()).unwrap()).unwrap();
        let result: Value = serde_json::from_slice(&result).unwrap();
        let progression = &result["groups"][0]["stats"][0];
        assert_eq!(progression["name"], "SyntheticStage");
        assert_eq!(progression["default"], "7");
        assert_eq!(result["groups"][1]["stats"][0]["default"], "0");
        assert_eq!(result["groups"][1]["stats"][1]["name"], "SyntheticDebt");
        assert_eq!(result["groups"][1]["stats"][1]["default"], "-3");
        assert_eq!(
            nfs_services::stats::Catalog::from_json(&result)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn missing_or_non_integer_stores_are_rejected() {
        let mut missing = tables();
        missing["tables"].as_array_mut().unwrap().pop();
        assert!(build(&serde_json::to_vec(&missing).unwrap()).is_err());
        for (key, value) in [("value_type", json!("float")), ("name", json!("bad name"))] {
            let mut malformed = tables();
            malformed["tables"][0]["columns"][0][key] = value;
            assert!(build(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
        let mut rows = tables();
        rows["tables"][0]["using_stores"] = json!(false);
        assert!(build(&serde_json::to_vec(&rows).unwrap()).is_err());
    }
}
