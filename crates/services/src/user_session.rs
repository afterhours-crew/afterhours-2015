// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Current connection network, hardware and latency updates.
//! The caller supplies its generated user notification. Exact retries ACK without
//! repeating notifications; unknown input ends this bounded startup sequence.
pub mod users_followup;
pub mod users_metrics;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    IneligibleRequest,
    ProfileShape,
    ProfileConfig,
    Reply,
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(64 * 1024, 0, 64 * 1024 - 16).expect("constant limits")
}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 64 * 1024 - 16,
        max_depth: 8,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
