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
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub asset: String,
    pub index: u32,
    pub instances: Vec<(String, u32)>,
}

/// A keeper channel reference resolved at context activation by the runtime.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Behavior {
    /// Authored node implemented by the enclosing native event host. Executing
    /// it without that host fails; it is never treated as inert/unsupported.
    Native,
    Ai(super::ai_commands::Definition<u16>),
    /// Constructor-owned property. StringEntity uses field c5e4e950 (037ac250).
    Constant {
        field: u32,
        value: Value,
    },
    FloatSelect,
    ScaleClamp,
    RandomFloat {
        true_random: bool,
    },
    Burnout(super::burnout::Configuration),
    VehicleInfo,
    ItemInfo {
        source: super::participant_properties::Property,
        provider: u16,
    },
    /// Source is verified as this logic's own controllable provider by content.
    BurnoutOverride,
    /// Activation-time GameId output (04620700); absent host game publishes nothing.
    GameId,
    EventDetails {
        source: Option<super::participant_properties::Property>,
    },
    /// Native participant-name RPC. Server CString dataflow remains unmodeled.
    ParticipantName,
    ContextReporter {
        source: super::participant_properties::Property,
        provider: u16,
        categories: Vec<u32>,
    },
    VehicleTuning {
        source: super::participant_properties::Property,
        provider: u16,
    },
    TeleportLocation {
        provider: u16,
        location: super::participant_properties::Property,
        manager: super::participant_properties::Property,
    },
    ParticipantEventChannel,
    ParticipantTransition {
        provider: u16,
        sources: Vec<super::participant_properties::Property>,
        target: Option<super::participant_properties::Property>,
    },
    TriggeredTransition {
        sources: Vec<super::participant_properties::Property>,
        target: Option<super::participant_properties::Property>,
    },
    ParticipantRankQuery {
        sorter: super::participant_properties::Property,
    },
    Bank {
        bank: super::participant_properties::Property,
        provider: u16,
        kind: super::bank::Kind,
    },
    Inert,
    Unsupported,
    /// Server EventSync has no local input behavior; its validated registry-82
    /// receiver emits Out through Session::event_sync.
    EventSync,
    ParticipantSetAccessor {
        set: Option<super::participant_sets::Key>,
        provider: Option<u16>,
    },
    ParticipantSetAdapter {
        source: Option<super::participant_sets::Key>,
        provider: Option<u16>,
    },
    ParticipantSetPredicate {
        operation: super::participant_sets::Operation,
        operands: Vec<super::participant_sets::Source>,
    },
    ParticipantSetCounter {
        source: Option<super::participant_sets::Source>,
    },
    ParticipantProvider,
    ParticipantEvent {
        provider: Option<u16>,
        filter: bool,
    },
    /// An owned snapshot of external property values, supplied by the enclosing
    /// session before construction. Published at binding in native node order.
    Snapshot(Vec<(u32, Value)>),
    /// Live accessor to this session's participant store. A connected Value
    /// writes at activation and on change; an unconnected Value only reads.
    ParticipantProperty {
        property: super::participant_properties::Property,
        kind: super::properties::Kind,
        write: bool,
    },
    ParticipantRank {
        property: super::participant_properties::Property,
        descending: bool,
    },
    IntegerSum {
        count: u16,
    },
    IntegerArithmetic {
        operations: Vec<super::integers::Operation>,
    },
    VectorDot,
    /// Absolute monotonic race clock, rounded to f32, sampled every frame.
    RaceTime,
    TransformSplitter,
    IntegerHub {
        count: u16,
    },
    FloatHub {
        count: u16,
    },
    FloatSwitch(super::float_switch::Definition),
    RankIntegers {
        count: u16,
        on_change: bool,
    },
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
    /// Native 04032540 binds the linked curve; only an input property callback
    /// evaluates it (04032f80). Binding and activation do not publish a value.
    LinkedCurve {
        source: super::participant_properties::Property,
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
    NosBuildup,
    /// Doughnut detection from the owned motion (E916).
    Donut(donut::Configuration),
    Collision(collision::Outputs),
    NearMiss,
    AccumulatedCollisions,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Unmodeled {
    pub port: Port,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Program {
    pub nodes: Vec<Node>,
    pub cells: Vec<Definition>,
    pub connections: Vec<Connection>,
    pub startup: Vec<Step>,
    pub unmodeled: Vec<Unmodeled>,
}

impl Program {
    fn validate_set_source(
        &self,
        source: &super::participant_sets::Source,
        path: &mut Vec<u16>,
    ) -> Result<(), Error> {
        use super::participant_sets::Source;
        match source {
            Source::Channel(key) => key.validate(),
            Source::Node(id) => {
                if path.len() >= 32 || path.contains(id) {
                    return Err(Error::InvalidGraph);
                }
                path.push(*id);
                let n = self.node(*id)?;
                if !n.created {
                    return Err(Error::InvalidGraph);
                }
                match &n.behavior {
                    Behavior::ParticipantSetAdapter { .. } => (),
                    Behavior::ParticipantSetPredicate { operands, .. } => {
                        for source in operands {
                            self.validate_set_source(source, path)?;
                        }
                    }
                    _ => return Err(Error::InvalidGraph),
                }
                path.pop();
                Ok(())
            }
        }
    }
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
                Behavior::Burnout(config) => config.validate()?,
                Behavior::TeleportLocation {
                    provider,
                    location,
                    manager,
                } => {
                    location.validate()?;
                    manager.validate()?;
                    if node(*provider)?.behavior != Behavior::ParticipantProvider {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::VehicleTuning { source, provider }
                | Behavior::ItemInfo { source, provider } => {
                    source.validate()?;
                    if node(*provider)?.behavior != Behavior::ParticipantProvider {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::ContextReporter {
                    source,
                    provider,
                    categories,
                } => {
                    source.validate()?;
                    if node(*provider)?.behavior != Behavior::ParticipantProvider {
                        return Err(Error::InvalidGraph);
                    }
                    if categories.len() > 128 {
                        return Err(Error::Bound);
                    }
                }
                Behavior::ParticipantTransition {
                    provider,
                    sources,
                    target,
                } => {
                    if node(*provider)?.behavior != Behavior::ParticipantProvider {
                        return Err(Error::InvalidGraph);
                    }
                    if sources.len() > crate::participant_states::MAX_STATES {
                        return Err(Error::Bound);
                    }
                    for key in sources.iter().chain(target.iter()) {
                        key.validate()?;
                    }
                }
                Behavior::TriggeredTransition { sources, target } => {
                    if sources.len() > crate::participant_states::MAX_STATES {
                        return Err(Error::Bound);
                    }
                    for key in sources.iter().chain(target.iter()) {
                        key.validate()?;
                    }
                }
                Behavior::LinkedCurve { source } => source.validate()?,
                Behavior::EventDetails {
                    source: Some(source),
                } => source.validate()?,
                Behavior::ParticipantRankQuery { sorter } => sorter.validate()?,
                Behavior::Bank { bank, provider, .. } => {
                    bank.validate()?;
                    if node(*provider)?.behavior != Behavior::ParticipantProvider {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::ParticipantSetPredicate {
                    operation,
                    operands,
                } => {
                    use super::participant_sets::{MAX_OPERANDS, Operation};
                    if operands.len() > MAX_OPERANDS
                        || (*operation == Operation::Difference && operands.len() != 2)
                    {
                        return Err(Error::Bound);
                    }
                    for source in operands {
                        self.validate_set_source(source, &mut vec![index as u16])?;
                    }
                }
                Behavior::ParticipantSetCounter {
                    source: Some(source),
                } => self.validate_set_source(source, &mut vec![index as u16])?,
                Behavior::ParticipantSetAccessor { set, provider }
                | Behavior::ParticipantSetAdapter {
                    source: set,
                    provider,
                } => {
                    if let Some(set) = set {
                        set.validate()?;
                    }
                    if let Some(provider) = provider
                        && node(*provider)?.behavior != Behavior::ParticipantProvider
                    {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::ParticipantEvent {
                    provider: Some(id), ..
                } => {
                    if node(*id)?.behavior != Behavior::ParticipantProvider {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::ParticipantRank { property, .. } => property.validate()?,
                Behavior::FloatSwitch(definition) => definition.validate()?,
                Behavior::IntegerHub { count } | Behavior::FloatHub { count }
                    if *count == 0 || *count > super::integer_hub::MAX_INPUTS =>
                {
                    return Err(Error::Bound);
                }
                Behavior::ParticipantProperty { property, kind, .. } => {
                    property.validate()?;
                    if !matches!(
                        kind,
                        super::properties::Kind::Integer
                            | super::properties::Kind::Boolean
                            | super::properties::Kind::Float
                    ) {
                        return Err(Error::InvalidGraph);
                    }
                }
                Behavior::IntegerArithmetic { operations }
                    if operations.len() > arithmetic::MAX_OPERATIONS =>
                {
                    return Err(Error::Bound);
                }
                Behavior::Ai(definition) => {
                    use super::ai_commands::Definition as Ai;
                    let state = |id| match &node(id)?.behavior {
                        Behavior::Ai(Ai::State { .. }) => Ok(()),
                        _ => Err(Error::InvalidGraph),
                    };
                    match definition {
                        Ai::Enable(v) => {
                            v.actor_manager.validate()?;
                            v.ai_manager.validate()?;
                            if let Some(id) = v.state {
                                state(id)?;
                            }
                        }
                        Ai::Control(v) => {
                            if v.states.len() > super::ai_commands::MAX_COMMANDS
                                || v.commands.len() > super::ai_commands::MAX_COMMANDS
                            {
                                return Err(Error::Bound);
                            }
                            for &id in &v.states {
                                state(id)?;
                            }
                            for &id in &v.commands {
                                if !matches!(
                                    node(id)?.behavior,
                                    Behavior::Ai(Ai::Goto(_) | Ai::Scripted(_))
                                ) {
                                    return Err(Error::InvalidGraph);
                                }
                            }
                            v.command_manager.validate()?;
                            v.actor_manager.validate()?;
                            v.context_manager.validate()?;
                        }
                        Ai::Goto(v) => {
                            for &value in v.transform.iter().flatten() {
                                super::finite(value)?;
                            }
                            super::finite(v.radius)?;
                            if let Some(location) = &v.location {
                                location.validate()?;
                            }
                        }
                        Ai::Scripted(v) => v.validate()?,
                        Ai::State { .. } => {}
                    }
                }
                Behavior::Snapshot(values) => {
                    let mut fields = std::collections::BTreeSet::new();
                    if values.len() > MAX_PROPERTIES {
                        return Err(Error::Bound);
                    }
                    for (field, value) in values {
                        if !fields.insert(field) {
                            return Err(Error::InvalidGraph);
                        }
                        if let Value::Float(v) = value {
                            super::finite(*v)?;
                        }
                    }
                }
                Behavior::Constant {
                    value: Value::Float(value),
                    ..
                } => {
                    super::finite(*value)?;
                }
                Behavior::IntegerSum { count }
                    if *count == 0 || usize::from(*count) > arithmetic::MAX_OPERATIONS + 1 =>
                {
                    return Err(Error::Bound);
                }
                Behavior::RankIntegers { count, .. }
                    if *count == 0 || *count > super::integers::MAX_RANK_INPUTS =>
                {
                    return Err(Error::Bound);
                }
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
            .map(|(_, value)| value.clone()))
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
