// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{Error, graph::bad};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Named sequence roles and executable-specific construction/link contracts.
/// Instance indices, catalogs, channel targets and goal defaults are extracted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rules {
    pub waiting: String,
    pub exit: String,
    pub entry: String,
    pub entry_timer_events: [u32; 4],
    pub waiting_scene: String,
    pub exit_scene: String,
    pub manager_scene: String,
    pub classes: BTreeMap<String, Option<String>>,
    pub root_channels: u32,
    pub spawner_channels: u32,
    pub spline_link: u32,
    pub spline_class_id: u32,
}
impl Rules {
    pub fn from_json(v: &Value) -> Result<Self, Error> {
        const NAMES: &[&str] = &[
            "waiting",
            "exit",
            "entry",
            "entry_timer_events",
            "waiting_scene",
            "exit_scene",
            "manager_scene",
            "classes",
            "root_channels",
            "spawner_channels",
            "spline_link",
            "spline_class_id",
        ];
        let map = v
            .as_object()
            .filter(|m| m.len() == NAMES.len() && NAMES.iter().all(|k| m.contains_key(*k)))
            .ok_or_else(|| bad("sequence rules fields"))?;
        let path = |key: &str| -> Result<String, Error> {
            map[key]
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
                .ok_or_else(|| bad("sequence rules path"))
        };
        let number = |key: &str| -> Result<u32, Error> {
            map[key]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v != 0)
                .ok_or_else(|| bad("sequence rules identifier"))
        };
        let entries = map["classes"]
            .as_object()
            .filter(|m| !m.is_empty() && m.len() <= 2048)
            .ok_or_else(|| bad("sequence class rules"))?;
        let mut classes = BTreeMap::new();
        for (name, role) in entries {
            if name.is_empty()
                || name.len() > 128
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(bad("sequence class name"));
            }
            let role = if role.is_null() {
                None
            } else {
                Some(
                    role.as_str()
                        .filter(|r| ["root", "provider", "event", "channel"].contains(r))
                        .ok_or_else(|| bad("sequence serializer role"))?
                        .to_owned(),
                )
            };
            classes.insert(name.clone(), role);
        }
        let result = Self {
            waiting: path("waiting")?,
            exit: path("exit")?,
            entry: path("entry")?,
            entry_timer_events: map["entry_timer_events"]
                .as_array()
                .filter(|a| a.len() == 4)
                .ok_or_else(|| bad("entry timer events"))?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| *n != 0)
                        .ok_or_else(|| bad("entry timer event"))
                })
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| bad("entry timer events"))?,
            waiting_scene: path("waiting_scene")?,
            exit_scene: path("exit_scene")?,
            manager_scene: path("manager_scene")?,
            classes,
            root_channels: number("root_channels")?,
            spawner_channels: number("spawner_channels")?,
            spline_link: number("spline_link")?,
            spline_class_id: number("spline_class_id")?,
        };
        if result.waiting == result.exit
            || result.entry == result.exit
            || result.entry == result.waiting
        {
            return Err(bad("duplicate sequence roots"));
        }
        Ok(result)
    }
    pub fn to_json(&self) -> Value {
        json!({"waiting":self.waiting,"exit":self.exit,"entry":self.entry,"entry_timer_events":self.entry_timer_events,"waiting_scene":self.waiting_scene,"exit_scene":self.exit_scene,"manager_scene":self.manager_scene,
            "classes":self.classes,"root_channels":self.root_channels,"spawner_channels":self.spawner_channels,"spline_link":self.spline_link,"spline_class_id":self.spline_class_id})
    }
}
