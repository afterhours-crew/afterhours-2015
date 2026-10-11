// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Immutable deployment catalogs. Configuration names domain fields and never
//! contains request/reply trees. Each reply is encoded for the current request.
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::{autolog, speedlist, stats};
use serde_json::Value as Json;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

pub const MAX_CONTENT_BYTES: usize = 128 * 1024;
pub const MAX_SCOPES: usize = 64;
pub const MAX_PAIRS: usize = 1024;
pub const MAX_PAIRS_PER_SCOPE: usize = 256;
pub const MAX_TYPES: usize = 32;
pub const MAX_SWITCHES: usize = 64;
pub const MAX_STRING_BYTES: usize = 127;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Ineligible,
    Encode,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "control catalog {self:?}")
    }
}
impl std::error::Error for Error {}

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 32 * 1024,
        max_depth: 4,
        max_values: 8192,
        max_collection: MAX_PAIRS_PER_SCOPE,
        max_byte_string: MAX_STRING_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(32 * 1024 + 16, 0, 32 * 1024).expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    matches!(
        (component, command),
        (stats::COMPONENT, stats::GET_KEY_SCOPES_MAP)
            | (
                autolog::COMPONENT,
                autolog::GET_KILL_SWITCHES | autolog::GET_TIME_LIMITED_FEATURES
            )
            | (speedlist::COMPONENT, speedlist::GET_SPEED_LIST_TYPE)
    )
}

struct Scope {
    name: String,
    aggregate_key: i64,
    aggregate: bool,
    values: Vec<(i64, i64)>,
}
struct SpeedType {
    id: u32,
    description: String,
    name: String,
    texture: String,
}
/// Contains definitions only: no identities, correlations, account history or
/// encoded frames. Clone/share this immutable catalog across authenticated users.
pub struct Catalog {
    scopes: Vec<Scope>,
    switches: Vec<String>,
    types: Vec<SpeedType>,
}
impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlCatalogs")
            .field("scopes", &self.scopes.len())
            .field("switches", &self.switches.len())
            .field("types", &self.types.len())
            .finish()
    }
}
fn fields(v: &Json, names: &[&str]) -> Result<(), ContentError> {
    let o = v.as_object().ok_or(ContentError::Invalid)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(())
}
fn text(v: &Json) -> Result<String, ContentError> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= MAX_STRING_BYTES && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(ContentError::Invalid)
}
fn rows(v: &Json, max: usize) -> Result<&[Json], ContentError> {
    v.as_array()
        .filter(|v| v.len() <= max)
        .map(Vec::as_slice)
        .ok_or(ContentError::Invalid)
}
fn integer(v: &Json) -> Result<i64, ContentError> {
    v.as_i64().ok_or(ContentError::Invalid)
}

