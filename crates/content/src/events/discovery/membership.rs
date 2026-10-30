// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Membership is evaluated for one participant. Preserve every authored set
//! operand; a mirrored set has its own state and is never its current input.
use super::*;
use crate::scoring::events::Identity;

const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 32;

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
}
impl Program {
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
            "ParticipantSetEntityData" => Op::Fact(Fact::Mutable(Identity::from(&site))),
            "IntPropertyWithinRangeParticipantSetEntityData" => {
                Op::Fact(Fact::IntegerRange(Identity::from(&site)))
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
    let mut compiler = Compiler {
        identified,
        active: BTreeSet::new(),
        ops: Vec::new(),
    };
    compiler.visit(g, root, endpoint)?;
    Ok(Program { ops: compiler.ops })
}

#[cfg(test)]
mod tests;
