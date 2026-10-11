// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! GameManager state payloads. These are wire models, not session policy.
//! names describe this build's enums; wire fields remain numeric so unknown
//! values survive. Enum names do not define allowed transitions or readiness.
use crate::{Blob, Error, Wire, schema};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits};
use std::{collections::BTreeSet, fmt};

mod group;
pub use group::*;
mod matchmaking;
pub use matchmaking::*;
mod criteria;
pub use criteria::*;
mod status;
pub use status::*;
mod attributes;
pub use attributes::*;
mod departure;
pub use departure::*;

pub const COMPONENT: u16 = 4;
pub const ADVANCE_GAME_STATE: u16 = 3;
pub const FINALIZE_GAME_CREATION: u16 = 15;
pub const NOTIFY_GAME_STATE_CHANGE: u16 = 100;

/// Names from the NFS16 GameState registration, not another title's enum.
pub const fn game_state_name(value: i64) -> Option<&'static str> {
    Some(match value {
        0 => "NEW_STATE",
        1 => "INITIALIZING",
        2 => "INACTIVE_VIRTUAL",
        3 => "CONNECTION_VERIFICATION",
        130 => "PRE_GAME",
        131 => "IN_GAME",
        4 => "POST_GAME",
        5 => "MIGRATING",
        6 => "DESTRUCTING",
        7 => "RESETABLE",
        8 => "REPLAY_SETUP",
        9 => "UNRESPONSIVE",
        16 => "GAME_GROUP_INITIALIZED",
        _ => return None,
    })
}

/// Names from the NFS16 GameType registration. Unknown values remain unnamed.
pub const fn game_type_name(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("GAME_TYPE_GAMESESSION"),
        1 => Some("GAME_TYPE_GROUP"),
        _ => None,
    }
}

/// DatalessContext registration. This describes the setup reason, not a
/// service permission or a general completion rule.
pub const fn dataless_context_name(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("CREATE_GAME_SETUP_CONTEXT"),
        1 => Some("JOIN_GAME_SETUP_CONTEXT"),
        2 => Some("INDIRECT_JOIN_GAME_FROM_QUEUE_SETUP_CONTEXT"),
        3 => Some("INDIRECT_JOIN_GAME_FROM_RESERVATION_CONTEXT"),
        4 => Some("HOST_INJECTION_SETUP_CONTEXT"),
        _ => None,
    }
}

schema!(AdvanceGameStateRequest {
    game_id: u64 => [0x9e, 0x99, 0x00],
    new_game_state: i64 => [0x9f, 0x3d, 0x21],
});
schema!(NotifyGameStateChange {
    game_id: u64 => [0x9e, 0x99, 0x00],
    new_game_state: i64 => [0x9f, 0x3d, 0x21],
});
// The client uses this payload for finalizeGameCreation (4/15), despite the
// different type name. Do not infer other routes from that name.
schema!(UpdateGameSessionRequest {
    game_id: u64 => [0x9e, 0x99, 0x00],
    np_session_id: &'a [u8] => [0xbb, 0x0c, 0xe9],
    xnet_nonce: Blob<'a> => [0xe2, 0xeb, 0xa3],
    xnet_session: Blob<'a> => [0xe3, 0x39, 0x73],
});
