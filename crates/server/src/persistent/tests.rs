// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_services::SUPPORTED_BUILD_SHA256 as BUILD;
use serde_json::Value as Json;
const GARAGE: &str = "GarageItemsTable";
const GARAGE_COLUMNS: [&str; 5] = [
    "PrimaryVehicleItem",
    "Vehicle1",
    "Vehicle2",
    "Vehicle3",
    "Vehicle4",
];
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

use serde_json::json;
pub(crate) fn content() -> Json {
    let tables: Vec<_> = REQUIRED.iter().map(|&(table, secondary, count)| {
        let columns: Vec<_> = (0..count).map(|i| {
            let name = if table == GARAGE { GARAGE_COLUMNS[i].to_string() } else { format!("Column{i}") };
            json!({"key":key(&name), "name":name, "value_type":if table == GARAGE {"string"} else {"int"},"default":"0"})
        }).collect();
        json!({"name":table,"key":key(table),"secondary_key":secondary,"using_stores":secondary.is_none(),"write_whole_rows":false,"columns":columns})
    }).collect();
    json!({"version":1,"build_sha256":BUILD,"source":{"path":"synthetic-test","sha256":"00".repeat(32)},"tables":tables})
}
