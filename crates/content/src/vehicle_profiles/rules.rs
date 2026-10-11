// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Error;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Class construction and link contracts for the selected executable build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rules {
    pub classes: BTreeMap<String, Option<String>>,
    pub bus_classes: BTreeSet<String>,
    pub mesh_classes: BTreeMap<String, String>,
    pub mesh_link: u32,
    /// Installed player bundles; traffic has a separate owner and constructor.
    pub bundle_prefix: String,
}

impl Rules {
    pub fn from_json(value: &Value) -> Result<Self, Error> {
        let bad = || Error::Content("invalid vehicle construction rules".into());
        let o = value.as_object().filter(|o| o.len() == 5).ok_or_else(bad)?;
        if ![
            "classes",
            "bus_classes",
            "mesh_classes",
            "mesh_link",
            "bundle_prefix",
        ]
        .iter()
        .all(|k| o.contains_key(*k))
        {
            return Err(bad());
        }
        let class_name = |v: &str| {
            !v.is_empty()
                && v.len() <= 128
                && v.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        };
        let entries = value["classes"]
            .as_object()
            .filter(|o| !o.is_empty() && o.len() <= 2048)
            .ok_or_else(bad)?;
        let kinds = [
            "root",
            "chassis",
            "part",
            "triple_nibbles",
            "bool",
            "wheel",
            "index",
            "tuning",
            "appearance",
            "four_bit",
            "tagged",
            "nibbles_guid",
            "mesh",
            "guid_float",
        ];
        let mut classes = BTreeMap::new();
        for (name, value) in entries {
            if !class_name(name) {
                return Err(bad());
            }
            let kind = if value.is_null() {
                None
            } else {
                Some(
                    value
                        .as_str()
                        .filter(|v| kinds.contains(v))
                        .ok_or_else(bad)?
                        .to_owned(),
                )
            };
            classes.insert(name.clone(), kind);
        }
        let mut bus_classes = BTreeSet::new();
        for value in value["bus_classes"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 128)
            .ok_or_else(bad)?
        {
            let name = value.as_str().filter(|n| class_name(n)).ok_or_else(bad)?;
            if !classes.contains_key(name) || !bus_classes.insert(name.to_owned()) {
                return Err(bad());
            }
        }
        let mut mesh_classes = BTreeMap::new();
        for (name, value) in value["mesh_classes"]
            .as_object()
            .filter(|o| !o.is_empty() && o.len() <= 128)
            .ok_or_else(bad)?
        {
            let extra = value
                .as_str()
                .filter(|v| ["none", "rim", "light", "spoiler"].contains(v))
                .ok_or_else(bad)?;
            if !classes.contains_key(name) {
                return Err(bad());
            }
            mesh_classes.insert(name.clone(), extra.to_owned());
        }
        let mesh_link = value["mesh_link"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v != 0)
            .ok_or_else(bad)?;
        let bundle_prefix = value["bundle_prefix"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 256
                    && s.ends_with('/')
                    && !s.starts_with('/')
                    && !s.contains("..")
                    && s.bytes().all(|c| c.is_ascii_lowercase() || c == b'/')
            })
            .ok_or_else(bad)?
            .to_owned();
        Ok(Self {
            classes,
            bus_classes,
            mesh_classes,
            mesh_link,
            bundle_prefix,
        })
    }
    pub fn to_json(&self) -> Value {
        json!({"classes":self.classes,"bus_classes":self.bus_classes,
            "mesh_classes":self.mesh_classes,"mesh_link":self.mesh_link,
            "bundle_prefix":self.bundle_prefix})
    }
}
