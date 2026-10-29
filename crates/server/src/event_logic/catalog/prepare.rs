// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Blocking installation preparation shared by startup and offline audits.
use super::{Catalog, Content, Entry};
use crate::{event_logic::request, scoring, vehicle_content};
use nfs_content::{Error, assets::AssetIndex, scoring::content::EventScoring};
use nfs_services::item_content::ItemContent;
use nfs_world::{replication::entity::creation::Asset, scoring::interpreter::Category};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

pub struct Prepared {
    entries: Vec<(Content, EventScoring, Asset)>,
    roster: Vec<Entry>,
}

impl Prepared {
    /// Attach the entire batch atomically. A missing program or binding cannot
    /// leave a visible playable entry whose scoring silently uses another race.
    pub fn attach(
        self,
        mut scoring: scoring::Content,
    ) -> Result<(Catalog, scoring::Content), nfs_world::replication::Error> {
        let mut contents = Vec::new();
        for (content, program, asset) in self.entries {
            scoring = scoring.with_authored_event(
                content.record_id(),
                program,
                content.creation_content(),
                asset,
            )?;
            contents.push(content);
        }
        Ok((Catalog::new(contents, &self.roster)?, scoring))
    }
}

pub fn sprints(
    index: &mut AssetIndex,
    categories: &BTreeMap<u32, Category>,
    vehicles: Option<&vehicle_content::Content>,
    items: Option<&ItemContent>,
) -> Result<Prepared, Error> {
    let roster = index.event_roster()?;
    let navigation = Arc::new(index.road_distance()?);
    let mut entries = Vec::new();
    for entry in &roster {
        let Some(context) = request::sprint(entry) else {
            continue;
        };
        let started = Instant::now();
        let resolved = index.event_request(&context)?;
        let event = index.event(&resolved)?;
        let event_ms = started.elapsed().as_millis();
        let stage = Instant::now();
        let awards_layout = event
            .game_mode_logics
            .iter()
            .find(|l| l.blueprint == request::AWARDS)
            .ok_or_else(|| Error::Content("event awards layout".into()))?;
        let awards = index.event_awards(&event, awards_layout)?;
        let awards_ms = stage.elapsed().as_millis();
        if categories.get(&awards.keeper) != Some(&Category::Total) {
            return Err(Error::Content("event award keeper category".into()));
        }
        let driving = event
            .game_mode_logics
            .iter()
            .find(|l| l.blueprint == request::DRIVING_STYLE)
            .ok_or_else(|| Error::Content("event driving-style layout".into()))?;
        let asset = driving.asset;
        let stage = Instant::now();
        let program = index.event_scoring(driving)?;
        let category = index.speed_wall_category(&entry.id)?;
        let scoring_ms = stage.elapsed().as_millis();
        let stage = Instant::now();
        let content = Content::new(event, category)
            .and_then(|c| c.with_awards(awards))
            .and_then(|c| c.with_navigation(navigation.clone()))
            .map_err(|e| Error::Content(format!("sprint {}: {e:?}", entry.id)))?;
        let construction_ms = stage.elapsed().as_millis();
        let written: BTreeSet<_> = program.properties.iter().map(|(_, p)| p).collect();
        if !content
            .presented_properties()
            .iter()
            .all(|p| written.contains(p))
        {
            return Err(Error::Content(format!(
                "sprint {} results have unwritten properties",
                entry.id
            )));
        }
        let stage = Instant::now();
        let content = if content.event().ai.spawns.is_empty() {
            content
        } else {
            content
                .with_ai(
                    vehicles
                        .ok_or_else(|| Error::Content("event AI vehicles".into()))?
                        .clone(),
                    items.ok_or_else(|| Error::Content("event AI items".into()))?,
                )
                .map_err(|e| Error::Content(format!("sprint AI {}: {e:?}", entry.id)))?
        };
        tracing::info!(
            event = entry.id,
            checkpoints = content.event().game_mode.checkpoints.len(),
            opponents = content.event().ai.spawns.len(),
            event_ms,
            awards_ms,
            scoring_ms,
            construction_ms,
            opponents_ms = stage.elapsed().as_millis(),
            total_ms = started.elapsed().as_millis(),
            "sprint content prepared"
        );
        entries.push((content, program, asset));
    }
    Ok(Prepared { entries, roster })
}
