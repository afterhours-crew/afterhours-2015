// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Branch, ParseError};
use nfs_world::replication::entity::{
    Kind, MAX_SERIALIZERS, Profile,
    creation::{Asset, Catalog, Content, MAX_CATALOGS, MAX_PROFILES, MAX_TYPES},
};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

pub const SUB_ID: u32 = 1;
const MAX_SELECTORS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Root,
    Channel {
        scene_content_key: u32,
        scene_serializer: usize,
    },
    Objective {
        id: u64,
    },
    Rpc,
    Launcher,
    Bool,
    Int,
}
impl Role {
    fn kind(self) -> Kind {
        match self {
            Self::Root => Kind::Root,
            Self::Channel { .. } => Kind::RpcReference,
            Self::Objective { .. } => Kind::RpcFlags,
            Self::Rpc | Self::Launcher => Kind::Rpc,
            Self::Bool => Kind::BoolProperty,
            Self::Int => Kind::I32Property,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blueprint {
    asset: Asset,
    bus_count: u16,
    roles: Vec<Role>,
}
impl Blueprint {
    pub fn asset(&self) -> Asset {
        self.asset
    }
    pub fn bus_count(&self) -> u16 {
        self.bus_count
    }
    pub fn roles(&self) -> &[Role] {
        &self.roles
    }
    pub fn objective_ids(&self) -> BTreeSet<u64> {
        self.roles
            .iter()
            .filter_map(|role| match role {
                Role::Objective { id } => Some(*id),
                _ => None,
            })
            .collect()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Construction {
    content: Content,
    branches: BTreeMap<Branch, Blueprint>,
}

fn fields(v: &Json, names: &[&str]) -> Result<(), ParseError> {
    let o = v.as_object().ok_or(ParseError::Invalid)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(ParseError::Invalid);
    }
    Ok(())
}
fn word(v: &Json) -> Result<u16, ParseError> {
    u16::try_from(v.as_u64().ok_or(ParseError::Invalid)?).map_err(|_| ParseError::Invalid)
}
fn asset(v: &Json) -> Result<Asset, ParseError> {
    fields(v, &["bundle", "type_id", "local_index"])?;
    Ok(Asset {
        bundle: word(&v["bundle"])?,
        type_id: word(&v["type_id"])?,
        local_index: word(&v["local_index"])?,
    })
}
fn kind(v: &Json) -> Result<Kind, ParseError> {
    match v.as_str() {
        Some("root") => Ok(Kind::Root),
        Some("rpc") => Ok(Kind::Rpc),
        Some("rpc_reference") => Ok(Kind::RpcReference),
        Some("rpc_flags") => Ok(Kind::RpcFlags),
        Some("bool_property") => Ok(Kind::BoolProperty),
        Some("i32_property") => Ok(Kind::I32Property),
        Some("float_property") => Ok(Kind::FloatProperty),
        Some("rpc_variant") => Ok(Kind::RpcVariant),
        Some("rpc_ghost_reference") => Ok(Kind::RpcGhostReference),
        _ => Err(ParseError::Invalid),
    }
}
pub fn content(v: &Json) -> Result<Content, ParseError> {
    fields(v, &["catalogs", "profiles"])?;
    let catalogs = v["catalogs"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_CATALOGS)
        .ok_or(ParseError::Invalid)?;
    let profiles = v["profiles"]
        .as_array()
        .filter(|a| a.len() <= MAX_PROFILES)
        .ok_or(ParseError::Invalid)?;
    let catalogs = catalogs
        .iter()
        .map(|c| {
            fields(c, &["bundle", "entries"])?;
            let rows = c["entries"]
                .as_array()
                .filter(|a| a.len() <= MAX_TYPES)
                .ok_or(ParseError::Invalid)?;
            let entries = rows
                .iter()
                .map(|r| {
                    let pair = r
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or(ParseError::Invalid)?;
                    Ok((word(&pair[0])?, word(&pair[1])?))
                })
                .collect::<Result<Vec<_>, ParseError>>()?;
            Catalog::new(word(&c["bundle"])?, &entries).map_err(|_| ParseError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let profiles = profiles
        .iter()
        .map(|p| {
            fields(p, &["asset", "serializers"])?;
            let kinds = p["serializers"]
                .as_array()
                .filter(|a| a.len() <= MAX_SERIALIZERS)
                .ok_or(ParseError::Invalid)?
                .iter()
                .map(kind)
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                asset(&p["asset"])?,
                Profile::new(&kinds).map_err(|_| ParseError::Invalid)?,
            ))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    Content::new(catalogs, profiles).map_err(|_| ParseError::Invalid)
}
fn role(v: &Json) -> Result<Role, ParseError> {
    let name = v["kind"].as_str().ok_or(ParseError::Invalid)?;
    Ok(match name {
        "root" => {
            fields(v, &["kind"])?;
            Role::Root
        }
        "channel" => {
            fields(v, &["kind", "scene_content_key", "scene_serializer"])?;
            let key = v["scene_content_key"]
                .as_u64()
                .and_then(|k| u32::try_from(k).ok())
                .filter(|k| *k != 0)
                .ok_or(ParseError::Invalid)?;
            let index = v["scene_serializer"]
                .as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| (1..MAX_SELECTORS).contains(i))
                .ok_or(ParseError::Invalid)?;
            Role::Channel {
                scene_content_key: key,
                scene_serializer: index,
            }
        }
        "objective" => {
            fields(v, &["kind", "id"])?;
            Role::Objective {
                id: v["id"]
                    .as_u64()
                    .filter(|id| *id != 0)
                    .ok_or(ParseError::Invalid)?,
            }
        }
        "rpc" => {
            fields(v, &["kind"])?;
            Role::Rpc
        }
        "launcher" => {
            fields(v, &["kind"])?;
            Role::Launcher
        }
        "bool" => {
            fields(v, &["kind"])?;
            Role::Bool
        }
        "int" => {
            fields(v, &["kind"])?;
            Role::Int
        }
        _ => return Err(ParseError::Invalid),
    })
}

impl Construction {
    pub fn from_json(v: &Json) -> Result<Self, ParseError> {
        fields(v, &["content", "branches"])?;
        let content = content(&v["content"])?;
        let raw = v["branches"]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or(ParseError::Invalid)?;
        let mut branches = BTreeMap::new();
        let mut objective_ids = BTreeSet::new();
        for b in raw {
            fields(b, &["name", "asset", "bus_count", "roles"])?;
            let branch = match b["name"].as_str() {
                Some("main") => Branch::Main,
                Some("intro") => Branch::Intro,
                _ => return Err(ParseError::Invalid),
            };
            let asset = asset(&b["asset"])?;
            let profile = content.profile(asset).map_err(|_| ParseError::Invalid)?;
            let roles = b["roles"]
                .as_array()
                .filter(|a| a.len() == profile.kinds().len())
                .ok_or(ParseError::Invalid)?
                .iter()
                .map(role)
                .collect::<Result<Vec<_>, _>>()?;
            let mut rpcs = 0;
            for (role, kind) in roles.iter().zip(profile.kinds()) {
                if role.kind() != *kind {
                    return Err(ParseError::Invalid);
                }
                if !matches!(role, Role::Bool | Role::Int) {
                    rpcs += 1;
                }
                if let Role::Objective { id } = role
                    && !objective_ids.insert(*id)
                {
                    return Err(ParseError::Invalid);
                }
            }
            if rpcs > MAX_SELECTORS {
                return Err(ParseError::Invalid);
            }
            let blueprint = Blueprint {
                asset,
                bus_count: word(&b["bus_count"])?,
                roles,
            };
            if branches.insert(branch, blueprint).is_some() {
                return Err(ParseError::Invalid);
            }
        }
        Ok(Self { content, branches })
    }
    pub fn content(&self) -> &Content {
        &self.content
    }
    pub fn blueprint(&self, branch: Branch) -> Option<&Blueprint> {
        self.branches.get(&branch)
    }
    pub fn check_objectives(
        &self,
        branch: Branch,
        expected: &BTreeSet<u64>,
    ) -> Result<(), ParseError> {
        let blueprint = self.branches.get(&branch).ok_or(ParseError::Invalid)?;
        if blueprint.objective_ids() != *expected {
            return Err(ParseError::Invalid);
        }
        Ok(())
    }
}
