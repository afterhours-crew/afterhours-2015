// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Follow the reputation calculator's threshold-asset reference.
use crate::{Error, scan::Assets};
use nfs_frostbite::ebx::Value;
use nfs_services::reputation::{MAX_THRESHOLDS, Thresholds};
use serde_json::json;

pub(crate) const CALCULATOR: &str = "gameplay/scoring/replevelcalculationlogic";

pub fn build(assets: &mut Assets) -> Result<Vec<u8>, Error> {
    let objects = assets.objects(CALCULATOR)?;
    let mut calculators = objects
        .iter()
        .filter(|o| o.class == "IntScoreToIntLevelEntityData");
    let calculator = calculators
        .next()
        .ok_or_else(|| Error::Content("reputation calculator is missing".into()))?;
    let pointer = calculator
        .fields
        .get("LevelThresholdsAsset")
        .and_then(Value::as_pointer);
    if calculators.next().is_some() {
        return Err(Error::Content("reputation calculator is ambiguous".into()));
    }
    let (_, asset) = assets.resolve(pointer)?;
    if asset.class != "IntScoreToIntLevelAsset" {
        return Err(Error::Content(
            "reputation threshold reference has the wrong class".into(),
        ));
    }
    let rows = asset
        .fields
        .get("LevelThresholds")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= MAX_THRESHOLDS)
        .ok_or_else(|| Error::Content("reputation thresholds are missing or too large".into()))?;
    let values = rows
        .iter()
        .map(|v| match v {
            Value::Int32(n) => Ok(*n),
            _ => Err(Error::Content(
                "reputation threshold is not an Int32".into(),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = json!({"format":"nfs-reputation-thresholds","version":1,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"thresholds":values});
    Thresholds::from_json(&result)
        .map_err(|e| Error::Content(format!("reputation thresholds rejected: {e}")))?;
    serde_json::to_vec(&result).map_err(|_| Error::Content("reputation encoding".into()))
}
