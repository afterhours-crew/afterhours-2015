// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AI scoring uses the selected scene spawner's sets and services. Identical
//! channel schemas elsewhere do not provide that instance's dependencies.
use super::{
    content::Participant,
    events::Identity,
    participant_pacing,
    participant_properties::{self, Property},
};
use crate::{
    Error,
    assets::AssetIndex,
    events::discovery::{Endpoint as SceneEndpoint, membership as sets},
    graph::{
        Graph, Site, Source, bad,
        scenes::{SceneLookup, scene_roots},
    },
    participant_logic,
};
use nfs_world::scoring::participant_pacing::{Library, MAX_PERSONALITIES, Sources};
use nfs_world::scoring::{program::Behavior, properties::Kind};
use std::collections::{BTreeMap, BTreeSet};

pub mod membership;

fn identity_valid(id: &Identity) -> bool {
    !id.asset.is_empty()
        && id.asset.len() <= 512
        && id.instances.len() <= crate::scenes::MAX_GRAPH_DEPTH
        && id
            .instances
            .iter()
            .all(|(asset, _)| !asset.is_empty() && asset.len() <= 512)
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pacing {
    pub sources: Sources,
    pub manager: Identity,
    pub actors: SceneEndpoint,
    pub personalities: Identity,
    pub curves: Identity,
    pub library: Library,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub participant: Participant,
    pub eligibility: sets::Program,
    pub membership: Vec<membership::Binding>,
    pub pacing: Vec<Pacing>,
    pub boolean_sets: Vec<BooleanSet>,
}

/// A query set whose membership is the linked Boolean participant property.
/// The property and exported set retain their channel identities independently.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanSet {
    pub property: Property,
    pub set: Property,
    pub definition: Identity,
}

impl Deployment {
    /// Restore compiler invariants before publishing cached or supplied data.
    /// This checks binding completeness, not the host's live membership facts.
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = || bad("AI scoring deployment invariants");
        self.participant.program.validate().map_err(|_| invalid())?;
        membership::validate(self)?;
        let bound = super::participant_properties::MAX_ACCESSORS;
        if self.pacing.len() > bound || self.boolean_sets.len() > bound {
            return Err(invalid());
        }
        let mut pacing = BTreeSet::new();
        for binding in &self.pacing {
            binding.sources.validate().map_err(|_| invalid())?;
            binding.library.validate().map_err(|_| invalid())?;
            if !pacing.insert(&binding.sources)
                || binding.library.personalities.is_empty()
                || binding.actors.scene_key == 0
                || [&binding.manager, &binding.personalities, &binding.curves]
                    .into_iter()
                    .any(|id| !identity_valid(id))
            {
                return Err(invalid());
            }
        }
        let mut properties = BTreeSet::new();
        let mut sets = BTreeSet::new();
        for binding in &self.boolean_sets {
            binding.property.validate().map_err(|_| invalid())?;
            binding.set.validate().map_err(|_| invalid())?;
            if !identity_valid(&binding.definition) || !sets.insert(&binding.set) {
                return Err(invalid());
            }
            properties.insert(&binding.property);
        }
        let mut required_pacing = BTreeSet::new();
        let mut required_properties = BTreeSet::new();
        for node in &self.participant.program.nodes {
            match &node.behavior {
                Behavior::ParticipantPacing { sources, .. } => {
                    required_pacing.insert(sources);
                }
                Behavior::ParticipantProperty {
                    property,
                    kind: Kind::Boolean,
                    ..
                } => {
                    required_properties.insert(property);
                }
                _ => {}
            }
        }
        if pacing != required_pacing || properties != required_properties {
            return Err(invalid());
        }
        Ok(())
    }
}

fn boolean_sets<S: Source>(
    graph: &mut Graph<S>,
    branch: &[crate::graph::Node],
    aliases: &BTreeMap<Site, Site>,
) -> Result<Vec<BooleanSet>, Error> {
    let mut result = BTreeSet::new();
    for node in branch
        .iter()
        .filter(|n| n.object.class == "BoolParticipantPropertyAccessorEntityData")
    {
        let port = participant_properties::accessor(&node.object.class)
            .unwrap()
            .0;
        let property = participant_properties::identifier(graph, &node.site, port)?;
        let definition = one(
            graph,
            &node.site,
            port,
            aliases,
            "BoolParticipantPropertyEntityData",
        )?;
        let mut found = false;
        for (reference, channel) in aliases {
            for port in graph.link_ports(channel)? {
                let targets = graph.links(channel, port)?;
                for target in &targets {
                    if target.port != 0
                        || graph.object(&target.site)?.class
                            != "ParticipantSetBoolPropertyAdapterEntityData"
                    {
                        continue;
                    }
                    if targets.len() != 1 {
                        return Err(bad("AI scoring query set export is ambiguous"));
                    }
                    let input = one(
                        graph,
                        &target.site,
                        0x969a_1637,
                        aliases,
                        "BoolParticipantPropertyEntityData",
                    )?;
                    if input != definition {
                        continue;
                    }
                    let set = participant_properties::channel_endpoint(
                        graph,
                        &crate::graph::Endpoint {
                            site: reference.clone(),
                            port,
                        },
                    )?;
                    result.insert(BooleanSet {
                        property: property.clone(),
                        set,
                        definition: Identity::from(&target.site),
                    });
                    found = true;
                }
            }
        }
        if !found {
            return Err(bad("AI scoring Boolean property has no bound query set"));
        }
    }
    Ok(result.into_iter().collect())
}

