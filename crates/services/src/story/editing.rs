// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared CLI records. The server owns edits; launchers only consume records.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "story-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Milestone {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "story-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub version: u32,
    pub account_id: String,
    pub generation: String,
    pub source_checkpoint: String,
    pub target_checkpoint: String,
    pub operation_id: String,
    /// Binds this account, generation, target and exact prepared rules.
    pub plan_id: String,
    pub completes: Vec<Milestone>,
    pub reopens: Vec<Milestone>,
    pub can_apply: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "story-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Backup {
    /// One immutable directory under artifacts/story-backups, never a path.
    pub id: String,
    pub generation: String,
    pub database_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "story-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub version: u32,
    pub account_id: String,
    pub previous_generation: String,
    pub generation: String,
    pub target_checkpoint: String,
    pub replayed: bool,
    pub backup: Backup,
}
