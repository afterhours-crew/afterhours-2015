// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A compiled, per-realm scoring graph: node behaviors, typed cells, event
//! edges and the ordered startup sequence. The content crate builds it from the
//! installed composition; the interpreter executes it for one session.
//! A Program is data. It performs no I/O, awards nothing and encodes no packet.
use super::{
    Error, arithmetic, collision, delay, donut,
    events::{Connection, MAX_CONNECTIONS, MAX_NODES},
    math, nodes,
    properties::{DefaultMode, Definition, MAX_PROPERTIES, Port, Registration},
    property_cast, property_gate, road, synced,
    value::Value,
    vehicle_query,
};

pub const MAX_STEPS: usize = 65_536;
pub const MAX_KEEPERS: usize = 64;

/// Composition identity of one node occurrence. Repeated prefab instances
/// have distinct `instances` paths and therefore distinct nodes.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Identity {
    pub asset: String,
    pub index: u32,
    pub instances: Vec<(String, u32)>,
}

/// A keeper channel reference resolved at context activation by the runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Keeper {
    /// Channel reference node inside this composition.
    pub node: u16,
    /// Link ID declared by the channel asset; the scene resolves it to a keeper.
    pub field: u32,
}

/// What the interpreter does with a node. `Inert` nodes exist (their cells and
/// event forwarding are valid) but have no modeled behavior of their own.
/// `Unsupported` nodes are modeled as absent behavior with their outputs left
/// unready, so consumers fall back to authored values; they are logged.
#[derive(Clone, Debug, PartialEq)]
pub enum Behavior {
    Inert,
    Unsupported,
    Scalar(nodes::Definition),
    Math {
        program: math::Program,
        evaluate_on_event: bool,
    },
    Curve {
        curve: super::curve::Curve,
        apply: bool,
        default_input: f32,
    },
    Arithmetic {
        operations: Vec<arithmetic::Operation>,
        connected: Vec<u16>,
    },
    VectorLength {
        length_bound: bool,
        normal_bound: bool,
    },
    Fold {
        all: bool,
        input_count: u16,
        connected: Vec<u16>,
    },
    Cast(property_cast::Bindings),
    Gate {
        definition: property_gate::Definition,
        outputs: Vec<property_gate::Channel>,
    },
    Delay(delay::Definition),
    Stopwatch {
        time_bound: bool,
    },
    Synced(synced::Kind),
    Score {
        default: i32,
        context: u16,
    },
    ScoreContext {
        participant_provider: u16,
        keepers: Vec<Keeper>,
    },
    VehicleQuery(vehicle_query::Bindings),
    Road {
        configuration: road::Configuration,
        limit_bound: bool,
    },
    /// Participant-proximity count from the other cars (E916).
    Proximity,
    /// Vehicle figures from the owned inputs (E916).
    Stats,
    /// Nitrous query from the owned inputs (E916).
    Nos,
    /// Doughnut detection from the owned motion (E916).
    Donut(donut::Configuration),
    Collision(collision::Outputs),
    NearMiss,
    AccumulatedCollisions,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub class: String,
    /// Created in this realm. Nodes outside it keep their index but never run.
    pub created: bool,
    /// Entity event-listener realm includes this realm.
    pub events: bool,
    /// Entity property-listener realm includes this realm.
    pub properties: bool,
    /// Prefab boundary: events received here are re-emitted at that node.
    pub forward: Option<u16>,
    /// Authored input fallbacks, used when a port is unbound or not ready.
    pub defaults: Vec<(u32, Value)>,
    pub behavior: Behavior,
}

/// One startup action. The compiler emits the verified native order:
/// bus construction (forwarders, interface defaults), then per entity bind and
/// listener registration, then activation in insertion order.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Bind(u16),
    Register(Registration),
    /// Forwarder construction copies an already ready source into its merge
    /// destination once; later changes arrive through the registration.
    CopyReady {
        source: Port,
        target: Port,
    },
    Default {
        port: Port,
        value: Value,
        mode: DefaultMode,
    },
    Activate(u16),
}

/// A property endpoint that has no typed cell in this program because its
/// value kind is not modeled. Only sink-only endpoints may be unmodeled; the
/// compiler rejects an untyped cell that any modeled node reads or writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unmodeled {
    pub port: Port,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub nodes: Vec<Node>,
    pub cells: Vec<Definition>,
    pub connections: Vec<Connection>,
    pub startup: Vec<Step>,
    pub unmodeled: Vec<Unmodeled>,
}

