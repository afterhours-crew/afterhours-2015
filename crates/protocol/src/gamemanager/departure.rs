// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Departure payloads: `leaveGameByGroup` (`4/22`) and the player-removed
//! notification (`4/40`). Codecs carry no membership policy.
use crate::{Error, Wire, schema, users::ObjectId};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits};
use std::{collections::BTreeSet, fmt};

pub const LEAVE_GAME_BY_GROUP: u16 = 22;
pub const NOTIFY_PLAYER_REMOVED: u16 = 40;

/// `PlayerRemovedReason` values, numbered by the order of the client's
/// reason-name table (`PLAYER_JOIN_TIMEOUT` is 0).
pub const PLAYER_LEFT: i32 = 7;
pub const GROUP_LEFT: i32 = 8;

schema!(LeaveGameByGroupRequest {
    blaze_object_type_and_id: ObjectId => [0x8b, 0x4c, 0x2c],
    player_removed_title_context: u64 => [0x8e, 0xed, 0x38],
    game_id: u64 => [0x9e, 0x99, 0x00],
    player_id: i64 => [0xc2, 0x99, 0x00],
    player_removed_reason: i32 => [0xca, 0x58, 0x73],
    title_context_string: &'a [u8] => [0xce, 0x3d, 0x38],
});

schema!(NotifyPlayerRemoved {
    player_removed_title_context: u64 => [0x8e, 0xed, 0x38],
    game_id: u64 => [0x9e, 0x99, 0x00],
    is_lockable_for_preferred_joins: bool => [0xb2, 0x6c, 0x2a],
    player_id: i64 => [0xc2, 0x99, 0x00],
    player_removed_reason: i32 => [0xca, 0x58, 0x73],
});
