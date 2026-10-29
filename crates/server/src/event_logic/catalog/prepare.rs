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
    sprints_cached(index, categories, vehicles, items, None)
}

/// A cache hit loads compiled static inputs; ordinary constructors still check
/// them against the current vehicle/item definitions before publication.
pub fn sprints_cached(
    index: &mut AssetIndex,
    categories: &BTreeMap<u32, Category>,
    vehicles: Option<&vehicle_content::Content>,
    items: Option<&ItemContent>,
    cache: Option<&crate::prepared_content::Cache>,
) -> Result<Prepared, Error> {
    let navigation = Arc::new(index.road_distance()?);
    let finish =
        |snapshot: Snapshot| snapshot.finish(categories, vehicles, items, navigation.clone());
    match cache {
        Some(cache) => cache.prepare(
            "normal-sprints",
            &serde_json::to_vec(categories)
                .map_err(|_| Error::Cache("sprint category identity"))?,
            |bytes| {
                let snapshot: Snapshot = serde_json::from_slice(bytes)
                    .map_err(|_| Error::Cache("invalid prepared sprint snapshot"))?;
                finish(snapshot)
            },
            || serde_json::to_vec(&build(index)?).map_err(|_| Error::Cache("sprint serialization")),
        ),
        None => finish(build(index)?),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    event: nfs_content::events::Event,
    awards: nfs_content::events::awards::Content,
    program: EventScoring,
    category: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    entries: Vec<Input>,
    roster: Vec<Entry>,
}

fn build(index: &mut AssetIndex) -> Result<Snapshot, Error> {
    let roster = index.event_roster()?;
    let mut entries = Vec::new();
    for entry in &roster {
        let Some(context) = request::sprint(entry) else {
            continue;
        };
        let started = Instant::now();
        let resolved = index.event_request(&context)?;
        let request_ms = started.elapsed().as_millis();
        let event = index.event_with_progress(&resolved, |phase, elapsed| {
            tracing::info!(
                event = entry.id,
                phase,
                elapsed_ms = elapsed.as_millis(),
                "event preparation stage complete"
            );
        })?;
        let event_ms = started.elapsed().as_millis();
        let stage = Instant::now();
        let awards_layout = event
            .game_mode_logics
            .iter()
            .find(|l| l.blueprint == request::AWARDS)
            .ok_or_else(|| Error::Content("event awards layout".into()))?;
        let awards = index.event_awards(&event, awards_layout)?;
        let awards_ms = stage.elapsed().as_millis();
        let driving = event
            .game_mode_logics
            .iter()
            .find(|l| l.blueprint == request::DRIVING_STYLE)
            .ok_or_else(|| Error::Content("event driving-style layout".into()))?;
        let stage = Instant::now();
        let program = index.event_scoring(driving)?;
        let category = index.speed_wall_category(&entry.id)?;
        let scoring_ms = stage.elapsed().as_millis();
        tracing::info!(
            event = entry.id,
            checkpoints = event.game_mode.checkpoints.len(),
            opponents = event.ai.spawns.len(),
            event_ms,
            request_ms,
            awards_ms,
            scoring_ms,
            total_ms = started.elapsed().as_millis(),
            "sprint content prepared"
        );
        entries.push(Input {
            event,
            awards,
            program,
            category,
        });
    }
    Ok(Snapshot { entries, roster })
}

impl Snapshot {
    fn finish(
        self,
        categories: &BTreeMap<u32, Category>,
        vehicles: Option<&vehicle_content::Content>,
        items: Option<&ItemContent>,
        navigation: Arc<nfs_world::road_distance::Network>,
    ) -> Result<Prepared, Error> {
        let invalid = || Error::Cache("invalid compiled sprint input");
        let expected: Vec<_> = self.roster.iter().filter_map(request::sprint).collect();
        if self.roster.len() > nfs_content::events::discovery::MAX_EVENTS
            || self.entries.is_empty()
            || self.entries.len() != expected.len()
        {
            return Err(invalid());
        }
        let mut entries = Vec::new();
        for (input, context) in self.entries.into_iter().zip(expected) {
            let Input {
                event,
                awards,
                program,
                category,
            } = input;
            if event.game_mode_id != context.game_mode_id
                || event.scene != context.scene
                || program.categories != *categories
                || categories.get(&awards.keeper) != Some(&Category::Total)
            {
                return Err(invalid());
            }
            crate::event_logic::lifecycle::Plan::installed(&event.game_mode.lifecycle)
                .map_err(|_| invalid())?;
            if let Some(recovery) = event.game_mode.recovery {
                recovery.validate().map_err(|_| invalid())?;
            }
            program.program.validate().map_err(|_| invalid())?;
            let driving = event
                .game_mode_logics
                .iter()
                .find(|l| l.blueprint == request::DRIVING_STYLE)
                .ok_or_else(invalid)?;
            nfs_content::scoring::content::EventParticipant {
                blueprint: program.blueprint.clone(),
                program: program.program.clone(),
                fields: program.fields.clone(),
                bus_entities: program.bus_entities.clone(),
                event_sync: vec![],
            }
            .validate_layout(driving)?;
            let asset = driving.asset;
            for (node, property) in &program.properties {
                if usize::from(*node) >= program.program.nodes.len() {
                    return Err(invalid());
                }
                property.validate().map_err(|_| invalid())?;
            }
            let content = Content::new(event, category)
                .and_then(|c| c.with_awards(awards))
                .and_then(|c| c.with_navigation(navigation.clone()))
                .map_err(|_| invalid())?;
            content
                .creation_content()
                .binding(asset)
                .map_err(|_| invalid())?;
            let written: BTreeSet<_> = program.properties.iter().map(|(_, p)| p).collect();
            if !content
                .presented_properties()
                .iter()
                .all(|p| written.contains(p))
            {
                return Err(invalid());
            }
            let content = if content.event().ai.spawns.is_empty() {
                content
            } else {
                content
                    .with_ai(
                        vehicles.ok_or_else(invalid)?.clone(),
                        items.ok_or_else(invalid)?,
                    )
                    .map_err(|_| invalid())?
            };
            entries.push((content, program, asset));
        }
        Catalog::new(
            entries.iter().map(|(c, _, _)| c.clone()).collect(),
            &self.roster,
        )
        .map_err(|_| invalid())?;
        Ok(Prepared {
            entries,
            roster: self.roster,
        })
    }
}
