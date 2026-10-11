// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Kickback (component 2053) menu state bound to an explicit durable local
//! account: the screenshot counters (`2053/2`), the last-week winner tile
//! (`2053/17`) and the snapshot gallery tiles (`2053/22`).
//!
//! The counters and the winner tile are named values of a versioned state
//! document; the gallery is an explicit empty local directory. A document
//! without a winner tile answers `2053/17` with `KICKBACK_ERR_NOT_FOUND`
//! rather than inventing a tile. Screenshot uploads, kickback voting and
//! gallery contents are outside this read-only subset.
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::kickback::{
    COMPONENT, GET_LAST_WEEKS_WINNER_DATA, GET_SCREENSHOT_COUNTERS, GET_SNAPSHOT_GALLERY_TILES,
    GetLastWeekWinnerDataRequest, GetScreenshotCounterRequest, GetScreenshotCounterResponse,
    GetSnapshotGalleryTilesRequest, GetSnapshotGalleryTilesResponse, ScreenshotGalleryDataEx,
    SnapshotGalleryTiles,
};
use nfs_storage::AccountId;
use serde_json::Value as Json;
use std::{fs::File, io::Read, path::Path};

pub const MAX_STATE_BYTES: usize = 64 * 1024;
pub const MAX_TITLE_BYTES: usize = 256;
pub const MAX_RECORD_NAME_BYTES: usize = 40;
pub const MAX_BODY_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a complete, metadata-free category-0 request on an owned route.
    Ineligible,
    /// The request belongs to another account or an unsupported query.
    Identity,
    Encode,
}

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_BODY_BYTES,
        max_depth: 2,
        max_values: 32,
        max_collection: 16,
        max_byte_string: MAX_TITLE_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_BODY_BYTES + nfs_fire2::HEADER_LEN, 0, MAX_BODY_BYTES)
        .expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    component == COMPONENT
        && matches!(
            command,
            GET_SCREENSHOT_COUNTERS | GET_LAST_WEEKS_WINNER_DATA | GET_SNAPSHOT_GALLERY_TILES
        )
}

fn frame(wire: &[u8]) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || f.fields.routing_a != COMPONENT
        || f.fields.category != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Error::Ineligible);
    }
    Ok(f)
}
fn response(f: &Frame<'_>, body: &[u8]) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                category: 1,
                ..f.fields
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}

/// The last-week winner tile: a named local value, never a captured reply.
#[derive(Clone, PartialEq, Eq)]
pub struct Winner {
    pub datetime: i64,
    pub title: String,
    pub screenshot_id: u64,
    pub kickback_count: u32,
    pub persona_id: i64,
    pub player_provided_kickback: bool,
    pub record_name: String,
    pub screenshot_type: u16,
}
impl std::fmt::Debug for Winner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Winner").finish_non_exhaustive()
    }
}

