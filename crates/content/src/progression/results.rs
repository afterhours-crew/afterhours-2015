// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Selected direct participant-result completions, with authored enable and
//! category filters. Result production and ownership remain the event service's
//! responsibility; this reader does not turn a client report into success.
use super::conditions::{self, Condition, Fact};
use crate::{Error, assets::AssetIndex, graph::*};
use nfs_frostbite::ebx::{Pointer, Value};
use std::collections::{BTreeMap, BTreeSet};

const COMPLETE: u32 = 448_895_852;
const REPORTED: u32 = 89_858_563;
const ENABLED: u32 = 2_662_400;
const DETAILS: u32 = 109_263_670;
const MANAGER: u32 = (-1_713_696_348i32) as u32;
const PARTICIPANT: u32 = (-1_232_400_965i32) as u32;
const GAMEPLAY: &str = "levels/genesis01/gameplay";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub objective: u64,
    pub categories: BTreeSet<u32>,
    pub enabled: Condition,
}

impl AssetIndex {
    /// Every selected objective must have one direct server-side result source.
    /// Filtered/event-gated completions are not approximated by this reader.
    pub fn objective_results(
        &mut self,
        objectives: &BTreeSet<u64>,
    ) -> Result<Vec<Completion>, Error> {
        if objectives.is_empty() || objectives.len() > 64 || objectives.contains(&0) {
            return Err(bad("objective result selection bound"));
        }
        let rules = self.identity.progression.clone();
        let level = self.identity.scenes.level.clone();
        let startup = self.startup_entries(&level)?;
        let managers = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == "MessageManagerEntityData")
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let mut graph = Graph::new(&mut self.assets);
        let roots = crate::graph::scenes::scene_roots(&mut graph, &level, &startup)?;
        let scene = roots
            .get(&rules.scene)
            .ok_or_else(|| bad("result scene not loaded"))?
            .clone();
        let (nodes, aliases) = crate::participant_logic::context(
            &mut graph,
            scene,
            &rules.main,
            rules.ports["root_channels"],
            rules.ports["spawner_channels"],
        )?;
        let world = graph.read(Site::root(GAMEPLAY))?;
        read(
            &mut graph,
            &nodes,
            &world,
            &managers,
            rules.ports["stable_id"],
            objectives,
            &aliases,
        )
    }
}

fn direct<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    class: &str,
    aliases: &BTreeMap<Site, Site>,
) -> Result<Site, Error> {
    let ends = crate::participant_logic::links(graph, site, port, aliases, &mut BTreeSet::new())?;
    match ends.as_slice() {
        [end] if end.port == 0 && graph.object(&end.site)?.class == class => Ok(end.site.clone()),
        _ => Err(bad(&format!(
            "objective result needs one {class} at {port}: {ends:?}"
        ))),
    }
}

fn read<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    managers: &BTreeMap<Key, usize>,
    stable: u32,
    objectives: &BTreeSet<u64>,
    aliases: &BTreeMap<Site, Site>,
) -> Result<Vec<Completion>, Error> {
    let mut found = BTreeSet::new();
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|n| n.object.class == "RaceObjectiveEntityData")
    {
        let objective = crate::chapter_notifications::stable(graph, stable, &node.site)?;
        if !objectives.contains(&objective) {
            continue;
        }
        if !found.insert(objective) {
            return Err(bad("duplicate result objective"));
        }
        let (reporter, port) = super::delays::server_source(graph, &node.site, COMPLETE)?;
        let object = graph.object(&reporter)?;
        if port != REPORTED
            || object.class != "ParticipantResultReporterEntityData"
            || object.fields.get("Id").and_then(Value::as_pointer) != Some(Pointer::Null)
            || object.fields.get("Enabled").and_then(Value::as_bool) != Some(true)
            || object
                .fields
                .get("MatchAnyCategory")
                .and_then(Value::as_bool)
                != Some(false)
            || !object
                .fields
                .get("Categories")
                .and_then(Value::as_array)
                .is_some_and(|a| a.is_empty())
            || graph.input_ports(&reporter)? != [ENABLED]
        {
            return Err(bad("unsupported objective result reporter policy"));
        }
        let details = direct(
            graph,
            &reporter,
            DETAILS,
            "DetailsRefererEntityData",
            aliases,
        )?;
        if graph
            .object(&details)?
            .fields
            .get("Id")
            .and_then(Value::as_pointer)
            != Some(Pointer::Null)
        {
            return Err(bad("objective result details include an ID filter"));
        }
        let categories = crate::events::intro::results::categories(graph, &details)?;
        if categories.is_empty() {
            return Err(bad("objective result has no category filter"));
        }
        direct(
            graph,
            &reporter,
            MANAGER,
            "ResultManagerEntityData",
            aliases,
        )?;
        let provider = direct(
            graph,
            &reporter,
            PARTICIPANT,
            "ParticipantLogicParticipantProviderEntityData",
            aliases,
        )?;
        if !nodes.iter().any(|n| n.site == provider) {
            return Err(bad("objective result participant is outside owning branch"));
        }
        let input = graph.input(&reporter, ENABLED)?;
        let enabled = conditions::from_endpoint(graph, input, stable, world, managers)?;
        if !enabled.facts().any(|f| f == Fact::Active(objective)) {
            return Err(bad("objective result does not depend on its activation"));
        }
        // Event-driven enable/disable inputs would need their own state owner.
        let bus = graph.source.object(&reporter.bus)?;
        for row in bus
            .fields
            .get("EventConnections")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("objective result event table"))?
        {
            let fields = row
                .as_fields()
                .ok_or_else(|| bad("objective result event row"))?;
            let pointer = fields
                .get("Target")
                .and_then(Value::as_pointer)
                .ok_or_else(|| bad("objective result event target"))?;
            if graph.source.target(&reporter.bus.0, pointer)?.as_ref() == Some(&reporter.key) {
                return Err(bad("objective result reporter has event inputs"));
            }
        }
        out.push(Completion {
            objective,
            categories,
            enabled,
        });
    }
    if &found != objectives {
        return Err(bad("selected result objective missing"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
