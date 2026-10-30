// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed state predicates controlling scene-owned participant logic.
//! State-list/range leaves describe observer latches, not current membership
//! queries. The runtime must drive the shared participant-state property model.
use super::*;
use crate::events::{discovery::Endpoint as SceneEndpoint, lifecycle::bindings::BooleanOperation};
use nfs_world::participant_states::properties::Definition;

const CONTROLLER: u32 = 0xa707_ccbf;
const STATES: u32 = 0x9e22_81be;
const BEGIN: u32 = 0xff47_6cc8;
const END: u32 = 0x68d2_51b8;
const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 24;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Predicate<T = SceneEndpoint> {
    Property(Definition<T>),
    Boolean {
        operation: BooleanOperation,
        operands: Vec<Predicate<T>>,
    },
}
impl<T> Predicate<T> {
    pub fn try_map<U, E>(
        &self,
        map: &mut impl FnMut(&T) -> Result<U, E>,
    ) -> Result<Predicate<U>, E> {
        Ok(match self {
            Self::Property(definition) => Predicate::Property(definition.try_map(map)?),
            Self::Boolean {
                operation,
                operands,
            } => Predicate::Boolean {
                operation: *operation,
                operands: operands
                    .iter()
                    .map(|p| p.try_map(map))
                    .collect::<Result<_, _>>()?,
            },
        })
    }
}

fn one<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let sources = graph.links(site, port)?;
    let [source] = sources.as_slice() else {
        return Err(bad("activation source missing or ambiguous"));
    };
    if source.port != 0 {
        return Err(bad("activation source is not a local state predicate"));
    }
    Ok(source.site.clone())
}

fn states<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Vec<Site>, Error> {
    let sources = graph.links(site, port)?;
    if sources.len() > 256 {
        return Err(bad("activation state count bound"));
    }
    sources
        .into_iter()
        .map(|source| {
            if source.port != 0
                || !graph
                    .object(&source.site)?
                    .class
                    .ends_with("ParticipantStateEntityData")
            {
                return Err(bad("activation source is not a participant state"));
            }
            Ok(source.site)
        })
        .collect()
}

fn read<S: Source>(
    graph: &mut Graph<S>,
    site: Site,
    active: &mut BTreeSet<Site>,
    count: &mut usize,
) -> Result<Predicate<Site>, Error> {
    *count += 1;
    if *count > MAX_NODES || active.len() >= MAX_DEPTH || !active.insert(site.clone()) {
        return Err(bad("activation predicate bound or cycle"));
    }
    let object = graph.object(&site)?;
    let result = match object.class.as_str() {
        "ParticipantStateListBoolParticipantPropertyEntityData" => {
            Predicate::Property(Definition::List(states(graph, &site, STATES)?))
        }
        "ParticipantStateRangeBoolParticipantPropertyEntityData" => {
            let begins = states(graph, &site, BEGIN)?;
            let ends = states(graph, &site, END)?;
            let ([begin], [end]) = (begins.as_slice(), ends.as_slice()) else {
                return Err(bad("activation range endpoint count"));
            };
            Predicate::Property(Definition::Range {
                begin: begin.clone(),
                end: end.clone(),
            })
        }
        "BooleanOperationPredicateEntityData" => {
            let operation = match object.fields.get("Operation") {
                Some(Value::Enum {
                    value: 0,
                    name: Some(n),
                }) if n == "AND" => BooleanOperation::And,
                Some(Value::Enum {
                    value: 1,
                    name: Some(n),
                }) if n == "OR" => BooleanOperation::Or,
                Some(Value::Enum {
                    value: 2,
                    name: Some(n),
                }) if n == "NOT" => BooleanOperation::Not,
                _ => return Err(bad("activation Boolean operation")),
            };
            let Some(Value::Int32(arity)) = object.fields.get("NumberOfOperands") else {
                return Err(bad("activation operand count"));
            };
            if !(1..=32).contains(arity) || (operation == BooleanOperation::Not && *arity != 1) {
                return Err(bad("activation operand bound"));
            }
            let mut operands = Vec::new();
            for i in 0..*arity {
                let source = one(
                    graph,
                    &site,
                    nfs_world::scoring::integers::port("", i as u16),
                )?;
                operands.push(read(graph, source, active, count)?);
            }
            Predicate::Boolean {
                operation,
                operands,
            }
        }
        _ => {
            return Err(bad(&format!(
                "unsupported activation predicate {}",
                object.class
            )));
        }
    };
    active.remove(&site);
    Ok(result)
}

impl AssetIndex {
    /// Resolve one Boolean-controlled spawner in its real scene/prefab context.
    /// Unconnected, ambiguous or unsupported controllers are errors, not false.
    /// Set-controlled spawners intentionally require a separate model.
    pub fn participant_state_predicate(
        &mut self,
        scene: &str,
        blueprint: &str,
    ) -> Result<Predicate, Error> {
        if [scene, blueprint]
            .iter()
            .any(|v| v.is_empty() || v.len() > 512)
        {
            return Err(bad("activation asset name bound"));
        }
        let (scene, blueprint) = (scene.to_lowercase(), blueprint.to_lowercase());
        let scene_rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&scene_rules.level)?;
        let mut graph = Graph::new(&mut self.assets);
        let mut scenes = SceneLookup {
            roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
            rules: &scene_rules,
            endpoints: BTreeMap::new(),
            loaded: BTreeSet::new(),
        };
        let root = scenes
            .roots
            .get(&scene)
            .ok_or_else(|| bad("activation scene missing"))?
            .clone();
        let nodes = graph.read(root)?;
        let mut spawners = Vec::new();
        for node in nodes {
            if matches!(
                node.object.class.as_str(),
                "ParticipantLogicSpawnerEntityData"
                    | "ParticipantLogicSpawnerSetControlledEntityData"
            ) && graph.target_field(&node.site, "ParticipantLogicBlueprint")?
                == (blueprint.clone(), 0)
            {
                spawners.push(node);
            }
        }
        let [spawner] = spawners.as_slice() else {
            return Err(bad("activation spawner missing or ambiguous"));
        };
        if spawner.object.class != "ParticipantLogicSpawnerEntityData" {
            return Err(bad("activation spawner is set-controlled"));
        }
        let controller = one(&mut graph, &spawner.site, CONTROLLER)?;
        let predicate = read(&mut graph, controller, &mut BTreeSet::new(), &mut 0)?;
        predicate.try_map(&mut |site| {
            let (scene_key, serializer) = scenes.endpoint(&mut graph, site)?;
            Ok(SceneEndpoint {
                scene_key,
                serializer,
            })
        })
    }
}

#[cfg(test)]
mod tests;
