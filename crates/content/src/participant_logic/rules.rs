// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{Error, graph::bad};
use nfs_world::replication::entity::Kind;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Executable-specific constructor and connection contracts. No instance
/// indexes, channel targets, account values or lifecycle phases belong here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rules {
    pub classes: BTreeMap<String, Option<Kind>>,
    pub spawners: BTreeSet<String>,
    pub root_channels: u32,
    pub spawner_channels: u32,
    pub boolean_ports: BTreeMap<String, u32>,
}
impl Rules {
    pub fn from_json(v: &Value) -> Result<Self, Error> {
        let names = [
            "classes",
            "spawners",
            "root_channels",
            "spawner_channels",
            "boolean_ports",
        ];
        let map = v
            .as_object()
            .filter(|m| m.len() == names.len() && names.iter().all(|k| m.contains_key(*k)))
            .ok_or_else(|| bad("participant rules fields"))?;
        fn name(v: &str) -> Result<String, Error> {
            if v.is_empty()
                || v.len() > 128
                || !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(bad("participant class name"));
            }
            Ok(v.into())
        }
        let entries = map["classes"]
            .as_object()
            .filter(|m| !m.is_empty() && m.len() <= 2048)
            .ok_or_else(|| bad("participant class rules"))?;
        let mut classes = BTreeMap::new();
        for (class, role) in entries {
            let role = match role.as_str() {
                None if role.is_null() => None,
                Some("root") => Some(Kind::Root),
                Some("channel") => Some(Kind::RpcReference),
                Some("rpc") => Some(Kind::Rpc),
                Some("bool") => Some(Kind::BoolProperty),
                Some("int") => Some(Kind::I32Property),
                Some("variant") => Some(Kind::RpcVariant),
                _ => return Err(bad("participant serializer kind")),
            };
            classes.insert(name(class)?, role);
        }
        let values = map["spawners"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 16)
            .ok_or_else(|| bad("participant spawner classes"))?;
        let mut spawners = BTreeSet::new();
        for value in values {
            if !spawners.insert(name(
                value
                    .as_str()
                    .ok_or_else(|| bad("participant spawner name"))?,
            )?) {
                return Err(bad("duplicate participant spawner class"));
            }
        }
        let number = |key: &str| {
            map[key]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n != 0)
                .ok_or_else(|| bad("participant port"))
        };
        Ok(Self {
            classes,
            spawners,
            root_channels: number("root_channels")?,
            spawner_channels: number("spawner_channels")?,
            boolean_ports: super::boolean::ports(&map["boolean_ports"])?,
        })
    }
    pub fn to_json(&self) -> Value {
        let classes: BTreeMap<_, _> = self
            .classes
            .iter()
            .map(|(name, kind)| {
                (
                    name,
                    kind.map(|k| match k {
                        Kind::Root => "root",
                        Kind::RpcReference => "channel",
                        Kind::Rpc => "rpc",
                        Kind::BoolProperty => "bool",
                        Kind::I32Property => "int",
                        Kind::RpcVariant => "variant",
                        _ => "unsupported",
                    }),
                )
            })
            .collect();
        json!({"classes":classes,"spawners":self.spawners,"root_channels":self.root_channels,"spawner_channels":self.spawner_channels,"boolean_ports":self.boolean_ports})
    }
}
