// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static vehicle composition and constructor inputs from installed blueprints.
use nfs_world::items::Guid;
use nfs_world::replication::{Error, vehicle::*};
use serde_json::Value;
use std::collections::BTreeMap;

mod defaults;
mod extract;
mod fields;
mod graph;
mod info;
mod parse;
mod rules;
pub mod traffic;
pub use crate::vehicle_parts::ParseError;
pub(crate) use extract::build;
pub use rules::Rules;

/// Shared, validated constructor inputs. Dynamic selections come from Items.
#[derive(Clone, Debug)]
pub struct Definition {
    pub blueprint: String,
    pub bundle: String,
    pub asset_type: u16,
    pub asset_index: u16,
    pub catalog: Vec<(u16, u16)>,
    pub profile: Profile,
    pub root: root::Blueprint,
    pub resting: placement::RestingConfig,
    pub team: u8,
    pub mode: u8,
    pub parts: BTreeMap<usize, bool>,
    pub wheel_baseline: [f32; 2],
    pub appearance: appearance::Defaults,
    pub meshes: Vec<mesh::Component>,
    pub health: health::State,
    /// Installed vehicle identity for event records; absent in older profiles.
    pub info: Option<nfs_world::scoring::vehicle_info::Sample>,
}

impl Definition {
    pub fn from_json(value: &Value) -> Result<Self, ParseError> {
        parse::definition(value)
    }
}