/// One account's kickback menu state.
pub struct State {
    account: AccountId,
    screenshot_count: u32,
    screenshot_count_max: u32,
    winner: Option<Winner>,
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KickbackState")
            .field("winner", &self.winner.is_some())
            .finish_non_exhaustive()
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
fn word<T: TryFrom<u64>>(v: &Json) -> Result<T, ContentError> {
    v.as_u64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or(ContentError::Invalid)
}

impl State {
    pub fn new(
        account: AccountId,
        screenshot_count: u32,
        screenshot_count_max: u32,
        winner: Option<Winner>,
    ) -> Result<Self, ContentError> {
        if screenshot_count > screenshot_count_max {
            return Err(ContentError::Invalid);
        }
        if let Some(w) = &winner
            && (w.title.len() > MAX_TITLE_BYTES
                || w.title.contains('\0')
                || w.record_name.is_empty()
                || w.record_name.len() > MAX_RECORD_NAME_BYTES
                || w.record_name.contains('\0')
                || w.screenshot_id == 0
                || w.persona_id <= 0)
        {
            return Err(ContentError::Invalid);
        }
        Ok(Self {
            account,
            screenshot_count,
            screenshot_count_max,
            winner,
        })
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
        fields(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "account",
                "screenshot_count",
                "screenshot_count_max",
                "gallery",
                "winner",
            ],
        )?;
        if v["format"] != "nfs-kickback-state"
            || v["version"] != 1
            || v["build_sha256"] != SUPPORTED_BUILD_SHA256
            || v["account"].as_str() != Some(account.hex().as_str())
            || v["gallery"] != "empty"
        {
            return Err(ContentError::Invalid);
        }
        let winner = match &v["winner"] {
            Json::Null => None,
            w => {
                fields(
                    w,
                    &[
                        "datetime",
                        "title",
                        "screenshot_id",
                        "kickback_count",
                        "persona_id",
                        "player_provided_kickback",
                        "record_name",
                        "screenshot_type",
                    ],
                )?;
                Some(Winner {
                    datetime: w["datetime"].as_i64().ok_or(ContentError::Invalid)?,
                    title: text(&w["title"], MAX_TITLE_BYTES)?,
                    screenshot_id: word(&w["screenshot_id"])?,
                    kickback_count: word(&w["kickback_count"])?,
                    persona_id: w["persona_id"].as_i64().ok_or(ContentError::Invalid)?,
                    player_provided_kickback: w["player_provided_kickback"]
                        .as_bool()
                        .ok_or(ContentError::Invalid)?,
                    record_name: text(&w["record_name"], MAX_RECORD_NAME_BYTES)?,
                    screenshot_type: word(&w["screenshot_type"])?,
                })
            }
        };
        Self::new(
            account,
            word(&v["screenshot_count"])?,
            word(&v["screenshot_count_max"])?,
            winner,
        )
    }
    pub fn winner(&self) -> Option<&Winner> {
        self.winner.as_ref()
    }
    pub fn counters(&self) -> (u32, u32) {
        (self.screenshot_count, self.screenshot_count_max)
    }

    /// Answer one owned request for the authenticated account. `Identity` and
    /// `Ineligible` are "not mine": the caller must not fall back to a template.
    pub fn reply(&self, wire: &[u8], account: AccountId, persona: i64) -> Result<Vec<u8>, Error> {
        let f = frame(wire)?;
        if account != self.account || persona <= 0 {
            return Err(Error::Identity);
        }
        let limits = body_limits();
        let body = match f.fields.routing_b {
            GET_SCREENSHOT_COUNTERS => {
                GetScreenshotCounterRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                GetScreenshotCounterResponse {
                    screenshot_count_max: Some(self.screenshot_count_max),
                    screenshot_count: Some(self.screenshot_count),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?
            }
            GET_LAST_WEEKS_WINNER_DATA => {
                GetLastWeekWinnerDataRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                let Some(w) = self.winner.as_ref() else {
                    return crate::error_reply(
                        f.fields,
                        nfs_protocol::metadata::KICKBACK_ERR_NOT_FOUND,
                    )
                    .ok_or(Error::Encode);
                };
                ScreenshotGalleryDataEx {
                    datetime: Some(w.datetime),
                    title: Some(w.title.as_bytes()),
                    screenshot_id: Some(w.screenshot_id),
                    kickback_count: Some(w.kickback_count),
                    is_last_week_winner: Some(true),
                    persona_id: Some(w.persona_id),
                    player_provided_kickback: Some(w.player_provided_kickback),
                    record_name: Some(w.record_name.as_bytes()),
                    screenshot_type: Some(w.screenshot_type),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?
            }
            GET_SNAPSHOT_GALLERY_TILES => {
                GetSnapshotGalleryTilesRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                GetSnapshotGalleryTilesResponse {
                    tile_identifiers: Some(SnapshotGalleryTiles(vec![])),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?
            }
            _ => return Err(Error::Ineligible),
        };
        response(&f, &body)
    }
}