impl Catalog {
    /// File I/O belongs on a blocking worker at an async edge.
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
    pub fn from_json(v: &Json) -> Result<Self, ContentError> {
        fields(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "key_scopes",
                "kill_switches",
                "speed_list_types",
                "time_limited_features",
            ],
        )?;
        if v["format"] != "nfs-control-catalogs"
            || v["version"] != 1
            || v["build_sha256"] != SUPPORTED_BUILD_SHA256
            || v["time_limited_features"] != "disabled"
        {
            return Err(ContentError::Invalid);
        }
        let mut scopes = Vec::new();
        let mut names = BTreeSet::new();
        let mut total = 0;
        for row in rows(&v["key_scopes"], MAX_SCOPES)? {
            fields(row, &["name", "aggregate_key", "aggregate", "values"])?;
            let name = text(&row["name"])?;
            if !names.insert(name.clone()) {
                return Err(ContentError::Invalid);
            }
            let mut values = Vec::new();
            let mut keys = BTreeSet::new();
            for pair in rows(&row["values"], MAX_PAIRS_PER_SCOPE)? {
                let pair = rows(pair, 2)?;
                if pair.len() != 2 {
                    return Err(ContentError::Invalid);
                }
                let key = integer(&pair[0])?;
                if !keys.insert(key) {
                    return Err(ContentError::Invalid);
                }
                values.push((key, integer(&pair[1])?));
            }
            total += values.len();
            if total > MAX_PAIRS {
                return Err(ContentError::TooLarge);
            }
            scopes.push(Scope {
                name,
                aggregate_key: integer(&row["aggregate_key"])?,
                aggregate: row["aggregate"].as_bool().ok_or(ContentError::Invalid)?,
                values,
            });
        }
        let mut switches = Vec::new();
        let mut names = BTreeSet::new();
        for row in rows(&v["kill_switches"], MAX_SWITCHES)? {
            let name = text(row)?;
            if !names.insert(name.clone()) {
                return Err(ContentError::Invalid);
            }
            switches.push(name);
        }
        let mut types = Vec::new();
        let mut ids = BTreeSet::new();
        for row in rows(&v["speed_list_types"], MAX_TYPES)? {
            fields(row, &["id", "description", "name", "texture"])?;
            let id = u32::try_from(row["id"].as_u64().ok_or(ContentError::Invalid)?)
                .map_err(|_| ContentError::Invalid)?;
            if !ids.insert(id) {
                return Err(ContentError::Invalid);
            }
            types.push(SpeedType {
                id,
                description: text(&row["description"])?,
                name: text(&row["name"])?,
                texture: text(&row["texture"])?,
            });
        }
        let catalog = Self {
            scopes,
            switches,
            types,
        };
        // Reject configurations that fit individual limits but exceed encoding
        // budgets. The largest positive persona bounds any runtime identity.
        catalog.scope_body().map_err(|_| ContentError::TooLarge)?;
        catalog.switch_body().map_err(|_| ContentError::TooLarge)?;
        catalog
            .type_body(i64::MAX)
            .map_err(|_| ContentError::TooLarge)?;
        Ok(catalog)
    }
    fn scope_body(&self) -> Result<Vec<u8>, Error> {
        stats::KeyScopes {
            key_scopes_map: Some(stats::KeyScopesMap(
                self.scopes
                    .iter()
                    .map(|s| {
                        (
                            s.name.as_bytes(),
                            stats::KeyScopeItem {
                                aggregate_key_value: Some(s.aggregate_key),
                                enable_aggregation: Some(s.aggregate),
                                key_scope_values: Some(stats::KeyScopeValues(s.values.clone())),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            )),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)
    }
    fn switch_body(&self) -> Result<Vec<u8>, Error> {
        autolog::KillSwitchResponse {
            kill_switch_list: Some(autolog::StringList(
                self.switches.iter().map(|s| s.as_bytes()).collect(),
            )),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)
    }
    fn type_body(&self, persona: i64) -> Result<Vec<u8>, Error> {
        speedlist::SpeedListTypeResponse {
            blaze_id: Some(persona),
            speed_list_types: Some(speedlist::SpeedListTypes(
                self.types
                    .iter()
                    .map(|t| {
                        (
                            t.id,
                            speedlist::SpeedListTypeInstance {
                                speed_list_type_id: Some(t.id),
                                type_description_string_id: Some(t.description.as_bytes()),
                                type_localised_name_string_id: Some(t.name.as_bytes()),
                                type_texture_id: Some(t.texture.as_bytes()),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            )),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)
    }
    /// The caller supplies the authenticated persona. Unsupported forms get no
    /// answer and must not fall through to another handler on these four routes.
    pub fn reply(&self, wire: &[u8], persona: i64) -> Result<Vec<u8>, Error> {
        let d = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = d.frame;
        if persona <= 0
            || d.consumed != wire.len()
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
        {
            return Err(Error::Ineligible);
        }
        let body = match (f.fields.routing_a, f.fields.routing_b) {
            (stats::COMPONENT, stats::GET_KEY_SCOPES_MAP) => {
                stats::GetKeyScopesMapRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                self.scope_body()?
            }
            (autolog::COMPONENT, autolog::GET_TIME_LIMITED_FEATURES) => {
                let q = autolog::GetTimeLimitedFeaturesRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.deda != Some(b"")
                    || q.unknown_field_count() != 0
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                Vec::new()
            }
            (autolog::COMPONENT, autolog::GET_KILL_SWITCHES) => {
                let q = autolog::KillSwitchRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.blaze_id != Some(0)
                    || q.unknown_field_count() != 0
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                self.switch_body()?
            }
            (speedlist::COMPONENT, speedlist::GET_SPEED_LIST_TYPE) => {
                let q = speedlist::SpeedListTypeRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.blaze_id != Some(persona)
                    || q.list_of_speed_list_types_requested.is_some()
                    || q.unknown_field_count() != 0
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                self.type_body(persona)?
            }
            _ => return Err(Error::Ineligible),
        };
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    category: 1,
                    ..f.fields
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
}
