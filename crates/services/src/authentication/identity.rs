// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::Identity;
use crate::ContentError;
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};
impl Identity {
    /// Private operator-owned profile mapping. The edge must compare its storage
    /// account with the account selected for this connection before serving it.
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > 4096 {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    pub fn from_json(v: &Value) -> Result<Self, ContentError> {
        let fields = ["version", "storage_account", "persona", "account", "name"];
        if v.as_object()
            .is_none_or(|o| o.len() != fields.len() || !fields.iter().all(|k| o.contains_key(*k)))
            || v["version"].as_u64() != Some(1)
        {
            return Err(ContentError::Invalid);
        }
        let text = v["storage_account"]
            .as_str()
            .filter(|s| s.len() == 32 && s.bytes().all(|v| v.is_ascii_hexdigit()))
            .ok_or(ContentError::Invalid)?;
        let mut id = [0; 16];
        for (i, byte) in id.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
                .map_err(|_| ContentError::Invalid)?;
        }
        let storage =
            nfs_storage::AccountId::from_owned_config(id).map_err(|_| ContentError::Invalid)?;
        let name = v["name"]
            .as_str()
            .filter(|s| s.len() <= 64)
            .ok_or(ContentError::Invalid)?;
        Self::new(
            storage,
            v["persona"].as_i64().ok_or(ContentError::Invalid)?,
            v["account"].as_i64().ok_or(ContentError::Invalid)?,
            name.into(),
        )
        .map_err(|_| ContentError::Invalid)
    }
}
