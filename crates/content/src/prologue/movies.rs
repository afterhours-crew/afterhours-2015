// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A selected story movie and its before/after sequences, resolved through the
//! exact capability occurrence and the intro participant's bound scene channels.
use super::*;
use crate::{graph::scenes::SceneLookup, scoring::events::Identity};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sequence {
    pub blueprint: String,
    pub reference: Identity,
    pub state: (u32, usize),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Movie {
    pub objective: u64,
    pub blueprint: String,
    pub video: nfs_world::fmv::Video,
    pub state: (u32, usize),
    pub before: Sequence,
    pub after: Sequence,
}
struct Raw {
    objective: u64,
    video: crate::graph::Key,
    state: Site,
    sequences: [(String, Site, Site); 2],
}
pub(super) fn sequence_state<S: Source>(
    graph: &mut Graph<S>,
    provider: &Site,
) -> Result<Site, Error> {
    let nodes = graph.read(Site {
        key: provider.bus.clone(),
        ..provider.clone()
    })?;
    let mut states = vec![];
    for node in nodes
        .iter()
        .filter(|n| n.object.class == "StateSequenceParticipantStateEntityData")
    {
        let ends = graph.links(&node.site, (-1_813_733_428i32) as u32)?;
        if ends.len() == 1 && ends[0].port == 0 && ends[0].site == *provider {
            states.push(node.site.clone());
        }
    }
    let [state] = states.as_slice() else {
        return Err(bad("movie sequence provider state occurrence"));
    };
    Ok(state.clone())
}
pub(super) fn bound<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    aliases: &BTreeMap<Site, Site>,
    class: &str,
) -> Result<Site, Error> {
    let mut edges = graph.links(site, port)?;
    let mut seen = BTreeSet::new();
    loop {
        let [end] = edges.as_slice() else {
            return Err(bad("movie link cardinality"));
        };
        if !seen.insert(end.clone()) || seen.len() > 64 {
            return Err(bad("movie link cycle"));
        }
        let object = graph.object(&end.site)?;
        if object.class == class && end.port == 0 {
            return Ok(end.site.clone());
        }
        if object.class != "LinkedSchematicChannelReferenceEntityData" {
            return Err(bad(&format!(
                "movie expected {class}, found {} port {}",
                object.class, end.port
            )));
        }
        let alias = aliases
            .get(&end.site)
            .ok_or_else(|| bad("movie unbound channel"))?;
        edges = graph.links(alias, end.port)?;
    }
}
fn read<S: Source>(
    graph: &mut Graph<S>,
    root: Site,
    aliases: &BTreeMap<Site, Site>,
    stable_port: u32,
) -> Result<Raw, Error> {
    let nodes = graph.read(root)?;
    let selector = unique(&nodes, "FMVSelectorEntityData")?;
    let provider = bound(
        graph,
        selector,
        (-1_232_400_965i32) as u32,
        aliases,
        "ParticipantLogicParticipantProviderEntityData",
    )?;
    let referer = bound(
        graph,
        selector,
        777_057_321,
        aliases,
        "FMVRefererEntityData",
    )?;
    let video = graph.target_field(&referer, "VideoAsset")?;
    if graph.source.object(&video)?.class != "VideoRefererAsset" {
        return Err(bad("movie video class"));
    }
    let state = bound(
        graph,
        selector,
        1_234_675_151,
        aliases,
        "FMVParticipantStateEntityData",
    )?;
    // PlayFMV's incoming Play invokes before-selector, after-selector, video
    // selector and the participant transition in this authored order.
    let mut sequence_sites = Vec::new();
    let source = graph.event_sources(selector, 1_175_083_884)?;
    let [input] = source.as_slice() else {
        return Err(bad("movie play input"));
    };
    let composition = crate::scoring::events::read_at(
        graph.source,
        Site {
            key: selector.bus.clone(),
            ..selector.clone()
        },
    )?;
    let source_node = composition
        .nodes
        .iter()
        .position(|n| n.identity == Identity::from(&input.0))
        .ok_or_else(|| bad("movie play source"))?;
    if !composition.connections.iter().any(|e| {
        usize::from(e.source.node) == source_node
            && e.source.event == input.1
            && e.target.event == 1_175_083_884
            && composition.nodes[usize::from(e.target.node)].identity == Identity::from(selector)
            && e.delivery == nfs_world::scoring::events::Delivery::Local
    }) {
        return Err(bad("movie selection requires a server event"));
    }
    for edge in &composition.connections {
        if usize::from(edge.source.node) != source_node || edge.source.event != input.1 {
            continue;
        }
        if edge.delivery != nfs_world::scoring::events::Delivery::Local {
            return Err(bad("movie play delivery"));
        }
        let target = &composition.nodes[usize::from(edge.target.node)];
        if target.class != "StateSequenceSelectorEntityData" {
            continue;
        }
        if edge.target.event != 283_106_254 {
            return Err(bad("movie sequence selection event"));
        }
        let site = &nodes
            .iter()
            .find(|n| Identity::from(&n.site) == target.identity)
            .ok_or_else(|| bad("movie selector occurrence"))?
            .site;
        if graph
            .object(site)?
            .fields
            .get("OverrideSequenceQueue")
            .and_then(Value::as_bool)
            != Some(true)
            || bound(
                graph,
                site,
                (-1_232_400_965i32) as u32,
                aliases,
                "ParticipantLogicParticipantProviderEntityData",
            )? != provider
        {
            return Err(bad("movie sequence owner/policy"));
        }
        let referer = bound(
            graph,
            site,
            (-621_692_900i32) as u32,
            aliases,
            "StateSequenceRefererEntityData",
        )?;
        let sequence = graph.target_field(&referer, "Sequence")?;
        exported(graph, &sequence, "ParticipantStateSequenceBlueprint")?;
        let provider = bound(
            graph,
            site,
            (-1_813_733_428i32) as u32,
            aliases,
            "StateSequenceProviderEntityData",
        )?;
        let state = sequence_state(graph, &provider)?;
        sequence_sites.push((sequence.0, referer, state));
    }
    let sequences: [(String, Site, Site); 2] = sequence_sites
        .try_into()
        .map_err(|_| bad("movie before/after count"))?;
    if sequences[0].2 == sequences[1].2 {
        return Err(bad("movie before/after share a state"));
    }
    let stable = endpoint(
        graph,
        unique(&nodes, "RaceObjectiveEntityData")?,
        stable_port,
        "StableUniqueIdEntityData",
    )?;
    let objective = match graph.object(&stable)?.fields.get("StableUniqueId64") {
        Some(Value::UInt64(n)) => *n,
        Some(Value::Int64(n)) => *n as u64,
        _ => return Err(bad("movie objective identity")),
    };
    if objective == 0 {
        return Err(bad("movie zero objective"));
    }
    Ok(Raw {
        objective,
        video,
        state,
        sequences,
    })
}

