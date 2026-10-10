// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned world sessions, typed replication and player/garage state.
//!
//! [`session::Host`] is sans-IO: callers supply datagrams, time, randomness and
//! explicit content schemas. Socket ownership belongs to the server edge.
//! Scene roles are deployment inputs; this crate contains no asset catalogs.

pub mod actors;
pub mod arrival;
pub mod frame;
pub mod garage;
pub mod launchers;
pub mod level_poll;
pub mod logic;
pub mod participants;
pub mod replication;
pub mod sequences;
pub mod session;
pub mod spawn_points;
pub mod teleport;

pub use nfs_world_core::{
    application, bits, content, crypto, files, handshake, items, link, transport,
};

#[cfg(test)]
mod test_data {
    pub const GAMEPLAY_KEY: u32 = 101;
    pub const TRAFFIC_KEYS: [u32; 10] = [11, 12, 13, 14, 15, 16, 17, 18, 19, 20];
}
