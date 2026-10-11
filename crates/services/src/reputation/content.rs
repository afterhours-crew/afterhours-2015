// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Versioned installation content, separate from account scores and garage logic.
use super::{MAX_THRESHOLDS, Thresholds};
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

impl Thresholds {
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > 16384 {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }

    pub fn from_json(value: &Value) -> Result<Self, ContentError> {
        let object = value.as_object().ok_or(ContentError::Invalid)?;
        if object.len() != 4
            || value["format"] != "nfs-reputation-thresholds"
            || value["version"] != 1
            || value["build_sha256"] != SUPPORTED_BUILD_SHA256
        {
            return Err(ContentError::Invalid);
        }
        let rows = value["thresholds"]
            .as_array()
            .filter(|a| a.len() <= MAX_THRESHOLDS)
            .ok_or(ContentError::Invalid)?;
        let values = rows
            .iter()
            .map(|v| {
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or(ContentError::Invalid)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(values).map_err(|_| ContentError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_rejects_wrong_build_shape_and_non_monotone_values() {
        let good = json!({"format":"nfs-reputation-thresholds","version":1,
            "build_sha256":SUPPORTED_BUILD_SHA256,"thresholds":[0,7,42]});
        assert!(Thresholds::from_json(&good).is_ok());
        for (field, invalid) in [
            ("version", json!(2)),
            ("build_sha256", json!("other")),
            ("format", json!("other")),
            ("extra", json!(true)),
            ("thresholds", json!([0, 7, 7])),
            ("thresholds", json!([-1, 7])),
            ("thresholds", json!([0, 2147483648u64])),
            ("thresholds", json!([0, "7"])),
            ("thresholds", json!([0, 7.5])),
            ("thresholds", json!([])),
            ("thresholds", json!((0..257).collect::<Vec<_>>())),
        ] {
            let mut value = good.clone();
            value[field] = invalid;
            assert!(Thresholds::from_json(&value).is_err(), "{field}");
        }
    }
}