pub fn extract(index: &mut AssetIndex, capability: &str) -> Result<Movie, Error> {
    let progression = index.identity.progression.clone();
    let rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&rules.level)?;
    let videos = index.registered_objects(&progression.scene, "VideoRefererAsset")?;
    let bundle = startup
        .iter()
        .position(|e| e.name.eq_ignore_ascii_case(&progression.scene))
        .and_then(|i| u16::try_from(i + 1).ok())
        .ok_or_else(|| bad("movie bundle"))?;
    let mut graph = Graph::new(&mut index.assets);
    let mut scenes = SceneLookup {
        roots: scene_roots(&mut graph, &rules.level, &startup)?,
        rules: &rules,
        endpoints: BTreeMap::new(),
        loaded: BTreeSet::new(),
    };
    let branch = graph.read(Site::root(&progression.intro))?;
    let roots = branch
        .iter()
        .filter(|n| n.site.key == (capability.into(), 0))
        .collect::<Vec<_>>();
    let [root] = roots.as_slice() else {
        return Err(bad("movie capability occurrence"));
    };
    let scene = scenes
        .roots
        .get(&progression.scene)
        .ok_or_else(|| bad("movie progression scene"))?
        .clone();
    let scene_nodes = graph.read(scene)?;
    let mut spawners = vec![];
    for n in scene_nodes
        .iter()
        .filter(|n| n.object.class == "ParticipantLogicSpawnerEntityData")
    {
        if graph.target_field(&n.site, "ParticipantLogicBlueprint")?
            == (progression.intro.clone(), 0)
        {
            spawners.push(n.site.clone());
        }
    }
    let [spawner] = spawners.as_slice() else {
        return Err(bad("movie intro spawner"));
    };
    let references = graph.links(
        unique(&branch, "ParticipantLogicEntityData")?,
        progression.ports["root_channels"],
    )?;
    let targets = graph.links(spawner, progression.ports["spawner_channels"])?;
    let aliases = crate::participant_logic::bind(&mut graph, &branch, references, targets)?;
    let raw = read(
        &mut graph,
        root.site.clone(),
        &aliases,
        progression.ports["stable_id"],
    )?;
    let video = nfs_world::fmv::Video {
        bundle,
        index: u16::try_from(
            videos
                .iter()
                .position(|k| *k == raw.video)
                .ok_or_else(|| bad("movie not registered"))?,
        )
        .map_err(|_| bad("movie registry index"))?,
        count: u16::try_from(videos.len()).map_err(|_| bad("movie registry count"))?,
    };
    video.validate().map_err(|_| bad("movie registry range"))?;
    let mut sequence = |s: (String, Site, Site)| -> Result<Sequence, Error> {
        Ok(Sequence {
            blueprint: s.0,
            reference: Identity::from(&s.1),
            state: scenes.endpoint(&mut graph, &s.2)?,
        })
    };
    let [before, after] = raw.sequences;
    let before = sequence(before)?;
    let after = sequence(after)?;
    Ok(Movie {
        objective: raw.objective,
        blueprint: raw.video.0,
        video,
        state: scenes.endpoint(&mut graph, &raw.state)?,
        before,
        after,
    })
}

#[cfg(test)]
mod tests;
