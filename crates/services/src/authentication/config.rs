// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::Error;
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

/// Deployment policy only. No account IDs, credentials, addresses or reply data.
#[derive(Clone, Debug)]
pub struct Config {
    pub first_console: bool,
    pub first_login: bool,
    pub locale: u32,
    pub namespace: Vec<u8>,
    pub session_type: i64,
    pub object_parts: [u16; 2],
    pub filter: Vec<u8>,
    pub no_toggle_ok: Vec<u8>,
    pub use_server_time: Vec<u8>,
    pub service_name: Vec<u8>,
    pub ticker_key_length: usize,
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        let text =
            |s: &[u8]| !s.is_empty() && s.len() <= 256 && s.iter().all(|v| (32..=126).contains(v));
        if self.namespace.is_empty()
            || self.namespace.len() > 64
            || !self.namespace.iter().all(u8::is_ascii_graphic)
            || ![
                &self.filter,
                &self.no_toggle_ok,
                &self.use_server_time,
                &self.service_name,
            ]
            .iter()
            .all(|s| text(s))
            || !matches!(self.ticker_key_length, 57 | 58)
        {
            return Err(Error::Config);
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let file = File::open(path).map_err(|_| ContentError::Io)?;
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > 4096 {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    pub fn from_json(v: &Value) -> Result<Self, ContentError> {
        let fields = [
            "version",
            "build_sha256",
            "first_console",
            "first_login",
            "locale",
            "namespace",
            "session_type",
            "object_parts",
            "filter",
            "no_toggle_ok",
            "use_server_time",
            "service_name",
            "ticker_key_length",
        ];
        if v.as_object()
            .is_none_or(|o| o.len() != fields.len() || !fields.iter().all(|k| o.contains_key(*k)))
            || v["version"].as_u64() != Some(1)
            || v["build_sha256"].as_str() != Some(SUPPORTED_BUILD_SHA256)
        {
            return Err(ContentError::Invalid);
        }
        let text = |k: &str| {
            v[k].as_str()
                .filter(|s| s.len() <= 256)
                .map(|s| s.as_bytes().to_vec())
                .ok_or(ContentError::Invalid)
        };
        let boolean = |k: &str| v[k].as_bool().ok_or(ContentError::Invalid);
        let parts = v["object_parts"]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or(ContentError::Invalid)?;
        let number = |v: &Value| {
            v.as_u64()
                .and_then(|v| u16::try_from(v).ok())
                .ok_or(ContentError::Invalid)
        };
        let config = Self {
            first_console: boolean("first_console")?,
            first_login: boolean("first_login")?,
            locale: v["locale"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or(ContentError::Invalid)?,
            namespace: text("namespace")?,
            session_type: v["session_type"].as_i64().ok_or(ContentError::Invalid)?,
            object_parts: [number(&parts[0])?, number(&parts[1])?],
            filter: text("filter")?,
            no_toggle_ok: text("no_toggle_ok")?,
            use_server_time: text("use_server_time")?,
            service_name: text("service_name")?,
            ticker_key_length: v["ticker_key_length"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or(ContentError::Invalid)?,
        };
        config.validate().map_err(|_| ContentError::Invalid)?;
        Ok(config)
    }
}