impl Program {
    /// Validate bounds and internal references. This does not prove native
    /// equivalence; it rejects programs the interpreter cannot execute safely.
    pub fn validate(&self) -> Result<(), Error> {
        let count = self.nodes.len();
        if count == 0
            || count > MAX_NODES
            || self.cells.len() > MAX_PROPERTIES
            || self.connections.len() > MAX_CONNECTIONS
            || self.startup.len() > MAX_STEPS
            || self.unmodeled.len() > MAX_PROPERTIES
        {
            return Err(Error::Bound);
        }
        let node = |id: u16| -> Result<&Node, Error> {
            self.nodes.get(usize::from(id)).ok_or(Error::UnknownNode)
        };
        for (index, n) in self.nodes.iter().enumerate() {
            if let Some(forward) = n.forward
                && (usize::from(forward) == index || node(forward)?.forward != Some(index as u16))
            {
                return Err(Error::InvalidGraph);
            }
            if n.defaults.len() > MAX_PROPERTIES {
                return Err(Error::Bound);
            }
            match &n.behavior {
                Behavior::Score { context, .. } => {
                    if !matches!(node(*context)?.behavior, Behavior::ScoreContext { .. }) {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::ScoreContext {
                    participant_provider,
                    keepers,
                } => {
                    node(*participant_provider)?;
                    if keepers.len() > MAX_KEEPERS {
                        return Err(Error::Bound);
                    }
                    for keeper in keepers {
                        node(keeper.node)?;
                    }
                }
                Behavior::Arithmetic {
                    operations,
                    connected,
                } => {
                    arithmetic::Node::new(operations.clone(), connected)?;
                }
                Behavior::Fold {
                    all,
                    input_count,
                    connected,
                } => {
                    super::boolean_fold::Node::new(*all, *input_count, connected)?;
                }
                Behavior::Gate {
                    definition,
                    outputs,
                } => {
                    property_gate::Gate::new(*definition, outputs)?;
                }
                Behavior::Road { configuration, .. } => configuration.validate()?,
                Behavior::Donut(configuration) => configuration.validate()?,
                Behavior::Scalar(definition) => {
                    nodes::Node::new(*definition)?;
                }
                _ => {}
            }
        }
        for step in &self.startup {
            match step {
                Step::Bind(id) | Step::Activate(id) => {
                    if !node(*id)?.created {
                        return Err(Error::InvalidGraph);
                    }
                }
                Step::Register(Registration::Notify(port)) | Step::Default { port, .. } => {
                    node(port.node)?;
                }
                Step::Register(Registration::Forward { source, target })
                | Step::CopyReady { source, target } => {
                    node(source.node)?;
                    node(target.node)?;
                }
            }
        }
        for cell in &self.cells {
            for port in cell.ports.iter().chain(&cell.listeners) {
                node(port.node)?;
            }
        }
        Ok(())
    }

    pub fn node(&self, id: u16) -> Result<&Node, Error> {
        self.nodes.get(usize::from(id)).ok_or(Error::UnknownNode)
    }

    /// Authored fallback for one input port, if the class declares one.
    pub fn default(&self, port: Port) -> Result<Option<Value>, Error> {
        Ok(self
            .node(port.node)?
            .defaults
            .iter()
            .find(|(field, _)| *field == port.field)
            .map(|(_, value)| *value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(index: u32) -> Identity {
        Identity {
            asset: "test".into(),
            index,
            instances: Vec::new(),
        }
    }
    fn node(index: u32, behavior: Behavior) -> Node {
        Node {
            identity: identity(index),
            class: "Test".into(),
            created: true,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior,
        }
    }

    #[test]
    fn validation_rejects_dangling_references_and_accepts_consistent_programs() {
        let mut program = Program {
            nodes: vec![
                node(0, Behavior::Inert),
                node(
                    1,
                    Behavior::Score {
                        default: 100,
                        context: 2,
                    },
                ),
                node(
                    2,
                    Behavior::ScoreContext {
                        participant_provider: 0,
                        keepers: vec![Keeper { node: 0, field: 7 }],
                    },
                ),
            ],
            cells: vec![],
            connections: vec![],
            startup: vec![Step::Bind(1), Step::Activate(2)],
            unmodeled: vec![],
        };
        program.validate().unwrap();
        program.startup.push(Step::Activate(9));
        assert_eq!(program.validate(), Err(Error::UnknownNode));
        program.startup.pop();
        program.nodes[1].behavior = Behavior::Score {
            default: 0,
            context: 0,
        };
        assert_eq!(program.validate(), Err(Error::InvalidGraph));
        program.nodes[1].behavior = Behavior::Inert;
        program.nodes[0].forward = Some(1);
        assert_eq!(program.validate(), Err(Error::InvalidGraph));
        program.nodes[1].forward = Some(0);
        program.validate().unwrap();
        program.nodes[2].created = false;
        assert_eq!(program.validate(), Err(Error::InvalidGraph));
        assert_eq!(
            Program {
                nodes: vec![],
                cells: vec![],
                connections: vec![],
                startup: vec![],
                unmodeled: vec![]
            }
            .validate(),
            Err(Error::Bound)
        );
    }

    #[test]
    fn defaults_are_looked_up_per_node_and_port() {
        let mut n = node(0, Behavior::Inert);
        n.defaults.push((5, Value::Float(2.5)));
        let program = Program {
            nodes: vec![n],
            cells: vec![],
            connections: vec![],
            startup: vec![],
            unmodeled: vec![],
        };
        assert_eq!(
            program.default(Port { node: 0, field: 5 }).unwrap(),
            Some(Value::Float(2.5))
        );
        assert_eq!(program.default(Port { node: 0, field: 6 }).unwrap(), None);
        assert_eq!(
            program.default(Port { node: 1, field: 5 }),
            Err(Error::UnknownNode)
        );
    }
}
