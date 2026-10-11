// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned local backend integration. [`deployment`] binds policy and identities;
//! [`control`] takes framed input with injected time, and [`net`] supplies the
//! bounded Tokio socket edge. Static content is explicit configuration; account
//! state is durable and session/world state is isolated per connection.
extern crate self as nfs_server;
pub mod auxiliary;
pub mod cli;
pub mod client_state;
pub mod content;
pub mod control;
pub mod customization_timer;
pub mod deployment;
pub mod fresh_account;
pub mod garage_logic;
pub mod glass;
pub mod identity;
pub mod limits;
pub mod matchmaking_session;
pub mod matchmaking_status;
pub mod menu_news;
pub mod net;
pub mod persistent;
pub mod progression;
pub mod recommendations;
pub mod record;
pub mod scene_roles;
pub mod seeds;
pub mod sequence_content;
pub mod shared_wraps;
pub mod startup;
pub mod startup_branch;
pub mod startup_profile;
pub mod telemetry;
pub mod user_lookup;
pub mod user_settings;
pub mod vehicle_content;
mod world_connection;
pub mod world_handshake;
mod world_post_readiness;
pub mod world_readiness;
pub mod world_setup;

pub use nfs_server_support::Failure;
pub use nfs_services::{inventory, item_content};

pub fn bootstrap_failure(error: nfs_services::bootstrap::Error) -> Failure {
    match error {
        nfs_services::bootstrap::Error::Ineligible => Failure::IneligibleRequest,
        nfs_services::bootstrap::Error::Encode => Failure::Reply,
        _ => Failure::ProfileConfig,
    }
}

pub fn group_failure(error: nfs_services::group::Error) -> Failure {
    match error {
        nfs_services::group::Error::Config => Failure::ProfileConfig,
        nfs_services::group::Error::Ineligible => Failure::IneligibleRequest,
        nfs_services::group::Error::Encode => Failure::Reply,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    limits::control_frame_limits()
}
pub fn content_failure(error: nfs_services::ContentError) -> Failure {
    match error {
        nfs_services::ContentError::Invalid => Failure::ProfileConfig,
        nfs_services::ContentError::TooLarge => Failure::BodyLimit,
        nfs_services::ContentError::Io => Failure::Output,
    }
}
pub fn authentication_failure(error: nfs_services::authentication::Error) -> Failure {
    use nfs_services::authentication::Error;
    match error {
        Error::Ineligible => Failure::IneligibleRequest,
        Error::Encode => Failure::Reply,
        _ => Failure::ProfileConfig,
    }
}
pub fn user_session_failure(error: nfs_services::user_session::Error) -> Failure {
    match error {
        nfs_services::user_session::Error::IneligibleRequest => Failure::IneligibleRequest,
        nfs_services::user_session::Error::ProfileShape => Failure::ProfileShape,
        nfs_services::user_session::Error::ProfileConfig => Failure::ProfileConfig,
        nfs_services::user_session::Error::Reply => Failure::Reply,
    }
}

pub fn support_failure(error: Failure) -> Failure {
    error
}
