// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! observed getTimeLimitedFeatures request subset.
//! This models the DEDA string on the wire, without assigning semantics to its
//! contents or inventing the schema of a nonempty response.
use crate::{Error, Wire, schema};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits};
use std::{collections::BTreeSet, fmt};

pub const COMPONENT: u16 = 2050;
pub const GET_TIME_LIMITED_FEATURES: u16 = 78;

mod awards;
mod speedwall;
pub use speedwall::*;
mod walls;
pub use walls::*;
mod settings;
pub use awards::*;
mod startup;
pub use settings::{
    GET_USER_SETTINGS, SET_USER_SETTINGS, SettingsFloatBits, SettingsIntegers, UserSettingsRequest,
    UserSettingsResponse, UserSettingsUpdateRequest, UserSettingsUpdateResponse,
};
pub use startup::{
    GET_KILL_SWITCHES, GET_RANDOM_PLAYERS, GET_RECENT_PLAYERS, GetRecentPlayersRequest,
    KillSwitchRequest, KillSwitchResponse, PlayerIds, RandomPlayersRequest, RandomPlayersResponse,
    StringList,
};

schema!(GetTimeLimitedFeaturesRequest { deda: &'a [u8] => [0x92,0x59,0x21] });

/// : native FriendsRecommendationsRequest has one int64 blazeId member.
/// The nonempty FriendsRecommendationsResponse element graph is not modeled.
pub const GET_FRIENDS_RECOMMENDATIONS: u16 = 26;
schema!(FriendsRecommendationsRequest { blaze_id: i64 => [0x8a,0xca,0x64] });

/// native NewsRequest. Success news bodies remain outside this codec.
pub const GET_NEWS: u16 = 74;
schema!(NewsRequest {
    debug_date: &'a [u8] => [0x92,0x59,0x21],
    locale: &'a [u8] => [0xb2,0xf8,0xc0],
});
