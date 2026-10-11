// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static Stats definitions and current account-table values. No recorded
//! exchanges or imported player values are runtime inputs. The caller supplies
//! the authenticated persona and a committed snapshot for each query.
use crate::{
    ContentError, SUPPORTED_BUILD_SHA256,
    persistent::{Loaded, key},
    reputation,
};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::stats::*;
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

pub const MAX_CONTENT_BYTES: usize = 64 * 1024;
pub const MAX_GROUPS: usize = 16;
pub const MAX_STATS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Ineligible,
    State,
    Encode,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stats {self:?}")
    }
}
impl std::error::Error for Error {}

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 16 * 1024,
        max_depth: 4,
        max_values: 2048,
        max_collection: MAX_STATS,
        max_byte_string: 1024,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(16 * 1024 + 16, 0, 16 * 1024).expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    component == COMPONENT && matches!(command, GET_STAT_GROUP | GET_STATS_BY_GROUP_ASYNC)
}

struct Definition {
    body: Vec<u8>,
    table: u32,
    columns: Vec<u32>,
    entity_type: ObjectType,
}
pub struct Catalog {
    groups: BTreeMap<Vec<u8>, Definition>,
}
impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatDefinitions")
            .field("groups", &self.groups.len())
            .finish()
    }
}

