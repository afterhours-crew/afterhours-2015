// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::graph::bad;
use crate::Error;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Executable-specific progression construction and connection contracts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rules {
    pub main: String,
    pub intro: String,
    pub scene: String,
    pub classes: BTreeMap<String, Option<String>>,
    pub ports: BTreeMap<String, u32>,
}
pub(super) const PORTS: &[&str] = &[
    "objectives",
    "ints",
    "floats",
    "dynamic_floats",
    "stable_id",
    "active",
    "completed",
    "bool_value",
    "logic_output",
    "not_input",
    "setting_output",
    "participant_output",
    "participant_channel",
    "spawner_channels",
    "root_channels",
];
impl Rules {
    pub fn from_json(v: &Value) -> Result<Self, Error> {
        let object = v.as_object().ok_or_else(|| bad("rules object"))?;
        if object.len() != 5
            || !["main", "intro", "scene", "classes", "ports"]
                .iter()
                .all(|k| object.contains_key(*k))
        {
            return Err(bad("rules fields"));
        }
        let path = |key: &str| -> Result<String, Error> {
            v[key]
                .as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 512
                        && !s.starts_with('/')
                        && !s.contains("..")
                        && s.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'/' | b'_')
                        })
                })
                .map(str::to_owned)
                .ok_or_else(|| bad("rules asset path"))
        };
        let entries = v["classes"]
            .as_object()
            .filter(|m| !m.is_empty() && m.len() <= 2048)
            .ok_or_else(|| bad("rules classes"))?;
        let mut classes = BTreeMap::new();
        for (name, value) in entries {
            if name.is_empty()
                || name.len() > 128
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(bad("rules class name"));
            }
            let role = if value.is_null() {
                None
            } else {
                Some(
                    value
                        .as_str()
                        .filter(|s| {
                            [
                                "root",
                                "objective",
                                "rpc",
                                "launcher",
                                "bool",
                                "int",
                                "channel",
                            ]
                            .contains(s)
                        })
                        .ok_or_else(|| bad("rules role"))?
                        .to_owned(),
                )
            };
            classes.insert(name.clone(), role);
        }
        let values = v["ports"]
            .as_object()
            .filter(|m| m.len() == PORTS.len())
            .ok_or_else(|| bad("rules ports"))?;
        let ports = PORTS
            .iter()
            .map(|name| {
                Ok((
                    (*name).to_owned(),
                    values
                        .get(*name)
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| *n != 0)
                        .ok_or_else(|| bad("rules port"))?,
                ))
            })
            .collect::<Result<_, Error>>()?;
        let result = Self {
            main: path("main")?,
            intro: path("intro")?,
            scene: path("scene")?,
            classes,
            ports,
        };
        if result.main == result.intro {
            return Err(bad("duplicate progression root"));
        }
        Ok(result)
    }
    pub fn to_json(&self) -> Value {
        json!({"main":self.main,"intro":self.intro,"scene":self.scene,"classes":self.classes,"ports":self.ports})
    }
}
