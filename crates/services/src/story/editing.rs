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
    /// Binds the exact save file, account, generation, target and prepared rules.
    pub plan_id: String,
    pub completes: Vec<Milestone>,
    pub reopens: Vec<Milestone>,
    pub can_apply: bool,
    pub reasons: Vec<String>,
    /// Authored first-car definitions, only when skipping requires a new grant.
    pub starters: Vec<String>,
    /// The first-car choice included in this exact preview proof.
    pub starter: Option<String>,
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
    pub starter: Option<String>,
}

/// Installation-derived choices shared by the editor and read-only launcher.
/// Absence in an older content cache disables editing until setup rebuilds it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub version: u32,
    pub selection: String,
    pub starters: Vec<String>,
}

impl Options {
    /// Only entry paths already implemented by the runtime. Intermediate
    /// objectives remain useful progress markers, not restart promises.
    pub fn supports(&self, target: super::Checkpoint, view: &super::View) -> bool {
        target == super::Checkpoint::AfterIntro
            || view
                .milestones
                .first()
                .is_some_and(|m| target == super::Checkpoint::Objective(m.id))
    }

    pub fn parse(
        value: &serde_json::Value,
        catalog: &super::Catalog,
        settings: super::Settings,
    ) -> Result<Option<Self>, nfs_storage::Error> {
        let Some(value) = value.get("story_editor") else {
            return Ok(None);
        };
        let options: Self =
            serde_json::from_value(value.clone()).map_err(|_| nfs_storage::Error::Config)?;
        let order = catalog
            .intro_order(settings)
            .map_err(|_| nfs_storage::Error::Config)?;
        if options.version != 1
            || order.get(1).map(u64::to_string).as_ref() != Some(&options.selection)
            || options.starters.len() != 3
            || options
                .starters
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != 3
            || options.starters.iter().any(|s| {
                s.len() != 32
                    || s == &"0".repeat(32)
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(nfs_storage::Error::Config);
        }
        Ok(Some(options))
    }
}
