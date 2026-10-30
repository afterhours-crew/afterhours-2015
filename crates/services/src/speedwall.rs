// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Speed-wall brief information (`2050/79`, `getSpeedWallBriefInfo`) for one
//! durable local account: the account's own row on each configured wall and
//! stat type, with the rank, rating and total count of that wall; and the
//! in-game speed walls (`2050/20`, `getInGameSpeedWalls`): for both stat types,
//! each requested wall with the account's own row when it has one (E997: the
//! official reply lists a wall without rows when there is none).
//!
//! Rows are named values of a versioned state document. The reply names the
//! current persona and display name of the connection, never a stored
//! identity. A supported query without an owned row answers `AUTOLOG_ERR_NOT_FOUND`.
//! Unknown stat types and queries for other personas remain unsupported.
//! Completed sprint records overlay the optional imported baseline from the
//! current account snapshot, so queries never retain a startup-only result view.
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::autolog::{
    BlazeUser, COMPONENT, GET_IN_GAME_SPEED_WALLS, GET_SPEED_WALL_BRIEF_INFO,
    InGameSpeedWallResponse, InGameSpeedWallResponseRow, InGameSpeedWallResponseSpeedWall,
    InGameSpeedWallsRequest, MAX_SPEED_WALLS, RatingBits, RowIntegers, RowStrings,
    SettingsFloatBits, SpeedWallBriefInfoRequest, SpeedWallBriefInfoResponse, SpeedWallRows,
    SpeedWallsByStat,
};
use nfs_storage::AccountId;
use serde_json::Value as Json;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

pub const MAX_STATE_BYTES: usize = 64 * 1024;
pub const MAX_ROWS: usize = 256;
pub const MAX_ENTRIES: usize = 16;
pub const MAX_KEY_BYTES: usize = 64;
pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_NAME_BYTES: usize = 32;
pub mod records;
pub use records::{Current, Sprint};
pub const MAX_BODY_BYTES: usize = 4096;
/// The observed relation of the account's own row.
pub const SELF_RELATION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a complete, metadata-free category-0 `2050/79` request.
    Ineligible,
    /// Another account, persona or display name than this connection's.
    Identity,
    /// The query selects an unsupported stat type.
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
/// In-game speed-wall replies nest rows inside walls inside the stat-type map.
pub const MAX_WALLS_BODY_BYTES: usize = 16 * 1024;
pub fn walls_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_WALLS_BODY_BYTES,
        max_depth: 8,
        max_values: 1024,
        max_collection: MAX_SPEED_WALLS.max(MAX_ENTRIES),
        max_byte_string: MAX_VALUE_BYTES + 1,
    }
}
pub fn walls_frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(
        MAX_WALLS_BODY_BYTES + nfs_fire2::HEADER_LEN,
        0,
        MAX_WALLS_BODY_BYTES,
    )
    .expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    (component, command) == (COMPONENT, GET_SPEED_WALL_BRIEF_INFO)
}
pub fn owns_walls(component: u16, command: u16) -> bool {
    (component, command) == (COMPONENT, GET_IN_GAME_SPEED_WALLS)
}
/// The stat types every in-game reply lists, in the official order.
const STAT_TYPES: [i32; 2] = [0, 1];

/// The account's own row as the client reads it: the connection's persona and
/// display name with the stored stat maps.
fn own_row<'a>(row: &'a Row, persona: i64, name: &'a [u8]) -> InGameSpeedWallResponseRow<'a> {
    InGameSpeedWallResponseRow {
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
    }
}
fn request_frame(wire: &[u8], command: u16) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || (f.fields.routing_a, f.fields.routing_b) != (COMPONENT, command)
        || f.fields.category != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Error::Ineligible);
    }
    Ok(f)
}
fn reply_frame(f: &Frame<'_>, body: &[u8], limits: nfs_fire2::Limits) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                category: 1,
                ..f.fields
            },
            metadata: &[],
            body,
        },
        limits,
    )
    .map_err(|_| Error::Encode)
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

#[derive(Clone)]
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
    /// Overlay current owned records while retaining unrelated imported walls.
    pub fn with_current(&self, current: &Current) -> Result<Self, ContentError> {
        if self.account != current.account {
            return Err(ContentError::Invalid);
        }
        let mut rows = self.rows.clone();
        for row in &current.rows {
            if let Some(old) = rows
                .iter_mut()
                .find(|r| r.speed_wall_id == row.speed_wall_id && r.stat_type == row.stat_type)
            {
                *old = records::combine(old, row)?;
            } else {
                rows.push(row.clone());
            }
        }
        Self::new(self.account, rows)
    }

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
        let f = request_frame(wire, GET_SPEED_WALL_BRIEF_INFO)?;
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
        if !matches!(q.speed_wall_stat_type, Some(0 | 1)) {
            return Err(Error::Unsupported);
        }
        let Some(row) = self.rows.iter().find(|r| {
            Some(r.speed_wall_id) == q.speed_wall_id && Some(r.stat_type) == q.speed_wall_stat_type
        }) else {
            // Completing the read is necessary even for an empty local account:
            // the client serializes other content requests behind this job.
            return crate::error_reply(f.fields, nfs_protocol::metadata::AUTOLOG_ERR_NOT_FOUND)
                .ok_or(Error::Encode);
        };
        let body = SpeedWallBriefInfoResponse {
            rank: Some(row.rank),
            rating: Some(RatingBits(row.rating_bits)),
            speed_wall: Some(own_row(row, persona, name)),
            speed_wall_id: Some(row.speed_wall_id),
            speed_wall_stat_type: Some(row.stat_type),
            total_count: Some(row.total_count),
            ..Default::default()
        }
        .encode(limits)
        .map_err(|_| Error::Encode)?;
        reply_frame(&f, &body, frame_limits())
    }

    /// Answer one in-game speed-wall request for the authenticated connection:
    /// both stat types, each listing the requested walls in request order with
    /// the account's own row where it has one. Requests naming another persona,
    /// no or too many walls, or nonzero options are "not mine".
    pub fn reply_walls(
        &self,
        wire: &[u8],
        account: AccountId,
        persona: i64,
        name: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let f = request_frame(wire, GET_IN_GAME_SPEED_WALLS)?;
        let limits = walls_limits();
        let q = InGameSpeedWallsRequest::decode(f.body, limits).map_err(|_| Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
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
        let ids = q.speed_wall_ids.as_ref().map(|l| l.0.as_slice());
        // The observed requests set both options to zero; other values are not
        // interpreted.
        let Some(ids) = ids.filter(|ids| !ids.is_empty() && ids.len() <= MAX_SPEED_WALLS) else {
            return Err(Error::Unsupported);
        };
        if q.usge != Some(0) || q.uspg != Some(0) {
            return Err(Error::Unsupported);
        }
        let walls = STAT_TYPES
            .iter()
            .map(|&stat| {
                let walls = ids
                    .iter()
                    .map(|&id| InGameSpeedWallResponseSpeedWall {
                        rows: self
                            .rows
                            .iter()
                            .find(|r| r.speed_wall_id == id && r.stat_type == stat)
                            .map(|row| SpeedWallRows(vec![own_row(row, persona, name)])),
                        speed_wall_id: Some(id),
                        ..Default::default()
                    })
                    .collect();
                (stat as u32, walls)
            })
            .collect();
        let body = InGameSpeedWallResponse {
            speed_walls: Some(SpeedWallsByStat(walls)),
            ..Default::default()
        }
        .encode(limits)
        .map_err(|_| Error::Encode)?;
        reply_frame(&f, &body, walls_frame_limits())
    }
}
