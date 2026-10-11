// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Fire2's heat2 metadata region, separate from the framing library and message body.
use crate::{Error, Wire, schema};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits};
use std::{collections::BTreeSet, fmt};

schema!(Fire2Metadata {
    context: u64 => [0x8e, 0xed, 0x38],
    error_code: i32 => [0x97, 0x2c, 0xa3],
    session_key: &'a [u8] => [0xce, 0xb9, 0x79],
});

/// Util `userSettingsLoad` error for a key the account does not hold.
pub const UTIL_USS_RECORD_NOT_FOUND: i32 = 0x00c8_0009;
/// Kickback error for a record that does not exist.
pub const KICKBACK_ERR_NOT_FOUND: i32 = 0x0004_0805;

/// Names for the observed nonzero statuses and the not-found statuses the
/// services return, each verified in its component's error-name method (the
/// code is `code << 16 | component`). This is deliberately incomplete and
/// supplies no retry policy. Keep the original error_code; a missing name
/// does not imply success.
pub const fn error_name(component: u16, code: i32) -> Option<&'static str> {
    match (component, code) {
        (30_722, 0x0001_7802) => Some("USER_ERR_USER_NOT_FOUND"),
        (2_050, 0x001b_0802) => Some("AUTOLOG_ERR_NEWS_JSON_DECODE"),
        (9, UTIL_USS_RECORD_NOT_FOUND) => Some("UTIL_USS_RECORD_NOT_FOUND"),
        (2_053, KICKBACK_ERR_NOT_FOUND) => Some("KICKBACK_ERR_NOT_FOUND"),
        _ => None,
    }
}
