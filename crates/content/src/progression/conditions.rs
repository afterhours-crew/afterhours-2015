// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored mission-property conditions. Every input comes from the owning
//! account's committed state; sending a message is not proof it was read.
use crate::{Error, assets::AssetIndex, graph::*};
use nfs_frostbite::ebx::Value;
use std::collections::{BTreeMap, BTreeSet};

const ACTIVE: u32 = 1_010_522_579;
const COMPLETED: u32 = 1_514_830_046;
const READ: u32 = 1_056_530_460;
const OUT: u32 = 193_453_899;
const IN: u32 = 5_862_146;
const VALUE: u32 = 225_375_086;
const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 24;
const GAMEPLAY: &str = "levels/genesis01/gameplay";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Fact {
    Active(u64),
    Completed(u64),
    /// HashId in the resolved gameplay message manager's table.
    MessageRead {
        hash: u32,
        manager: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Op {
    Fact(Fact),
    Not(usize),
    All(Vec<usize>),
    Any(Vec<usize>),
}

/// Bounded postorder program, compiled from a graph or explicit fact conjunction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Condition {
    ops: Vec<Op>,
}
impl Condition {
    /// A bounded conjunction for callers constructing definitions directly.
    /// Fact identities still have to be resolved against their owning catalog.
    pub fn all(facts: &[Fact]) -> Result<Self, Error> {
        if facts.is_empty()
            || facts.len() >= MAX_NODES
            || facts.iter().collect::<BTreeSet<_>>().len() != facts.len()
            || facts
                .iter()
                .any(|f| matches!(f, Fact::Active(0) | Fact::Completed(0)))
        {
            return Err(bad("explicit mission condition fact bound or identity"));
        }
        let mut ops: Vec<_> = facts.iter().copied().map(Op::Fact).collect();
        ops.push(Op::All((0..facts.len()).collect()));
        Ok(Self { ops })
    }
    pub fn facts(&self) -> impl Iterator<Item = Fact> + '_ {
        self.ops.iter().filter_map(|op| match op {
            Op::Fact(f) => Some(*f),
            _ => None,
        })
    }
    /// The owner must distinguish a known absent/unread message (false) from
    /// unavailable account state (None). Validate even unselected OR branches.
    pub fn evaluate(&self, mut state: impl FnMut(Fact) -> Option<bool>) -> Result<bool, Error> {
        let mut values: Vec<bool> = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            values.push(match op {
                Op::Fact(f) => state(*f).ok_or_else(|| bad("mission condition state missing"))?,
                Op::Not(i) => !values[*i],
                Op::All(ids) => ids.iter().all(|i| values[*i]),
                Op::Any(ids) => ids.iter().any(|i| values[*i]),
            });
        }
        values
            .last()
            .copied()
            .ok_or_else(|| bad("empty mission condition"))
    }
}

