// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! License vocabulary from the client's active Win32 entitlement queries.
//! These are recognized names, never a player's grants or proof of ownership.
use crate::{Error, items::ITEM_SYSTEM, scan::Assets};
use nfs_frostbite::ebx::Value;
use nfs_services::item_licenses::{Content, MAX_LICENSES};
use serde_json::json;
use std::collections::BTreeSet;

pub const SERVICE: &str = "configurations/onlinesettings/onlineservices/entitlementservice";

pub fn build(assets: &mut Assets) -> Result<Vec<u8>, Error> {
    let system = assets
        .objects(ITEM_SYSTEM)?
        .first()
        .filter(|o| o.class == "ItemSystemData")
        .and_then(|o| o.fields.get("Name"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Content("item system name is missing".into()))?
        .to_owned();
    let queries = assets
        .objects(SERVICE)?
        .first()
        .filter(|o| o.class == "PresenceEntitlementServiceData")
        .and_then(|o| o.fields.get("EntitlementQueries"))
        .and_then(Value::as_array)
        .filter(|q| !q.is_empty() && q.len() <= 32)
        .ok_or_else(|| {
            Error::Content("client entitlement queries are missing or oversized".into())
        })?
        .to_vec();
    let mut licenses = BTreeSet::new();
    let mut win32 = false;
    for query in queries {
        let (_, query) = assets.resolve(query.as_pointer())?;
        if query.class != "EntitlementQuery" {
            return Err(Error::Content("unexpected entitlement query class".into()));
        }
        let platform = query
            .fields
            .get("Platform")
            .and_then(Value::as_enum_name)
            .ok_or_else(|| Error::Content("entitlement query platform is missing".into()))?;
        if platform != "GamePlatform_Win32" {
            continue;
        }
        win32 = true;
        let rows = query
            .fields
            .get("EntitlementsData")
            .and_then(Value::as_array)
            .filter(|rows| rows.len() <= 256)
            .ok_or_else(|| {
                Error::Content("entitlement query data is missing or oversized".into())
            })?;
        for row in rows {
            let license = row
                .as_fields()
                .and_then(|r| r.get("License"))
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Content("entitlement license is missing".into()))?;
            licenses.insert(license.to_owned());
            if licenses.len() > MAX_LICENSES {
                return Err(Error::Content("too many known licenses".into()));
            }
        }
    }
    if !win32 {
        return Err(Error::Content("no Win32 entitlement queries".into()));
    }
    let licenses: Vec<_> = licenses
        .into_iter()
        .map(|license| json!({"license":license,"source":"Entitlement"}))
        .collect();
    let document = json!({"format":"nfs-item-licenses","version":2,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "item_system":system,"licenses":licenses});
    Content::from_json(&document)
        .map_err(|e| Error::Content(format!("license vocabulary rejected: {e}")))?;
    serde_json::to_vec(&document).map_err(|_| Error::Content("license vocabulary encoding".into()))
}
