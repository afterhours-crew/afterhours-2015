// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Membership is evaluated for one participant. Preserve every authored set
//! operand; a mirrored set has its own state and is never its current input.
use super::*;
use crate::scoring::events::Identity;

const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 32;
mod constant;
pub mod copies;
mod owned;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Fact {
    Boolean(String),
    Player(Identity),
    States(Vec<Endpoint>),
    Copy(Identity),
    /// Explicitly mutable membership, maintained by its event owner.
    Mutable(Identity),
    /// Membership computed by the integer-range owner. Bounds can be driven
    /// by event-detail properties; the node's static defaults are insufficient.
    IntegerRange(Identity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Difference,
    Union,
    Intersection,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Op {
    Fact(Fact),
    Combine(Operation, usize, usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    ops: Vec<Op>,
    ranges: BTreeMap<Identity, IntegerRange>,
    empty_sets: BTreeSet<Identity>,
}

/// Authored bounds plus the exact property that supplies the participant's
/// integer. Linked relays follow their own definition, never a schema-name
/// search. Static bounds alone never imply participant membership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegerRange {
    pub input: Identity,
    pub input_port: u32,
    pub minimum: i32,
    pub maximum: i32,
}

impl IntegerRange {
    /// Signed, inclusive bounds. An inverted interval contains no value.
    pub fn contains(&self, value: i32) -> bool {
        self.minimum <= value && value <= self.maximum
    }
}

impl Program {
    /// Owned sets whose constructor is empty and whose complete consumer graph
    /// contains only read-only predicates. Other mutable facts still need an
    /// explicit runtime owner; this does not fill missing state with false.
    pub fn empty_sets(&self) -> &BTreeSet<Identity> {
        &self.empty_sets
    }
    pub fn ranges(&self) -> &BTreeMap<Identity, IntegerRange> {
        &self.ranges
    }
    pub fn facts(&self) -> impl Iterator<Item = &Fact> {
        self.ops.iter().filter_map(|op| match op {
            Op::Fact(f) => Some(f),
            _ => None,
        })
    }
    pub fn evaluate(&self, mut state: impl FnMut(&Fact) -> Option<bool>) -> Result<bool, Error> {
        let mut values: Vec<bool> = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            values.push(match op {
                Op::Fact(f) => state(f).ok_or_else(|| bad("event membership state missing"))?,
                Op::Combine(Operation::Difference, a, b) => values[*a] && !values[*b],
                Op::Combine(Operation::Union, a, b) => values[*a] || values[*b],
                Op::Combine(Operation::Intersection, a, b) => values[*a] && values[*b],
            });
        }
        values
            .last()
            .copied()
            .ok_or_else(|| bad("empty event membership"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub notification: Endpoint,
    pub program: Program,
}

fn link<S: Source>(g: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let links = g.links(site, port)?;
    match links.as_slice() {
        [target] if target.port == 0 => Ok(target.site.clone()),
        _ => Err(bad("event set link missing or ambiguous")),
    }
}

struct Compiler<'a> {
    identified: &'a BTreeMap<Site, String>,
    active: BTreeSet<Site>,
    ops: Vec<Op>,
    ranges: BTreeMap<Identity, IntegerRange>,
    empty_sets: BTreeSet<Identity>,
}
impl Compiler<'_> {
    fn visit<S: Source>(
        &mut self,
        g: &mut Graph<S>,
        site: Site,
        endpoint: &mut impl FnMut(&mut Graph<S>, &Site) -> Result<Endpoint, Error>,
    ) -> Result<usize, Error> {
        if self.ops.len() >= MAX_NODES
            || self.active.len() >= MAX_DEPTH
            || !self.active.insert(site.clone())
        {
            return Err(bad("event set cycle or bound"));
        }
        let object = g.object(&site)?;
        let op = match object.class.as_str() {
            "ParticipantSetPredicateEntityData" => {
                if object
                    .fields
                    .get("NumberOfOperands")
                    .and_then(Value::bits32)
                    != Some(2)
                {
                    return Err(bad("event set operand count"));
                }
                let operation = match object.fields.get("Operation").and_then(Value::as_enum_name) {
                    Some("DIF") => Operation::Difference,
                    Some("UNION") => Operation::Union,
                    Some("INTERSECTION") => Operation::Intersection,
                    _ => return Err(bad("event set operation")),
                };
                let a = link(g, &site, name("0"))?;
                let b = link(g, &site, name("1"))?;
                let a = self.visit(g, a, endpoint)?;
                let b = self.visit(g, b, endpoint)?;
                Op::Combine(operation, a, b)
            }
            "ParticipantSetFromBPPEntityData" => {
                let property = link(g, &site, 2_093_602_801)?;
                if g.object(&property)?.class != "BoolParticipantPropertyEntityData" {
                    return Err(bad("event set Boolean class"));
                }
                Op::Fact(Fact::Boolean(
                    self.identified
                        .get(&property)
                        .cloned()
                        .ok_or_else(|| bad("event set Boolean identifier missing"))?,
                ))
            }
            "ParticipantSetPlayerAdapterEntityData" => {
                let manager = link(g, &site, name("ParticipantManager"))?;
                if g.object(&manager)?.class != "ParticipantManagerEntityData" {
                    return Err(bad("event set player manager class"));
                }
                Op::Fact(Fact::Player(Identity::from(&manager)))
            }
            "ParticipantSetStateAdapterEntityData" => {
                let links = g.links(&site, name("ParticipantStates"))?;
                if links.is_empty() || links.len() > 64 {
                    return Err(bad("event set state bound"));
                }
                let mut states = BTreeSet::new();
                for target in links {
                    if target.port != 0
                        || !matches!(
                            g.object(&target.site)?.class.as_str(),
                            "ParticipantStateEntityData"
                                | "TeleportingParticipantStateEntityData"
                                | "StateSequenceParticipantStateEntityData"
                        )
                        || !states.insert(endpoint(g, &target.site)?)
                    {
                        return Err(bad("event set state class or duplicate"));
                    }
                }
                Op::Fact(Fact::States(states.into_iter().collect()))
            }
            "ParticipantSetCopyEntityData" => Op::Fact(Fact::Copy(Identity::from(&site))),
            "ParticipantSetEntityData" => {
                let id = Identity::from(&site);
                if owned::is_read_only(g, &site)? {
                    self.empty_sets.insert(id.clone());
                }
                Op::Fact(Fact::Mutable(id))
            }
            "IntPropertyWithinRangeParticipantSetEntityData" => {
                let id = Identity::from(&site);
                let range = integer_range(g, &site)?;
                if self
                    .ranges
                    .insert(id.clone(), range.clone())
                    .is_some_and(|old| old != range)
                {
                    return Err(bad("inconsistent event range owner"));
                }
                Op::Fact(Fact::IntegerRange(id))
            }
            _ => {
                return Err(bad(&format!(
                    "unsupported event set {}:{} {}",
                    site.key.0, site.key.1, object.class
                )));
            }
        };
        if self.ops.len() >= MAX_NODES {
            return Err(bad("event set program bound"));
        }
        let index = self.ops.len();
        self.ops.push(op);
        self.active.remove(&site);
        Ok(index)
    }
}

fn integer_range<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<IntegerRange, Error> {
    constant::no_events(g, site)?;
    let ports = g.input_ports(site)?;
    if ports
        .iter()
        .any(|p| *p != name("MinValue") && *p != name("MaxValue"))
    {
        return Err(bad("event range unexpected property input"));
    }
    for port in [name("MinValue"), name("MaxValue")] {
        constant::inputs(g, site, port)?;
    }
    let inputs = g.links(site, (-620_217_377i32) as u32)?;
    let [input] = inputs.as_slice() else {
        return Err(bad("event range property link"));
    };
    let input = range_input(g, input.clone(), &mut BTreeSet::new())?;
    let class = g.object(&input.site)?.class;
    if (class.as_str(), input.port) != ("IntParticipantPropertyEntityData", 0) {
        return Err(bad("event range property owner"));
    }
    let mut evaluator = Evaluator::new(g);
    let Number::Integer(minimum) = evaluator.field_or_input(site, name("MinValue"), "MinValue")?
    else {
        return Err(bad("event range minimum type"));
    };
    let Number::Integer(maximum) = evaluator.field_or_input(site, name("MaxValue"), "MaxValue")?
    else {
        return Err(bad("event range maximum type"));
    };
    // An inverted authored interval is empty; do not swap its endpoints or
    // substitute the node defaults for a connected threshold.
    Ok(IntegerRange {
        input: Identity::from(&input.site),
        input_port: input.port,
        minimum,
        maximum,
    })
}

// A linked relay resolves its actual definition through the authored source
// link. A matching channel asset elsewhere in a scene is not that binding.
fn range_input<S: Source>(
    g: &mut Graph<S>,
    input: crate::graph::Endpoint,
    active: &mut BTreeSet<crate::graph::Endpoint>,
) -> Result<crate::graph::Endpoint, Error> {
    if active.len() >= MAX_DEPTH || !active.insert(input.clone()) {
        return Err(bad("event range channel cycle or bound"));
    }
    if g.object(&input.site)?.class != "LinkedSchematicChannelRelayEntityData" {
        return Ok(input);
    }
    let sources = g.links(&input.site, 0x2780_2951)?;
    let [source] = sources.as_slice() else {
        return Err(bad("event range relay source missing or ambiguous"));
    };
    if source.port != 0
        || g.object(&source.site)?.class != "LinkedSchematicChannelEntityData"
        || g.target_field(&source.site, "ChannelAsset")?
            != g.target_field(&input.site, "ChannelAsset")?
    {
        return Err(bad("event range relay source class or schema"));
    }
    let targets = g.links(&source.site, input.port)?;
    let [target] = targets.as_slice() else {
        return Err(bad("event range channel target missing or ambiguous"));
    };
    range_input(g, target.clone(), active)
}

pub(super) fn read<S: Source>(
    g: &mut Graph<S>,
    notification: &Site,
    identified: &BTreeMap<Site, String>,
    endpoint: &mut impl FnMut(&mut Graph<S>, &Site) -> Result<Endpoint, Error>,
) -> Result<Program, Error> {
    if g.object(notification)?.class != NOTIFICATION {
        return Err(bad("event membership notification class"));
    }
    let root = link(g, notification, 2_664_990_019)?;
    read_set(g, root, identified, endpoint)
}

fn read_set<S: Source>(
    g: &mut Graph<S>,
    root: Site,
    identified: &BTreeMap<Site, String>,
    endpoint: &mut impl FnMut(&mut Graph<S>, &Site) -> Result<Endpoint, Error>,
) -> Result<Program, Error> {
    let mut compiler = Compiler {
        identified,
        active: BTreeSet::new(),
        ops: Vec::new(),
        ranges: BTreeMap::new(),
        empty_sets: BTreeSet::new(),
    };
    compiler.visit(g, root, endpoint)?;
    Ok(Program {
        ops: compiler.ops,
        ranges: compiler.ranges,
        empty_sets: compiler.empty_sets,
    })
}

#[cfg(test)]
mod tests;
