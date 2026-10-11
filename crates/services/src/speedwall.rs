// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Speed-wall brief information (`2050/79`, `getSpeedWallBriefInfo`) for one
//! durable local account: the account's own row on each configured wall and
//! stat type, with the rank, rating and total count of that wall.
//!
//! Rows are named values of a versioned state document. The reply names the
//! current persona and display name of the connection, never a stored
//! identity. Walls or stat types the document does not hold, and queries for
//! other personas, are unsupported (no reply). Score accrual and wall
//! membership changes are outside this read-only subset.
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::autolog::{
    BlazeUser, COMPONENT, GET_SPEED_WALL_BRIEF_INFO, InGameSpeedWallResponseRow, RatingBits,
    RowIntegers, RowStrings, SettingsFloatBits, SpeedWallBriefInfoRequest,
    SpeedWallBriefInfoResponse,
};
use nfs_storage::AccountId;
use serde_json::Value as Json;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

pub const MAX_STATE_BYTES: usize = 64 * 1024;
pub const MAX_ROWS: usize = 16;
pub const MAX_ENTRIES: usize = 16;
pub const MAX_KEY_BYTES: usize = 64;
pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_NAME_BYTES: usize = 32;
pub const MAX_BODY_BYTES: usize = 4096;
/// The observed relation of the account's own row.
pub const SELF_RELATION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a complete, metadata-free category-0 `2050/79` request.
    Ineligible,
    /// Another account, persona or display name than this connection's.
    Identity,
    /// The document holds no row for this wall and stat type.
    Unsupported,
    Encode,
}

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_BODY_BYTES,
        max_depth: 4,
        max_values: 128,
        max_collection: MAX_ENTRIES,
        max_byte_string: MAX_VALUE_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_BODY_BYTES + nfs_fire2::HEADER_LEN, 0, MAX_BODY_BYTES)
        .expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    (component, command) == (COMPONENT, GET_SPEED_WALL_BRIEF_INFO)
}

/// The account's row on one wall and stat type.
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    pub speed_wall_id: u32,
    pub stat_type: i32,
    pub rank: i32,
    pub rating_bits: u32,
    pub total_count: i32,
    pub integers: Vec<(String, i32)>,
    pub floats: Vec<(String, u32)>,
    /// The string stats map; `None` omits it (the observed own row has none).
    pub strings: Option<Vec<(String, String)>>,
}
impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeedWallRow")
            .field("speed_wall_id", &self.speed_wall_id)
            .field("stat_type", &self.stat_type)
            .finish_non_exhaustive()
    }
}

pub struct State {
    account: AccountId,
    rows: Vec<Row>,
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeedWallState")
            .field("rows", &self.rows.len())
            .finish()
    }
}

fn fields(v: &Json, names: &[&str]) -> Result<(), ContentError> {
    let object = v.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(())
}
fn text(v: &Json, max: usize) -> Result<String, ContentError> {
    v.as_str()
        .filter(|s| s.len() <= max && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(ContentError::Invalid)
}
fn keyed<T>(
    v: &Json,
    value: impl Fn(&Json) -> Result<T, ContentError>,
) -> Result<Vec<(String, T)>, ContentError> {
    let pairs = v
        .as_array()
        .filter(|a| a.len() <= MAX_ENTRIES)
        .ok_or(ContentError::Invalid)?;
    let mut keys = BTreeSet::new();
    pairs
        .iter()
        .map(|pair| {
            let pair = pair
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or(ContentError::Invalid)?;
            let key = text(&pair[0], MAX_KEY_BYTES)?;
            if key.is_empty() || !keys.insert(key.clone()) {
                return Err(ContentError::Invalid);
            }
            Ok((key, value(&pair[1])?))
        })
        .collect()
}
fn signed(v: &Json) -> Result<i32, ContentError> {
    v.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or(ContentError::Invalid)
}
fn bits(v: &Json) -> Result<u32, ContentError> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(ContentError::Invalid)
}

