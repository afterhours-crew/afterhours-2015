// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed Stats scopes plus the local AllDrive menu policy. SpeedList
//! offerings are operator policy and are empty in this default catalog.
use crate::{Error, scan::Assets};
use nfs_frostbite::ebx::Value;
use nfs_services::control_catalogs::{Catalog, MAX_PAIRS_PER_SCOPE, MAX_SCOPES};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn ranges(ids: BTreeSet<u32>) -> Vec<[i64; 2]> {
    let mut result: Vec<[i64; 2]> = Vec::new();
    for id in ids {
        let id = i64::from(id);
        if let Some(last) = result.last_mut().filter(|last| last[1] + 1 == id) {
            last[1] = id;
        } else {
            result.push([id, id]);
        }
    }
    result
}

pub fn build(assets: &mut Assets) -> Result<Vec<u8>, Error> {
    let walls = assets.named("SpeedWallAsset");
    if walls.is_empty() || walls.len() >= MAX_SCOPES {
        return Err(Error::Content(
            "missing or too many SpeedWall assets".into(),
        ));
    }
    let mut scopes = BTreeMap::from([("PersistenceSecondaryKey".to_owned(), vec![[0, i64::MAX]])]);
    for name in walls {
        let wall = assets
            .objects(&name)?
            .first()
            .cloned()
            .ok_or_else(|| Error::Content("missing SpeedWall object".into()))?;
        let category = wall
            .fields
            .get("CategoryName")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Content("missing SpeedWall category".into()))?;
        let modes = wall
            .fields
            .get("GameModeIds")
            .and_then(Value::as_array)
            .filter(|m| m.len() <= MAX_PAIRS_PER_SCOPE)
            .ok_or_else(|| Error::Content("missing or oversized SpeedWall game modes".into()))?;
        let mut ids = BTreeSet::new();
        for mode in modes {
            let (_, mode) = assets.resolve(mode.as_pointer())?;
            if mode.class != "GameModeId" {
                return Err(Error::Content(
                    "unexpected SpeedWall game-mode class".into(),
                ));
            }
            let id = match mode.fields.get("IdHash") {
                Some(Value::UInt32(id)) => *id,
                Some(Value::Int32(id)) => *id as u32,
                _ => return Err(Error::Content("missing game-mode IdHash".into())),
            };
            ids.insert(id);
        }
        if scopes.insert(category.to_owned(), ranges(ids)).is_some() {
            return Err(Error::Content("duplicate Stats scope".into()));
        }
    }
    let scopes: Vec<_> = scopes
        .into_iter()
        .map(|(name, values)| {
            json!({
                "name": name, "aggregate": false, "aggregate_key": 0, "values": values,
            })
        })
        .collect();
    let document = json!({"format":"nfs-control-catalogs", "version":1,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "key_scopes":scopes, "kill_switches":["autolog.recentPlayers"],
        "speed_list_types":[], "time_limited_features":"disabled"});
    Catalog::from_json(&document)
        .map_err(|e| Error::Content(format!("control catalog rejected: {e}")))?;
    serde_json::to_vec(&document).map_err(|_| Error::Content("control catalog encoding".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_ranges_merge_only_adjacent_unsigned_ids() {
        assert_eq!(
            ranges(BTreeSet::from([11, 10, 11, 99, u32::MAX, u32::MAX - 1])),
            [
                [10, 11],
                [99, 99],
                [i64::from(u32::MAX) - 1, i64::from(u32::MAX)]
            ]
        );
        assert!(ranges(BTreeSet::new()).is_empty());
    }
}