struct Compiler<'a> {
    stable: u32,
    world: &'a [Node],
    managers: &'a BTreeMap<Key, usize>,
    active: BTreeSet<Endpoint>,
    ops: Vec<Op>,
}
impl Compiler<'_> {
    fn visit<S: Source>(
        &mut self,
        graph: &mut Graph<S>,
        endpoint: Endpoint,
    ) -> Result<usize, Error> {
        if self.ops.len() >= MAX_NODES
            || self.active.len() >= MAX_DEPTH
            || !self.active.insert(endpoint.clone())
        {
            return Err(bad("mission condition cycle or bound"));
        }
        let object = graph.object(&endpoint.site)?;
        let site = &endpoint.site;
        let op = match (object.class.as_str(), endpoint.port) {
            ("RaceObjectiveEntityData", ACTIVE | COMPLETED) => {
                let id = crate::chapter_notifications::stable(graph, self.stable, site)?;
                Op::Fact(if endpoint.port == ACTIVE {
                    Fact::Active(id)
                } else {
                    Fact::Completed(id)
                })
            }
            ("UniqueMessageSenderEntityData", READ) => {
                let key = graph.target_field(site, "MessageAsset")?;
                let message = graph.source.object(&key)?;
                if message.class != "MessageAsset" {
                    return Err(bad("mission sender asset class"));
                }
                let hash = bits32(
                    message
                        .fields
                        .get("HashId")
                        .ok_or_else(|| bad("mission message hash"))?,
                )?;
                let manager =
                    crate::messages::sender_manager(graph, site, self.world, self.managers)?;
                Op::Fact(Fact::MessageRead { hash, manager })
            }
            ("SyncedBoolEntityData" | "NotEntityData", OUT) => {
                if graph.input_ports(site)? != [IN] {
                    return Err(bad("mission unary input"));
                }
                // A synchronized Boolean with event writes is stateful and
                // cannot be reduced to its connected input expression.
                let bus = graph.object(&Site {
                    key: site.bus.clone(),
                    ..site.clone()
                })?;
                for row in bus
                    .fields
                    .get("EventConnections")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("mission event table"))?
                {
                    let fields = row.as_fields().ok_or_else(|| bad("mission event row"))?;
                    let target = fields
                        .get("Target")
                        .and_then(Value::as_pointer)
                        .ok_or_else(|| bad("mission event target"))?;
                    if graph.source.target(&site.bus.0, target)?.as_ref() == Some(&site.key) {
                        return Err(bad("mission condition has event-controlled state"));
                    }
                }
                let input = graph.input(site, IN)?;
                let index = self.visit(graph, input)?;
                if object.class == "SyncedBoolEntityData" {
                    self.active.remove(&endpoint);
                    return Ok(index);
                }
                Op::Not(index)
            }
            ("AndEntityData" | "OrEntityData", OUT) => {
                let count = object
                    .fields
                    .get("InputCount")
                    .and_then(Value::bits32)
                    .ok_or_else(|| bad("mission logic arity"))?;
                let ports = graph.input_ports(site)?;
                if !(2..=16).contains(&count)
                    || ports.len() != count as usize
                    || object.fields.get("Realm").and_then(Value::as_enum_name)
                        != Some("Realm_Server")
                {
                    return Err(bad("mission logic realm or arity"));
                }
                let mut inputs = Vec::new();
                for port in ports {
                    let input = graph.input(site, port)?;
                    inputs.push(self.visit(graph, input)?);
                }
                if object.class == "AndEntityData" {
                    Op::All(inputs)
                } else {
                    Op::Any(inputs)
                }
            }
            _ => {
                return Err(bad(&format!(
                    "unsupported mission condition {}:{}",
                    object.class, endpoint.port
                )));
            }
        };
        if self.ops.len() >= MAX_NODES {
            return Err(bad("mission condition node bound"));
        }
        let index = self.ops.len();
        self.ops.push(op);
        self.active.remove(&endpoint);
        Ok(index)
    }
}

impl AssetIndex {
    /// Resolve every selected mission Boolean writer, preserving its exact
    /// objective and message-manager dependencies. No unknown writer is false.
    pub fn mission_conditions(
        &mut self,
        identifiers: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, Condition>, Error> {
        if identifiers.is_empty()
            || identifiers.len() > 256
            || identifiers.iter().any(|id| id.is_empty() || id.len() > 512)
        {
            return Err(bad("mission condition selection bound"));
        }
        let rules = self.identity.progression.clone();
        let managers = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == "MessageManagerEntityData")
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(&rules.main))?;
        let world = graph.read(Site::root(GAMEPLAY))?;
        let mut out = BTreeMap::new();
        for node in nodes
            .iter()
            .filter(|n| n.object.class == "IdentifiedBoolAccessorEntityData")
        {
            let identifier = graph.target_field(&node.site, "BoolIdentifier")?.0;
            if !identifiers.contains(&identifier) {
                continue;
            }
            let input = graph.input(&node.site, VALUE)?;
            let condition = from_endpoint(
                &mut graph,
                input,
                rules.ports["stable_id"],
                &world,
                &managers,
            )?;
            if out.insert(identifier, condition).is_some() {
                return Err(bad("duplicate mission writer"));
            }
        }
        if out.keys().cloned().collect::<BTreeSet<_>>() != *identifiers {
            return Err(bad("selected mission writer missing"));
        }
        Ok(out)
    }
}

pub(crate) fn from_endpoint<S: Source>(
    graph: &mut Graph<S>,
    endpoint: Endpoint,
    stable: u32,
    world: &[Node],
    managers: &BTreeMap<Key, usize>,
) -> Result<Condition, Error> {
    let mut compiler = Compiler {
        stable,
        world,
        managers,
        active: BTreeSet::new(),
        ops: Vec::new(),
    };
    let last = compiler.visit(graph, endpoint)?;
    if last + 1 != compiler.ops.len() {
        return Err(bad("mission condition result"));
    }
    Ok(Condition { ops: compiler.ops })
}

#[cfg(test)]
mod tests;
