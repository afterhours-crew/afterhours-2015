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
    story::{Policy, View, editing::Options},
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{io::Read, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Starter {
    pub id: String,
    pub name: Option<String>,
    pub image: Option<profile::images::CarImages>,
}

/// The server owns the plan; the launcher adds installed names and cached images.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Preview {
    pub version: u32,
    pub account_id: String,
    pub generation: String,
    pub source_checkpoint: String,
    pub target_checkpoint: String,
    pub operation_id: String,
    pub plan_id: String,
    pub completes: Vec<nfs_services::story::editing::Milestone>,
    pub reopens: Vec<nfs_services::story::editing::Milestone>,
    pub can_apply: bool,
    pub reasons: Vec<String>,
    pub starters: Vec<Starter>,
    pub starter: Option<String>,
    pub car_images: Option<std::path::PathBuf>,
}

impl Preview {
    pub fn enrich(
        plan: nfs_services::story::editing::Preview,
        content: &profile::content::Content,
    ) -> Self {
        let starters = plan
            .starters
            .iter()
            .map(|id| Starter {
                id: id.clone(),
                name: profile::content::guid(id)
                    .and_then(|guid| content.car_names.as_ref()?.get(&guid)?.names.long.clone()),
                image: content
                    .car_images
                    .as_ref()
                    .and_then(|images| images.get(id)),
            })
            .collect();
        Self {
            version: plan.version,
            account_id: plan.account_id,
            generation: plan.generation,
            source_checkpoint: plan.source_checkpoint,
            target_checkpoint: plan.target_checkpoint,
            operation_id: plan.operation_id,
            plan_id: plan.plan_id,
            completes: plan.completes,
            reopens: plan.reopens,
            can_apply: plan.can_apply,
            reasons: plan.reasons,
            starters,
            starter: plan.starter,
            car_images: content
                .car_images
                .as_ref()
                .map(|images| images.directory.clone()),
        }
    }
}

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

fn rules(directory: &Path) -> Result<(Catalog, Settings, Option<Options>)> {
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
    let (catalog, settings) = nfs_services::progression::checkpoint_rules(&value)
        .map_err(|_| Failure::refused("Unsupported or invalid progression checkpoint rules."))?;
    let options = Options::parse(&value, &catalog, settings)
        .map_err(|_| Failure::refused("Invalid story editor content; run setup again."))?;
    Ok((catalog, settings, options))
}

pub fn report(data: &Path, id: Option<&str>, content_dir: Option<&Path>) -> Result<StoryReport> {
    let saves = profile::Saves::open(data)?;
    let id = id
        .or_else(|| saves.active())
        .ok_or_else(|| Failure::no_session("No local driver selected; run setup first."))?;
    let (_, _, inspection) = saves.inspect(id)?;
    let source = profile::content::locate(saves.data(), content_dir)?
        .ok_or_else(|| Failure::no_session("No prepared story content found; run setup first."))?;
    let (catalog, settings, options) = rules(&source.directory)?;
    let state = Policy {
        catalog: &catalog,
        settings,
    }
    .inspect(&inspection.snapshot)
    .map_err(|_| Failure::refused("Saved story objectives are incomplete or inconsistent."))?;
    let running =
        session::lock_is_held(saves.data())? == Some(true) || saves.writer_is_leased(id)?;
    Ok(build(id, &catalog, state, running, options.as_ref()))
}

pub(crate) fn build(
    id: &str,
    catalog: &Catalog,
    state: View,
    running: bool,
    options: Option<&Options>,
) -> StoryReport {
    let current = state
        .milestones
        .get(state.position)
        .map_or_else(|| "after-intro".to_owned(), |m| m.id.to_string());
    let intro_completed = state.position == state.milestones.len();
    let terminal_played = intro_completed && state.milestones.iter().all(|m| m.played);
    let terminal_edited = intro_completed && state.milestones.iter().any(|m| m.edited);
    let terminal_existing = intro_completed && state.milestones.iter().any(|m| m.existing);
    let supported = state
        .milestones
        .iter()
        .map(|m| {
            options.is_some_and(|o| {
                o.supports(nfs_services::story::Checkpoint::Objective(m.id), &state)
            })
        })
        .collect::<Vec<_>>();
    let mut checkpoints = state
        .milestones
        .into_iter()
        .enumerate()
        .map(|(index, m)| StoryCheckpoint {
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
            selectable: supported[index],
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
        selectable: options.is_some(),
    });
    let mut warnings = if options.is_some() {
        vec!["Replay starts from the beginning and keeps earned cars and progress. The starter choice reuses your earned car. Middle steps cannot be used as restart points yet.".to_owned()]
    } else {
        vec!["Run setup to prepare story editing for this installation.".to_owned()]
    };
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
        can_edit: options.is_some() && !running,
        warnings,
    }
}

#[cfg(test)]
mod tests;