impl State {
    pub fn new(account: AccountId, rows: Vec<Row>) -> Result<Self, ContentError> {
        if rows.len() > MAX_ROWS {
            return Err(ContentError::TooLarge);
        }
        let mut walls = BTreeSet::new();
        for row in &rows {
            if !matches!(row.stat_type, 0 | 1)
                || !walls.insert((row.speed_wall_id, row.stat_type))
                || row.integers.len() > MAX_ENTRIES
                || row.floats.len() > MAX_ENTRIES
                || row.strings.as_ref().is_some_and(|s| s.len() > MAX_ENTRIES)
            {
                return Err(ContentError::Invalid);
            }
            for (key, _) in &row.integers {
                if key.is_empty() || key.len() > MAX_KEY_BYTES || key.contains('\0') {
                    return Err(ContentError::Invalid);
                }
            }
            for (key, _) in &row.floats {
                if key.is_empty() || key.len() > MAX_KEY_BYTES || key.contains('\0') {
                    return Err(ContentError::Invalid);
                }
            }
            for (key, value) in row.strings.iter().flatten() {
                if key.is_empty()
                    || key.len() > MAX_KEY_BYTES
                    || key.contains('\0')
                    || value.len() > MAX_VALUE_BYTES
                    || value.contains('\0')
                {
                    return Err(ContentError::Invalid);
                }
            }
        }
        Ok(Self { account, rows })
    }
    /// File I/O must run on a blocking worker at an async edge.
    pub fn load(path: &Path, account: AccountId) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take((MAX_STATE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(
            &serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?,
            account,
        )
    }
    pub fn from_json(v: &Json, account: AccountId) -> Result<Self, ContentError> {
        fields(v, &["format", "version", "build_sha256", "account", "rows"])?;
        if v["format"] != "nfs-speedwall-state"
            || v["version"] != 1
            || v["build_sha256"] != SUPPORTED_BUILD_SHA256
            || v["account"].as_str() != Some(account.hex().as_str())
        {
            return Err(ContentError::Invalid);
        }
        let rows = v["rows"]
            .as_array()
            .filter(|a| a.len() <= MAX_ROWS)
            .ok_or(ContentError::Invalid)?
            .iter()
            .map(|r| {
                fields(
                    r,
                    &[
                        "speed_wall_id",
                        "stat_type",
                        "rank",
                        "rating_bits",
                        "total_count",
                        "integers",
                        "floats",
                        "strings",
                    ],
                )?;
                Ok(Row {
                    speed_wall_id: bits(&r["speed_wall_id"])?,
                    stat_type: signed(&r["stat_type"])?,
                    rank: signed(&r["rank"])?,
                    rating_bits: bits(&r["rating_bits"])?,
                    total_count: signed(&r["total_count"])?,
                    integers: keyed(&r["integers"], signed)?,
                    floats: keyed(&r["floats"], bits)?,
                    strings: match &r["strings"] {
                        Json::Null => None,
                        v => Some(keyed(v, |v| text(v, MAX_VALUE_BYTES))?),
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(account, rows)
    }
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Answer one brief-info request for the authenticated connection. `name`
    /// is the connection's current display name. `Identity` and `Unsupported`
    /// are "not mine": the caller must not fall back to a template.
    pub fn reply(
        &self,
        wire: &[u8],
        account: AccountId,
        persona: i64,
        name: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let d = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = d.frame;
        if d.consumed != wire.len()
            || !owns(f.fields.routing_a, f.fields.routing_b)
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
        {
            return Err(Error::Ineligible);
        }
        let limits = body_limits();
        let q = SpeedWallBriefInfoRequest::decode(f.body, limits).map_err(|_| Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
            || q.speed_wall_id.is_none()
            || q.speed_wall_stat_type.is_none()
        {
            return Err(Error::Ineligible);
        }
        if account != self.account
            || persona <= 0
            || q.blaze_id != Some(persona)
            || name.is_empty()
            || name.len() > MAX_NAME_BYTES
            || name.contains(&0)
        {
            return Err(Error::Identity);
        }
        let row = self
            .rows
            .iter()
            .find(|r| {
                Some(r.speed_wall_id) == q.speed_wall_id
                    && Some(r.stat_type) == q.speed_wall_stat_type
            })
            .ok_or(Error::Unsupported)?;
        let body = SpeedWallBriefInfoResponse {
            rank: Some(row.rank),
            rating: Some(RatingBits(row.rating_bits)),
            speed_wall: Some(InGameSpeedWallResponseRow {
                blaze_user: Some(BlazeUser {
                    blaze_id: Some(persona),
                    persona_name: Some(name),
                    relation_type: Some(SELF_RELATION),
                    ..Default::default()
                }),
                stats_flt: Some(SettingsFloatBits(
                    row.floats.iter().map(|(k, v)| (k.as_bytes(), *v)).collect(),
                )),
                stats_int: Some(RowIntegers(
                    row.integers
                        .iter()
                        .map(|(k, v)| (k.as_bytes(), *v))
                        .collect(),
                )),
                stats_str: row.strings.as_ref().map(|strings| {
                    RowStrings(
                        strings
                            .iter()
                            .map(|(k, v)| (k.as_bytes(), v.as_bytes()))
                            .collect(),
                    )
                }),
                ..Default::default()
            }),
            speed_wall_id: Some(row.speed_wall_id),
            speed_wall_stat_type: Some(row.stat_type),
            total_count: Some(row.total_count),
            ..Default::default()
        }
        .encode(limits)
        .map_err(|_| Error::Encode)?;
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
