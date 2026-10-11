// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! State-owned account services built on protocol and storage libraries.
//! Repository and file calls are synchronous; async edges must use blocking workers.
pub mod account_state;
pub mod authentication;
pub mod awards;
pub mod bootstrap;
pub mod challenges;
pub mod client_state;
pub mod control_catalogs;
pub mod departure;
pub mod fresh_account;
pub mod group;
pub mod inventory;
pub mod item_builder;
pub mod item_content;
pub mod persistent;
pub mod recommendations;
pub mod reputation;
pub mod shared_wraps;
pub mod stats;
pub mod telemetry;

/// Build identity required by versioned content documents; no game data is bundled.
pub const SUPPORTED_BUILD_SHA256: &str =
    "92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df";

/// Bounded content loading failures without a dependency on a server or recorder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentError {
    Invalid,
    TooLarge,
    Io,
}
impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "content {self:?}")
    }
}
impl std::error::Error for ContentError {}

/// Bytes allowed for the metadata of an error answer: context and error code.
pub const ERROR_METADATA_BYTES: usize = 16;

/// A Blaze error answer to `request`: category 3 with the request's routing and
/// correlation, metadata holding context 0 and `code`, and an empty body, the
/// shape of the observed error answers. `None` when it cannot be encoded.
pub fn error_reply(request: nfs_fire2::Fields, code: i32) -> Option<Vec<u8>> {
    let limits = nfs_heat2::Limits {
        max_bytes: ERROR_METADATA_BYTES,
        max_depth: 2,
        max_values: 4,
        max_collection: 1,
        max_byte_string: 1,
    };
    let metadata = nfs_protocol::metadata::Fire2Metadata {
        context: Some(0),
        error_code: Some(code),
        ..Default::default()
    }
    .encode(limits)
    .ok()?;
    nfs_fire2::encode(
        nfs_fire2::Frame {
            fields: nfs_fire2::Fields {
                category: 3,
                ..request
            },
            metadata: &metadata,
            body: &[],
        },
        nfs_fire2::Limits::new(
            nfs_fire2::HEADER_LEN + ERROR_METADATA_BYTES,
            ERROR_METADATA_BYTES,
            0,
        )
        .ok()?,
    )
    .ok()
}

pub mod world_readiness;

pub mod matchmaking_status;
pub mod world_attributes;
pub mod world_connection;
pub mod world_setup;

pub mod matchmaking;

pub mod user_settings;

pub mod menu_news;
pub mod user_lookup;

pub mod local_social;

pub mod association;
pub mod entitlements;
pub mod heartbeat;
pub mod item_licenses;
pub mod kickback;
pub mod speedwall;

pub mod user_session;
