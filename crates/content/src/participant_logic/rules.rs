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
    pub integer_ports: BTreeMap<String, u32>,
}
impl Rules {
    pub fn from_json(v: &Value) -> Result<Self, Error> {
        let names = [
            "classes",
            "spawners",
            "root_channels",
            "spawner_channels",
            "boolean_ports",
            "integer_ports",
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
                Some("rpc_bool") => Some(Kind::RpcBool),
                Some("rpc_references") => Some(Kind::RpcReferences),
                Some("bool") => Some(Kind::BoolProperty),
                Some("int") => Some(Kind::I32Property),
                Some("float") => Some(Kind::FloatProperty),
                Some("variant") => Some(Kind::RpcVariant),
                Some("string") => Some(Kind::StringProperty),
                Some("game_mode_root") => Some(Kind::GameModeRoot),
                Some("countdown_state") => Some(Kind::CountdownState),
                Some("waypoints_state") => Some(Kind::WaypointsState),
                Some("dnf_transition") => Some(Kind::DnfTransition),
                Some("waypoint_list") => Some(Kind::WaypointList),
                Some("rpc_guid") => Some(Kind::RpcGuid),
                Some("rpc_set64") => Some(Kind::RpcSet64),
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
            integer_ports: super::integer::ports(&map["integer_ports"])?,
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
                        Kind::RpcBool => "rpc_bool",
                        Kind::RpcReferences => "rpc_references",
                        Kind::BoolProperty => "bool",
                        Kind::I32Property => "int",
                        Kind::FloatProperty => "float",
                        Kind::RpcVariant => "variant",
                        Kind::StringProperty => "string",
                        Kind::GameModeRoot => "game_mode_root",
                        Kind::CountdownState => "countdown_state",
                        Kind::WaypointsState => "waypoints_state",
                        Kind::DnfTransition => "dnf_transition",
                        Kind::WaypointList => "waypoint_list",
                        Kind::RpcGuid => "rpc_guid",
                        Kind::RpcSet64 => "rpc_set64",
                        _ => "unsupported",
                    }),
                )
            })
            .collect();
        json!({"classes":classes,"spawners":self.spawners,"root_channels":self.root_channels,"spawner_channels":self.spawner_channels,"boolean_ports":self.boolean_ports,"integer_ports":self.integer_ports})
    }
}