fn one<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    aliases: &BTreeMap<Site, Site>,
    class: &str,
) -> Result<Site, Error> {
    let targets = participant_logic::links(graph, site, port, aliases, &mut BTreeSet::new())?;
    let [target] = targets.as_slice() else {
        return Err(bad("AI scoring service missing or ambiguous"));
    };
    if target.port != 0 || graph.object(&target.site)?.class != class {
        return Err(bad(&format!("AI scoring service requires {class}")));
    }
    Ok(target.site.clone())
}

fn library<S: Source>(
    graph: &mut Graph<S>,
    personalities: &Site,
    curves: &Site,
    port: u32,
) -> Result<Library, Error> {
    let links = graph.links(personalities, port)?;
    if links.is_empty() || links.len() > MAX_PERSONALITIES {
        return Err(bad("AI scoring personality library bound"));
    }
    let mut seen = BTreeSet::new();
    let objects = links
        .into_iter()
        .map(|target| {
            if target.port != 0 || !seen.insert(target.site.key.clone()) {
                return Err(bad("AI scoring personality identity"));
            }
            graph.object(&target.site)
        })
        .collect::<Result<Vec<_>, _>>()?;
    participant_pacing::library(&objects, Some(&graph.object(curves)?))
}

impl AssetIndex {
    /// Compile the AI graph together with its exact set-controlled lifetime,
    /// actor manager and ordered personality/pacing libraries. Runtime owners
    /// must supply every membership fact and successful pacing lookup.
    pub fn ai_scoring_deployment(&mut self, scene: &str) -> Result<Deployment, Error> {
        if scene.is_empty() || scene.len() > 512 {
            return Err(bad("AI scoring scene name bound"));
        }
        let scene = scene.to_lowercase();
        let blueprint = super::content::AI_SCORING;
        let participant = self.ai_scoring_participant(&scene)?;
        let accessors = participant_pacing::read(&mut self.assets, blueprint)?;
        let rules = self.identity.scenes.clone();
        let participant_rules = self.identity.participant_logic.clone();
        let personalities_port = self.identity.rival_links.personalities;
        let startup = self.startup_entries(&rules.level)?;
        let mut graph = Graph::new(&mut self.assets);
        let roots = scene_roots(&mut graph, &rules.level, &startup)?;
        let root = roots
            .get(&scene)
            .cloned()
            .ok_or_else(|| bad("AI scoring scene not loaded"))?;
        let participant_logic::SpawnerContext {
            branch,
            aliases,
            spawner,
        } = participant_logic::spawner_context(
            &mut graph,
            root,
            blueprint,
            "ParticipantLogicSpawnerSetControlledEntityData",
            participant_rules.root_channels,
            participant_rules.spawner_channels,
        )?;
        let mut lookup = SceneLookup {
            roots,
            rules: &rules,
            endpoints: BTreeMap::new(),
            loaded: BTreeSet::new(),
        };
        let controllers = graph.links(&spawner, 0xb075_bd0b)?;
        let [controller] = controllers.as_slice() else {
            return Err(bad("AI scoring set controller missing or ambiguous"));
        };
        if controller.port != 0 {
            return Err(bad("AI scoring set controller port"));
        }
        let resolved = sets::read_set_sources(
            &mut graph,
            controller.site.clone(),
            &BTreeMap::new(),
            &mut |graph, site| {
                let (scene_key, serializer) = lookup.endpoint(graph, site)?;
                Ok(SceneEndpoint {
                    scene_key,
                    serializer,
                })
            },
        )?;
        let membership = membership::bind(&mut graph, &resolved, &mut |graph, site| {
            let (scene_key, serializer) = lookup.endpoint(graph, site)?;
            Ok(SceneEndpoint {
                scene_key,
                serializer,
            })
        })?;
        let mut pacing = Vec::new();
        for accessor in accessors {
            let node = branch
                .iter()
                .find(|n| Identity::from(&n.site) == accessor.identity)
                .ok_or_else(|| bad("AI scoring pacing accessor occurrence"))?;
            let manager = one(
                &mut graph,
                &node.site,
                0x7eef_cb0c,
                &aliases,
                "AiPacingManagerEntityData",
            )?;
            let actors = one(
                &mut graph,
                &node.site,
                0x2824_4ad5,
                &aliases,
                "AiActorManagerEntityData",
            )?;
            let personalities = one(
                &mut graph,
                &node.site,
                0xb57a_a3e6,
                &aliases,
                "PersonalityLibraryEntityData",
            )?;
            let curves = one(
                &mut graph,
                &node.site,
                0xa663_9088,
                &aliases,
                "PacingLibraryEntityData",
            )?;
            let library = library(&mut graph, &personalities, &curves, personalities_port)?;
            let (scene_key, serializer) = lookup.endpoint(&mut graph, &actors)?;
            pacing.push(Pacing {
                sources: accessor.sources,
                manager: Identity::from(&manager),
                actors: SceneEndpoint {
                    scene_key,
                    serializer,
                },
                personalities: Identity::from(&personalities),
                curves: Identity::from(&curves),
                library,
            });
        }
        let boolean_sets = boolean_sets(&mut graph, &branch, &aliases)?;
        let deployment = Deployment {
            participant,
            eligibility: resolved.program,
            membership,
            pacing,
            boolean_sets,
        };
        deployment.validate()?;
        Ok(deployment)
    }
}

#[cfg(test)]
mod tests;
