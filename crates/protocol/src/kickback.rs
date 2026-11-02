// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Startup screenshot counters exchange.
//! Counts are account data, not universal defaults or an upload implementation.
use crate::{Error, Wire, schema};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits, Value};
use std::{collections::BTreeSet, fmt};

pub const COMPONENT: u16 = 2053;
pub const GET_SCREENSHOT_COUNTERS: u16 = 2;
pub const ADD_PENDING_SCREENSHOT: u16 = 8;
pub const COMMIT_SCREENSHOT: u16 = 9;
pub const GET_LAST_WEEKS_WINNER_DATA: u16 = 17;
pub const GET_SNAPSHOT_GALLERY_TILES: u16 = 22;

// Screenshot record allocation and commit (E162 native binding: four
// `Blaze::Kickback` types, eleven members; official bodies observed seven times
// in the E1026 Amy capture after race finishes). Member meanings beyond the
// tag names are unassigned: `AUTO` and `ST` were always 1, `X` is signed
// (observed -5250..159), `NAME` was the client's clock in seconds, and the
// official `RECN` was that `NAME` as decimal text.
schema!(AddPendingScreenshotRequest {
    automatic: u32 => [0x87, 0x5d, 0x2f],
    description: &'a [u8] => [0x92, 0x5c, 0xe3],
    name: u64 => [0xba, 0x1b, 0x65],
    st: u32 => [0xcf, 0x40, 0x00],
    x: i32 => [0xe0, 0x00, 0x00],
    y: i32 => [0xe4, 0x00, 0x00],
});
schema!(AddPendingScreenshotResponse {
    screenshot_id: u64 => [0xa6, 0x40, 0x00],
    record_name: &'a [u8] => [0xca, 0x58, 0xee],
});
schema!(CommitScreenshotRequest {
    screenshot_id: u64 => [0xa6, 0x40, 0x00],
});
// `SCC`/`SCCM` are distinct tags from the counters query's `SSC`/`SCCM`; the
// E162 diagram names them screenshotCount and screenshotCountMax.
schema!(CommitScreenshotResponse {
    screenshot_count: u32 => [0xce, 0x38, 0xc0],
    screenshot_count_max: u32 => [0xce, 0x38, 0xed],
});

/// The native request has zero members. Extra fields are not acknowledged.
#[derive(Debug, Default)]
pub struct GetScreenshotCounterRequest;
impl GetScreenshotCounterRequest {
    pub fn decode(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let document = nfs_heat2::decode(bytes, limits)?;
        if document.fields().next().is_some() {
            return Err(Error::WrongType { tag: None });
        }
        Ok(Self)
    }
    pub fn encode(&self, limits: Limits) -> Result<Vec<u8>, Error> {
        Ok(Encoder::new(limits).finish()?)
    }
}

schema!(GetScreenshotCounterResponse {
    screenshot_count_max: u32 => [0xce, 0x38, 0xed],
    screenshot_count: u32 => [0xcf, 0x38, 0xc0],
});

/// native request descriptor has zero members; this is not an account ID.
#[derive(Debug, Default)]
pub struct GetSnapshotGalleryTilesRequest;
impl GetSnapshotGalleryTilesRequest {
    pub fn decode(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let document = nfs_heat2::decode(bytes, limits)?;
        if document.fields().next().is_some() {
            return Err(Error::WrongType { tag: None });
        }
        Ok(Self)
    }
    pub fn encode(&self, limits: Limits) -> Result<Vec<u8>, Error> {
        Ok(Encoder::new(limits).finish()?)
    }
}
schema!(SnapshotGalleryTileIdentifier {
    screenshot_id: u64 => [0xa6,0x40,0x00],
    persona_id: i64 => [0xc2,0x99,0x00],
    record_name: &'a [u8] => [0xca,0x58,0xee],
});
#[derive(Default)]
pub struct SnapshotGalleryTiles<'a>(pub Vec<SnapshotGalleryTileIdentifier<'a>>);
impl fmt::Debug for SnapshotGalleryTiles<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SnapshotGalleryTiles")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for SnapshotGalleryTiles<'a> {
    const KIND: Kind = Kind::List;
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::List {
                element: Kind::Struct,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        Ok(Self(
            item.elements()
                .ok_or(Error::WrongType { tag: None })?
                .map(|i| SnapshotGalleryTileIdentifier::read(i?))
                .collect::<Result<_, _>>()?,
        ))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.struct_list(tag, &self.0, |w, v| v.write_fields(w))
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 0)?;
        for v in &self.0 {
            v.validate(budget)?;
        }
        Ok(())
    }
    fn unknown_count(&self) -> usize {
        self.0.iter().map(Wire::unknown_count).sum()
    }
}
schema!(GetSnapshotGalleryTilesResponse {
    tile_identifiers: SnapshotGalleryTiles<'a> => [0xa6,0x4c,0xc0],
});

/// zero-member native SnapshotGetLastWeekWinnerDataRequest.
#[derive(Debug, Default)]
pub struct GetLastWeekWinnerDataRequest;
impl GetLastWeekWinnerDataRequest {
    pub fn decode(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let document = nfs_heat2::decode(bytes, limits)?;
        if document.fields().next().is_some() {
            return Err(Error::WrongType { tag: None });
        }
        Ok(Self)
    }
    pub fn encode(&self, limits: Limits) -> Result<Vec<u8>, Error> {
        Ok(Encoder::new(limits).finish()?)
    }
}

// native ScreenshotGalleryDataEx: date units and screenshot-type meanings
// remain unassigned. PID identifies the content owner, not the requesting player.
schema!(ScreenshotGalleryDataEx {
    datetime: i64 => [0x92,0x1d,0x25],
    title: &'a [u8] => [0x92,0x5c,0xe3],
    screenshot_id: u64 => [0xa6,0x40,0x00],
    kickback_count: u32 => [0xb2,0x9a,0xe5],
    is_last_week_winner: bool => [0xb3,0x7d,0xc0],
    persona_id: i64 => [0xc2,0x99,0x00],
    player_provided_kickback: bool => [0xc2,0xca,0x6b],
    record_name: &'a [u8] => [0xca,0x58,0xee],
    screenshot_type: u16 => [0xd3,0x9c,0x25],
});