fn fields<'a>(v: &'a Json, names: &[&str]) -> Result<&'a Json, ContentError> {
    let o = v.as_object().ok_or(ContentError::Invalid)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(v)
}
fn string(v: &Json) -> Result<&[u8], ContentError> {
    v.as_str()
        .filter(|s| s.len() <= 1024 && !s.contains('\0'))
        .map(str::as_bytes)
        .ok_or(ContentError::Invalid)
}
fn identifier(v: &Json) -> Result<&str, ContentError> {
    v.as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 127
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or(ContentError::Invalid)
}
impl Catalog {
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take((MAX_CONTENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > MAX_CONTENT_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    /// A versioned catalog contains definitions only. A group's table and stat
    /// names select committed integer stores, never defaults or query values.
    pub fn from_json(value: &Json) -> Result<Self, ContentError> {
        fields(value, &["format", "version", "build_sha256", "groups"])?;
        if value["format"] != "nfs-stat-definitions"
            || value["version"] != 1
            || value["build_sha256"] != SUPPORTED_BUILD_SHA256
        {
            return Err(ContentError::Invalid);
        }
        let rows = value["groups"].as_array().ok_or(ContentError::Invalid)?;
        if rows.is_empty() || rows.len() > MAX_GROUPS {
            return Err(ContentError::Invalid);
        }
        let mut groups = BTreeMap::new();
        let mut total = 0usize;
        for row in rows {
            fields(
                row,
                &[
                    "name",
                    "table",
                    "category",
                    "description",
                    "metadata",
                    "entity_type",
                    "stats",
                ],
            )?;
            let name = identifier(&row["name"])?;
            let table = identifier(&row["table"])?;
            if name.strip_prefix("Persistence_") != Some(table)
                || row["entity_type"] != serde_json::json!([30722, 1])
            {
                return Err(ContentError::Invalid);
            }
            let stats = row["stats"].as_array().ok_or(ContentError::Invalid)?;
            if stats.is_empty() || stats.len() > MAX_STATS {
                return Err(ContentError::Invalid);
            }
            let mut columns = Vec::new();
            let mut seen = BTreeSet::new();
            let mut descs = Vec::new();
            for stat in stats {
                fields(
                    stat,
                    &[
                        "category",
                        "default",
                        "derived",
                        "format",
                        "kind",
                        "long_desc",
                        "metadata",
                        "name",
                        "short_desc",
                        "stat_type",
                    ],
                )?;
                let column = identifier(&stat["name"])?;
                if !seen.insert(key(column)) || stat["derived"] != false || stat["stat_type"] != 0 {
                    return Err(ContentError::Invalid);
                }
                columns.push(key(column));
                descs.push(StatDescSummary {
                    category: Some(string(&stat["category"])?),
                    default_value: Some(string(&stat["default"])?),
                    derived: Some(false),
                    format: Some(string(&stat["format"])?),
                    kind: Some(string(&stat["kind"])?),
                    long_desc: Some(string(&stat["long_desc"])?),
                    metadata: Some(string(&stat["metadata"])?),
                    name: Some(column.as_bytes()),
                    short_desc: Some(string(&stat["short_desc"])?),
                    stat_type: Some(0),
                    ..Default::default()
                });
            }
            let entity_type = ObjectType(30722, 1);
            let body = StatGroupResponse {
                category_name: Some(string(&row["category"])?),
                desc: Some(string(&row["description"])?),
                entity_type: Some(entity_type),
                metadata: Some(string(&row["metadata"])?),
                name: Some(name.as_bytes()),
                stat_descs: Some(StatDescSummaryList(descs)),
                ..Default::default()
            }
            .encode(body_limits())
            .map_err(|_| ContentError::Invalid)?;
            total = total
                .checked_add(body.len())
                .ok_or(ContentError::TooLarge)?;
            if total > MAX_CONTENT_BYTES {
                return Err(ContentError::TooLarge);
            }
            if groups
                .insert(
                    name.as_bytes().to_vec(),
                    Definition {
                        body,
                        table: key(table),
                        columns,
                        entity_type,
                    },
                )
                .is_some()
            {
                return Err(ContentError::Invalid);
            }
        }
        Ok(Self { groups })
    }
    pub fn len(&self) -> usize {
        self.groups.len()
    }
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Unknown groups and unsupported query shapes never fall back to a
    /// template. The caller owns these routes whenever this catalog is enabled.
    pub fn reply(
        &self,
        wire: &[u8],
        persona: i64,
        current: &Current,
    ) -> Result<Option<Vec<Vec<u8>>>, Error> {
        let d = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = d.frame;
        if d.consumed != wire.len()
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
            || !owns(f.fields.routing_a, f.fields.routing_b)
            || persona <= 0
        {
            return Err(Error::Ineligible);
        }
        if f.fields.routing_b == GET_STAT_GROUP {
            let q = GetStatGroupRequest::decode(f.body, body_limits())
                .map_err(|_| Error::Ineligible)?;
            if q.unknown_field_count() != 0
                || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
            {
                return Err(Error::Ineligible);
            }
            let name = q.name.ok_or(Error::Ineligible)?;
            return self
                .groups
                .get(name)
                .map(|group| {
                    encode(
                        Fields {
                            category: 1,
                            ..f.fields
                        },
                        &group.body,
                    )
                    .map(|r| vec![r])
                })
                .transpose();
        }
        let q =
            GetStatsByGroupRequest::decode(f.body, body_limits()).map_err(|_| Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
            || q.entity_ids.as_ref().is_none_or(|ids| ids.0 != [persona])
            || q.key_scope_name_value_map.is_some()
            || q.period_ctr != Some(1)
            || q.period_offset != Some(0)
            || q.period_id != Some(0)
            || q.period_type != Some(0)
            || q.time != Some(0)
            || q.view_id.is_none_or(|v| v == 0)
        {
            return Err(Error::Ineligible);
        }
        let name = q.group_name.ok_or(Error::Ineligible)?;
        let Some(group) = self.groups.get(name) else {
            return Ok(None);
        };
        let values = group
            .columns
            .iter()
            .map(|column| {
                current
                    .stores
                    .get(&(group.table, *column))
                    .map(i32::to_string)
                    .ok_or(Error::State)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = KeyScopedStatValues {
            group_name: Some(name),
            key_string: Some(b"No_Scope_Defined"),
            last: Some(true),
            view_id: q.view_id,
            stat_values: Some(StatValues {
                entity_stats: Some(EntityStatsList(vec![EntityStats {
                    entity_id: Some(persona),
                    entity_type: Some(group.entity_type),
                    period_offset: Some(0),
                    stat_values: Some(StatStrings(values.iter().map(|v| v.as_bytes()).collect())),
                    ..Default::default()
                }])),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        Ok(Some(vec![
            encode(
                Fields {
                    category: 1,
                    ..f.fields
                },
                &[],
            )?,
            encode(
                Fields {
                    routing_a: COMPONENT,
                    routing_b: GET_STATS_ASYNC_NOTIFICATION,
                    category: 2,
                    ..Default::default()
                },
                &body,
            )?,
        ]))
    }
}

fn encode(fields: Fields, body: &[u8]) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields,
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}

/// One committed account view. RepLevel is projected from its score using the
/// same threshold model as the world, never copied from a stale stored level.
pub struct Current {
    stores: BTreeMap<(u32, u32), i32>,
    pub generation: u64,
}
impl Current {
    pub fn from_loaded(
        loaded: &Loaded,
        thresholds: &reputation::Thresholds,
    ) -> Result<Self, Error> {
        let scores = reputation::Scores::load(loaded).map_err(|_| Error::State)?;
        let mut stores = loaded.stores().clone();
        stores.insert(
            (key("RepValuesTable"), key("RepLevel")),
            thresholds.evaluate(scores).get(reputation::Field::Level),
        );
        Ok(Self {
            stores,
            generation: loaded.generation(),
        })
    }
}

#[cfg(test)]
mod tests;
