// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The saved story view, using the same objective rules as the game server.
use crate::{
    exit::{Failure, Result},
    paths, profile, session,
};
use nfs_services::{
    progression::{Catalog, Settings},
    story::{Policy, View},
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{io::Read, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct StoryCheckpoint {
    /// Exact native objective ID; `after-intro` is the terminal checkpoint.
    pub id: String,
    /// Authored label when the installed catalog supplies it.
    pub label: String,
    pub active: bool,
    pub completed: bool,
    pub played: bool,
    pub edited: bool,
    /// Completion inherited from a save without a gameplay provenance receipt.
    pub existing: bool,
    pub reward_claimed: bool,
    /// True only after this runtime supports starting from this boundary.
    pub selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct StoryReport {
    pub version: u32,
    pub account_id: String,
    pub generation: String,
    pub current_checkpoint: String,
    pub checkpoints: Vec<StoryCheckpoint>,
    pub intro_completed: bool,
    pub session_running: bool,
    pub can_edit: bool,
    pub warnings: Vec<String>,
}

fn rules(directory: &Path) -> Result<(Catalog, Settings)> {
    let path = directory.join("progression-content.json");
    let metadata = std::fs::symlink_metadata(&path)?;
    if paths::is_link(&metadata) || !metadata.is_file() || metadata.len() > 512 * 1024 {
        return Err(Failure::refused(
            "Invalid or oversized progression content.",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(512 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 512 * 1024 {
        return Err(Failure::refused("Oversized progression content."));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value["format"] != "nfs-progression-entities"
        || value["version"] != 1
        || value["build_sha256"] != nfs_services::SUPPORTED_BUILD_SHA256
    {
        return Err(Failure::refused(
            "Unsupported progression content format or game build.",
        ));
    }
    let catalog = Catalog::from_json(&value["restoration"])
        .map_err(|_| Failure::refused("Invalid progression rules."))?;
    let consumer_beta = value["settings"]["consumer_beta"]
        .as_bool()
        .ok_or_else(|| Failure::refused("Invalid progression settings."))?;
    Ok((catalog, Settings { consumer_beta }))
}

pub fn report(data: &Path, id: Option<&str>, content_dir: Option<&Path>) -> Result<StoryReport> {
    let saves = profile::Saves::open(data)?;
    let id = id
        .or_else(|| saves.active())
        .ok_or_else(|| Failure::no_session("No local driver selected; run setup first."))?;
    let (_, _, inspection) = saves.inspect(id)?;
    let source = profile::content::locate(saves.data(), content_dir)?
        .ok_or_else(|| Failure::no_session("No prepared story content found; run setup first."))?;
    let (catalog, settings) = rules(&source.directory)?;
    let state = Policy {
        catalog: &catalog,
        settings,
    }
    .inspect(&inspection.snapshot)
    .map_err(|_| Failure::refused("Saved story objectives are incomplete or inconsistent."))?;
    let running = session::lock_is_held(saves.data())? == Some(true);
    Ok(build(id, &catalog, state, running))
}

pub(crate) fn build(id: &str, catalog: &Catalog, state: View, running: bool) -> StoryReport {
    let current = state
        .milestones
        .get(state.position)
        .map_or_else(|| "after-intro".to_owned(), |m| m.id.to_string());
    let intro_completed = state.position == state.milestones.len();
    let terminal_played = intro_completed && state.milestones.iter().all(|m| m.played);
    let terminal_edited = intro_completed && state.milestones.iter().any(|m| m.edited);
    let terminal_existing = intro_completed && state.milestones.iter().any(|m| m.existing);
    let mut checkpoints = state
        .milestones
        .into_iter()
        .map(|m| StoryCheckpoint {
            id: m.id.to_string(),
            label: catalog
                .objective_label(m.id)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Objective {}", m.id)),
            active: m.active,
            completed: m.completed,
            played: m.played,
            edited: m.edited,
            existing: m.existing,
            reward_claimed: m.reward_claimed,
            selectable: false,
        })
        .collect::<Vec<_>>();
    checkpoints.push(StoryCheckpoint {
        id: "after-intro".into(),
        label: "Prologue complete".into(),
        active: intro_completed,
        completed: intro_completed,
        played: terminal_played,
        edited: terminal_edited,
        existing: terminal_existing,
        reward_claimed: false,
        selectable: false,
    });
    let mut warnings =
        vec!["Story checkpoint editing awaits playable prologue integration.".to_owned()];
    if !state.stats_consistent {
        warnings.push(
            "Legacy intro summary differs from objective rows; the saved objectives are shown."
                .to_owned(),
        );
    }
    StoryReport {
        version: 1,
        account_id: id.to_owned(),
        generation: state.generation.to_string(),
        current_checkpoint: current,
        checkpoints,
        intro_completed,
        session_running: running,
        can_edit: false,
        warnings,
    }
}

#[cfg(test)]
mod tests;
