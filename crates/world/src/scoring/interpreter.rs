// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Execute a compiled scoring `Program` for one participant session.
//!
//! The session is sans-IO: the caller supplies time, owned vehicle state and
//! decoded reports, and commits the returned output (remote events, query
//! commands, keeper award requests and dirty synchronized values) together
//! with its own state. Every operation runs on a private candidate and either
//! commits completely or leaves the session unchanged. Nothing here persists
//! REP, encodes packets or trusts a client-supplied award.
use super::{
    Error, arithmetic, boolean_fold, collision, curve, delay, donut, draft,
    events::{
        self, Context, Dispatch, Endpoint, EntityContext, Graph, Handler, PropertyHandler,
        RemoteEmission,
    },
    finite, nodes, nos, participant_properties,
    program::{Behavior, Program, Step},
    properties::{Definition, Kind, Port, Store},
    property_cast, property_gate, proximity,
    reports::Report,
    road, score, stats, stopwatch, synced,
    value::Value,
    vector_dot, vector_length,
    vehicle_query::{self, DriftSample, Query},
};
use crate::nitrous_fuel::Buildup;
use std::collections::BTreeMap;
use std::sync::Arc;
mod ai;
pub mod hosted;

pub const MAX_OUTPUT: usize = 4096;
pub const GAME_ID: u32 = 0x9897_f226;
pub const EVENT_ID: u32 = 0x128f_f024;
pub const EVENT_ID_STRING: u32 = 0xac21_a831;
pub const CONTEXT_ACTIVE: u32 = 0x9411_8d49;
pub const CONTEXT_MATCH: u32 = 0xdede_de4c;
pub const VEHICLE_TUNING: u32 = 0x0b88_ff5c;

/// Owned vehicle state for the query owner, as measured by the server. Fields
/// the server cannot measure are `None`: their query outputs stay as they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vehicle {
    /// Stable physics identity used to exclude this car from candidate sets.
    pub identity: u32,
    /// Right, up, forward and translation vectors.
    pub transform: [[f32; 3]; 4],
    pub velocity: [f32; 3],
    /// World-frame angular velocity, rad/s (doughnut detection).
    pub angular_velocity: [f32; 3],
    pub airborne: Option<bool>,
    pub state_3c230e35: Option<bool>,
    pub state_cd5ae2f0: Option<bool>,
    pub max_rpm: Option<f32>,
    pub rpm: Option<f32>,
    /// Owned nitrous measurement; absent keeps the nitrous query silent.
    pub nos: Option<nos::Sample>,
    pub burnout: Option<super::burnout::Controls>,
}

impl Vehicle {
    /// Motion measurements from a decoded chassis report. Control state is
    /// supplied separately by the caller, including the airborne enum; fields
    /// not present in Motion remain absent until that input is supplied.
    pub fn from_motion(
        identity: u32,
        motion: &crate::replication::vehicle::chassis::Motion,
    ) -> Result<Self, Error> {
        let basis =
            crate::replication::vehicle::orientation::basis_from_quaternion(motion.quaternion)
                .map_err(|_| Error::NonFinite)?;
        let transform = [basis[0], basis[1], basis[2], motion.position];
        for component in transform
            .iter()
            .flatten()
            .chain(&motion.velocity)
            .chain(&motion.angular_velocity)
        {
            finite(*component)?;
        }
        Ok(Self {
            identity,
            transform,
            velocity: motion.velocity,
            angular_velocity: motion.angular_velocity,
            airborne: None,
            state_3c230e35: None,
            state_cd5ae2f0: None,
            max_rpm: None,
            rpm: None,
            nos: None,
            burnout: None,
        })
    }
}

/// Per-tick inputs. `now` is a monotonic clock in seconds; `time_scale` is the
/// engine context scale supplied by the caller's policy (never assumed here).
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    pub participant: u64,
    pub vehicle: Option<Vehicle>,
    pub others: Vec<draft::Vehicle>,
    pub road: Option<road::Sample>,
    /// Owned vehicle figures for the stats provider; absent publishes nothing.
    pub stats: Option<stats::Stats>,
    pub now: f64,
    pub time_scale: f32,
}

/// Reputation category credited by a keeper. Total is separate from the five
/// categories; the installed graph wires it through its own keeper.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum Category {
    Total,
    Build,
    Crew,
    Outlaw,
    Speed,
    Style,
}

/// A keeper award request produced by a Score node. `keeper` is the link ID
/// declared by the installed channel; the scene resolves it to a category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Award {
    pub keeper: u32,
    pub participant: u64,
    pub amount: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub ai: Vec<super::ai_commands::Change>,
    pub burnout_overrides: Vec<super::burnout_override::Change>,
    pub teleport_locations: Vec<super::teleport_location::Request>,
    pub transitions: Vec<super::transitions::Request>,
    pub channel_events: Vec<super::transitions::ChannelEvent>,
    /// ParticipantNameProvider method-0 calls; repeats remain observable.
    pub participant_names: Vec<(u16, u64)>,
    pub rank_queries: Vec<super::rank_query::Update>,
    pub bank_requests: Vec<super::bank::Request>,
    pub participant_sets: Vec<super::participant_sets::Change>,
    /// Changes to the participant store, to deliver to the participant's other
    /// logic instances in order. They are not durable progression awards.
    pub participant_properties: Vec<participant_properties::Write>,
    pub buildup: Buildup,
    pub remote: Vec<RemoteEmission>,
    pub commands: Vec<(u16, collision::Command)>,
    pub awards: Vec<Award>,
    /// Synchronized nodes whose Out changed, with the current value.
    pub synced: Vec<(u16, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    retired: bool,
    deactivated: std::collections::BTreeSet<u16>,
    ai: ai::State,
    random_context: Option<super::random_float::Context>,
    random_entities: BTreeMap<u16, u32>,
    random_seeds: std::collections::VecDeque<u32>,
    item_samples:
        BTreeMap<(participant_properties::Property, u64), Option<super::item_info::Sample>>,
    item_observers: Vec<u16>,
    vehicle_info_observers: Vec<u16>,
    burnouts: BTreeMap<u16, super::burnout::State>,
    burnout_observers: Vec<u16>,
    burnout_overrides: BTreeMap<u16, super::burnout_override::State>,
    burnout_changes: Vec<super::burnout_override::Change>,
    teleport_locations: Vec<super::teleport_location::Request>,
    tuning_observers: Vec<u16>,
    event_strings: BTreeMap<participant_properties::Property, super::value::Text>,
    context_categories:
        BTreeMap<(participant_properties::Property, u64), std::collections::BTreeSet<u32>>,
    context_observers: Vec<u16>,
    context_active: std::collections::BTreeSet<u16>,
    event_channels: BTreeMap<u16, (u64, u64)>,
    transitions: Vec<super::transitions::Request>,
    channel_events: Vec<super::transitions::ChannelEvent>,
    linked_curves: BTreeMap<participant_properties::Property, (curve::Curve, bool)>,
    event_ids: BTreeMap<participant_properties::Property, u32>,
    participant_names: Vec<(u16, u64)>,
    rank_tables: BTreeMap<participant_properties::Property, Vec<super::rank_query::Row>>,
    rank_observers: Vec<u16>,
    active_rank_queries: std::collections::BTreeSet<u16>,
    rank_rows: BTreeMap<u16, super::rank_query::Row>,
    rank_updates: Vec<super::rank_query::Update>,
    game_id: Option<u64>,
    bank_serial: u64,
    bank_pending: BTreeMap<u64, u16>,
    bank_requests: Vec<super::bank::Request>,
    participant_sets: BTreeMap<super::participant_sets::Key, std::collections::BTreeSet<u64>>,
    scoped_sets: BTreeMap<(super::participant_sets::Key, u64), std::collections::BTreeSet<u64>>,
    local_sets: BTreeMap<u16, std::collections::BTreeSet<u64>>,
    set_observers: Vec<u16>,
    set_changes: Vec<super::participant_sets::Change>,
    participant_providers: BTreeMap<u16, u64>,
    participant_values: BTreeMap<(u64, participant_properties::Property, Kind), Value>,
    bound_accessors: Vec<(u16, u64)>,
    property_writes: Vec<participant_properties::Write>,
    store: Store,
    scalars: BTreeMap<u16, nodes::Node>,
    arithmetic: BTreeMap<u16, arithmetic::Node>,
    folds: BTreeMap<u16, boolean_fold::Node>,
    vectors: BTreeMap<u16, vector_length::Node>,
    dots: BTreeMap<u16, vector_dot::Node>,
    gates: BTreeMap<u16, property_gate::Gate>,
    synced: BTreeMap<u16, synced::Node>,
    collisions: BTreeMap<u16, collision::Node>,
    roads: BTreeMap<u16, road::Node>,
    proximity: BTreeMap<u16, proximity::Node>,
    nos: BTreeMap<u16, nos::Node>,
    buildup_counts: BTreeMap<u16, [u32; 2]>,
    buildup: Buildup,
    donuts: BTreeMap<u16, donut::Node>,
    /// Stats nodes with the figures they last published.
    stats: BTreeMap<u16, Option<stats::Stats>>,
    stopwatches: stopwatch::Manager,
    delays: delay::Manager<Option<Context>>,
    contexts: BTreeMap<u16, score::Context>,
    /// Query nodes activated and scheduled at interval one.
    scheduled: Vec<u16>,
    awards: Vec<Award>,
    commands: Vec<(u16, collision::Command)>,
}

#[derive(Clone)]
struct Candidate<'a> {
    program: &'a Program,
    inputs: &'a Inputs,
    state: State,
}

#[derive(Clone, Debug)]
pub struct Session {
    program: Arc<Program>,
    graph: Arc<Graph>,
    state: State,
    started: bool,
    failed_step: Option<usize>,
}

impl Session {
    /// Build an unstarted session: typed unready cells, fresh node state, no
    /// listeners. `start` runs the compiled startup sequence.
    pub fn new(program: Arc<Program>) -> Result<Self, Error> {
        program.validate()?;
        let count = program.nodes.len();
        let graph = Graph::new(count, program.connections.clone())?;
        let cells: Vec<Definition> = program.cells.clone();
        let store = Store::new(count, cells)?;
        let mut state = State {
            retired: false,
            deactivated: std::collections::BTreeSet::new(),
            ai: ai::State::default(),
            item_samples: BTreeMap::new(),
            random_context: None,
            random_entities: BTreeMap::new(),
            random_seeds: std::collections::VecDeque::new(),
            item_observers: Vec::new(),
            vehicle_info_observers: Vec::new(),
            burnouts: BTreeMap::new(),
            burnout_observers: Vec::new(),
            burnout_overrides: BTreeMap::new(),
            burnout_changes: Vec::new(),
            teleport_locations: Vec::new(),
            tuning_observers: Vec::new(),
            event_strings: BTreeMap::new(),
            context_categories: BTreeMap::new(),
            context_observers: Vec::new(),
            context_active: std::collections::BTreeSet::new(),
            event_channels: BTreeMap::new(),
            transitions: Vec::new(),
            channel_events: Vec::new(),
            linked_curves: BTreeMap::new(),
            game_id: None,
            event_ids: BTreeMap::new(),
            participant_names: Vec::new(),
            bank_serial: 0,
            bank_pending: BTreeMap::new(),
            bank_requests: Vec::new(),
            rank_tables: BTreeMap::new(),
            rank_observers: Vec::new(),
            active_rank_queries: std::collections::BTreeSet::new(),
            rank_rows: BTreeMap::new(),
            rank_updates: Vec::new(),
            participant_sets: BTreeMap::new(),
            scoped_sets: BTreeMap::new(),
            local_sets: BTreeMap::new(),
            set_observers: Vec::new(),
            set_changes: Vec::new(),
            participant_providers: BTreeMap::new(),
            participant_values: BTreeMap::new(),
            bound_accessors: Vec::new(),
            property_writes: Vec::new(),
            store,
            scalars: BTreeMap::new(),
            arithmetic: BTreeMap::new(),
            folds: BTreeMap::new(),
            vectors: BTreeMap::new(),
            dots: BTreeMap::new(),
            gates: BTreeMap::new(),
            synced: BTreeMap::new(),
            collisions: BTreeMap::new(),
            roads: BTreeMap::new(),
            proximity: BTreeMap::new(),
            nos: BTreeMap::new(),
            buildup_counts: BTreeMap::new(),
            buildup: Buildup::default(),
            donuts: BTreeMap::new(),
            stats: BTreeMap::new(),
            stopwatches: stopwatch::Manager::new(count, &[])?,
            delays: delay::Manager::new(count, &[])?,
            contexts: BTreeMap::new(),
            scheduled: Vec::new(),
            awards: Vec::new(),
            commands: Vec::new(),
        };
        let mut watches = Vec::new();
        let mut delays = Vec::new();
        for (i, node) in program.nodes.iter().enumerate() {
            let id = i as u16;
            if !node.created {
                continue;
            }
            match &node.behavior {
                Behavior::Burnout(_) => {
                    state.burnouts.insert(id, super::burnout::State::default());
                }
                Behavior::VectorDot => {
                    state.dots.insert(id, vector_dot::Node::default());
                }
                Behavior::Scalar(definition) => {
                    state.scalars.insert(id, nodes::Node::new(*definition)?);
                }
                Behavior::Arithmetic {
                    operations,
                    connected,
                } => {
                    state
                        .arithmetic
                        .insert(id, arithmetic::Node::new(operations.clone(), connected)?);
                }
                Behavior::Fold {
                    all,
                    input_count,
                    connected,
                } => {
                    state
                        .folds
                        .insert(id, boolean_fold::Node::new(*all, *input_count, connected)?);
                }
                Behavior::VectorLength {
                    length_bound,
                    normal_bound,
                } => {
                    state
                        .vectors
                        .insert(id, vector_length::Node::new(*length_bound, *normal_bound));
                }
                Behavior::Gate {
                    definition,
                    outputs,
                } => {
                    state
                        .gates
                        .insert(id, property_gate::Gate::new(*definition, outputs)?);
                }
                Behavior::Synced(kind) => {
                    state.synced.insert(id, synced::Node::new(*kind));
                }
                Behavior::Collision(outputs) => {
                    state.collisions.insert(id, collision::Node::new(*outputs));
                }
                Behavior::Proximity => {
                    state.proximity.insert(id, proximity::Node::default());
                }
                Behavior::Nos => {
                    state.nos.insert(id, nos::Node::default());
                }
                Behavior::NosBuildup => {
                    state.buildup_counts.insert(id, [0; 2]);
                }
                Behavior::Donut(_) => {
                    state.donuts.insert(id, donut::Node::default());
                }
                Behavior::Stats => {
                    state.stats.insert(id, None);
                }
                Behavior::Road {
                    configuration,
                    limit_bound,
                } => {
                    // Fresh counters start at zero: the native fresh-start
                    // initialization is not established, so this is the
                    // session's documented policy, not a native claim.
                    let snapshot = road::Snapshot {
                        last_speeding_time: 0.0,
                        last_not_speeding_time: 0.0,
                        speeding_duration: 0.0,
                        excessive_duration: 0.0,
                        not_speeding_duration: 0.0,
                        wrong_way_distance: 0.0,
                        speed_limit: 0.0,
                        wrong_way: false,
                        immediate_speeding_check: false,
                    };
                    state.roads.insert(
                        id,
                        road::Node::from_state(*configuration, snapshot, *limit_bound)?,
                    );
                }
                Behavior::Stopwatch { time_bound } => watches.push((id, *time_bound)),
                Behavior::Delay(definition) => delays.push((id, *definition)),
                _ => {}
            }
        }
        state.stopwatches = stopwatch::Manager::new(count, &watches)?;
        state.delays = delay::Manager::new(count, &delays)?;
        Ok(Self {
            program,
            graph: Arc::new(graph),
            state,
            started: false,
            failed_step: None,
        })
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Current value of any port, after commit. Unbound ports are an error.
    pub fn read(&self, port: Port) -> Result<Option<Value>, Error> {
        self.state.store.read(port)
    }

    pub fn started(&self) -> bool {
        self.started
    }

    fn transaction(
        &mut self,
        inputs: &Inputs,
        action: impl FnOnce(&mut Candidate<'_>, &mut Dispatch<'_>) -> Result<(), Error>,
    ) -> Result<Output, Error> {
        if self.state.retired {
            return Err(Error::InvalidGraph);
        }
        let candidate = Candidate {
            program: &self.program,
            inputs,
            state: self.state.clone(),
        };
        let (next, remote) = self.graph.transaction(&candidate, action)?;
        self.commit_output(next.state, remote)
    }

    fn commit_output(
        &mut self,
        mut state: State,
        remote: Vec<RemoteEmission>,
    ) -> Result<Output, Error> {
        if state.awards.len() > MAX_OUTPUT
            || state.commands.len() > MAX_OUTPUT
            || remote.len() > MAX_OUTPUT
        {
            return Err(Error::Bound);
        }
        let awards = std::mem::take(&mut state.awards);
        let commands = std::mem::take(&mut state.commands);
        let mut synced = Vec::new();
        for (&node, synced_state) in &mut state.synced {
            if synced_state.take_dirty() {
                let value = state
                    .store
                    .read(Port {
                        node,
                        field: synced::OUT,
                    })?
                    .ok_or(Error::MissingInput)?;
                synced.push((node, value));
            }
        }
        self.state = state;
        Ok(Output {
            ai: std::mem::take(&mut self.state.ai.output),
            burnout_overrides: std::mem::take(&mut self.state.burnout_changes),
            teleport_locations: std::mem::take(&mut self.state.teleport_locations),
            transitions: std::mem::take(&mut self.state.transitions),
            channel_events: std::mem::take(&mut self.state.channel_events),
            participant_names: std::mem::take(&mut self.state.participant_names),
            rank_queries: std::mem::take(&mut self.state.rank_updates),
            bank_requests: std::mem::take(&mut self.state.bank_requests),
            participant_sets: std::mem::take(&mut self.state.set_changes),
            participant_properties: std::mem::take(&mut self.state.property_writes),
            buildup: std::mem::take(&mut self.state.buildup),
            remote,
            commands,
            awards,
            synced,
        })
    }

    /// Supply the owned item manager's current sample. Pre-start values are read
    /// at binding; later notifications refresh bound observers in registration
    /// order. Missing items only publish Valid=false, retaining other outputs.
    pub fn notify_item_info(
        &mut self,
        source: &participant_properties::Property,
        participant: u64,
        sample: Option<super::item_info::Sample>,
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        source.validate()?;
        self.transaction(inputs, |candidate, dispatch| {
            let key = (source.clone(), participant);
            if !candidate.state.item_samples.contains_key(&key)
                && candidate.state.item_samples.len() >= participant_properties::MAX_VALUES
            {
                return Err(Error::Bound);
            }
            candidate.state.item_samples.insert(key, sample);
            for id in candidate.state.item_observers.clone() {
                if let Behavior::ItemInfo {
                    source: observed,
                    provider,
                } = &candidate.program.node(id)?.behavior
                    && observed == source
                    && candidate.state.participant_providers.get(provider) == Some(&participant)
                {
                    candidate.item_info(id, dispatch)?;
                }
            }
            Ok(())
        })
    }

    /// Notify the active own-car info queries. Activation publishes zero/empty,
    /// without querying the provider; each subsequent change emits CHANGED even
    /// when detaching an already empty provider. A present car republishes all
    /// three properties; detachment suppresses equal ready defaults.
    pub fn notify_vehicle_info(
        &mut self,
        sample: Option<&super::vehicle_info::Sample>,
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        self.transaction(inputs, |candidate, dispatch| {
            for id in candidate.state.vehicle_info_observers.clone() {
                for (field, value) in super::vehicle_info::outputs(sample)? {
                    let port = Port { node: id, field };
                    if candidate.bound(port)
                        && (sample.is_some()
                            || candidate.state.store.read(port)?.as_ref() != Some(&value))
                    {
                        dispatch.publish(candidate, port, value)?;
                    }
                }
                dispatch.emit(
                    candidate,
                    Endpoint {
                        node: id,
                        event: super::vehicle_info::CHANGED,
                    },
                )?;
            }
            Ok(())
        })
    }

    /// Deliver an owned controllable-provider change to bound override nodes.
    /// The host calls this when replacing or detaching the participant's car.
    pub fn vehicle_changed(&mut self, inputs: &Inputs) -> Result<Output, Error> {
        self.transaction(inputs, |candidate, dispatch| {
            for id in candidate.state.burnout_observers.clone() {
                candidate.burnout_reset(id, dispatch)?;
            }
            let nodes: Vec<_> = candidate.state.burnout_overrides.keys().copied().collect();
            for id in nodes {
                candidate.burnout_change(id, None)?;
            }
            Ok(())
        })
    }

    /// Consume a retired logic instance and release its vehicle overrides.
    /// Commit the returned changes before removing the corresponding car.
    pub fn retire(mut self, inputs: &Inputs) -> Result<Output, Error> {
        self.transaction(inputs, |candidate, dispatch| {
            candidate.retire_owned(dispatch)
        })
    }

    /// Supply the deterministic seed context for this dispatch. The host owns
    /// the clock/epoch; graph age and participant IDs are not substitutes.
    pub fn set_random_context(&mut self, context: super::random_float::Context) {
        self.state.random_context = Some(context);
    }

    /// Resolve the native entity identifier before startup; prefab occurrences
    /// need their own identity even when they share the same blueprint object.
    pub fn bind_random_entity(&mut self, node: u16, entity: u32) -> Result<(), Error> {
        if self.started
            || !matches!(
                self.program.node(node)?.behavior,
                Behavior::RandomFloat { .. }
            )
            || self.state.random_entities.contains_key(&node)
        {
            return Err(Error::InvalidGraph);
        }
        self.state.random_entities.insert(node, entity);
        Ok(())
    }

    /// Mode-1 inputs, already selected from a native seed override or external
    /// entropy. Each TrueRandom evaluation consumes one, transactionally.
    pub fn queue_random_seeds(&mut self, seeds: &[u32]) -> Result<(), Error> {
        if seeds.len() > super::random_float::MAX_PENDING_SEEDS - self.state.random_seeds.len() {
            return Err(Error::Bound);
        }
        self.state.random_seeds.extend(seeds);
        Ok(())
    }

    /// Bind the owning GameManager identity before this logic instance starts.
    /// Missing context is distinct from a game whose ID has zero bits.
    pub fn bind_game_id(&mut self, game_id: u64) -> Result<(), Error> {
        if self.started || self.state.game_id.is_some() {
            return Err(Error::InvalidGraph);
        }
        self.state.game_id = Some(game_id);
        Ok(())
    }

    /// Deliver a vehicle-item manager notification. The caller computes the
    /// index from its owned item collection; None means the participant has no
    /// vehicle item. Native binding subscribes without reading the existing
    /// item, and every present-item notification publishes, even when equal.
    pub fn notify_vehicle_tuning(
        &mut self,
        source: &participant_properties::Property,
        participant: u64,
        value: Option<f32>,
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        source.validate()?;
        if let Some(value) = value {
            finite(value)?;
        }
        self.transaction(inputs, |candidate, dispatch| {
            for id in candidate.state.tuning_observers.clone() {
                if let Behavior::VehicleTuning {
                    source: key,
                    provider,
                } = &candidate.program.node(id)?.behavior
                    && key == source
                    && candidate.state.participant_providers.get(provider) == Some(&participant)
                {
                    let port = Port {
                        node: id,
                        field: VEHICLE_TUNING,
                    };
                    if candidate.bound(port)
                        && (value.is_some()
                            || candidate.state.store.read(port)? != Some(Value::Float(5.0)))
                    {
                        dispatch.publish(candidate, port, Value::Float(value.unwrap_or(5.0)))?;
                    }
                }
            }
            Ok(())
        })
    }

    /// Supply the categories whose counts are positive in the owning event
    /// context registry. An empty present registry differs from no binding.
    /// Each import represents a native registry notification, even if positive
    /// membership stayed equal (for example, a count changing from one to two).
    pub fn import_context_categories(
        &mut self,
        source: &participant_properties::Property,
        participant: u64,
        categories: &[u32],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        source.validate()?;
        if categories.len() > 128 {
            return Err(Error::Bound);
        }
        self.transaction(inputs, |candidate, dispatch| {
            candidate.import_owned(
                events::native::Import::Context {
                    source: source.clone(),
                    participant,
                    categories: categories.to_vec(),
                },
                dispatch,
            )
        })
    }

    /// The spawning participant logic supplies both the weak channel target
    /// and participant (native 04780ee0); incoming events do not select them.
    pub fn bind_event_channel(
        &mut self,
        node: u16,
        channel: u64,
        participant: u64,
    ) -> Result<(), Error> {
        if self.started
            || self.program.node(node)?.behavior != Behavior::ParticipantEventChannel
            || self.state.event_channels.contains_key(&node)
        {
            return Err(Error::InvalidGraph);
        }
        self.state
            .event_channels
            .insert(node, (channel, participant));
        Ok(())
    }

    /// Resolve an immutable curve from the enclosing event composition before
    /// startup. Equal channel names in other event sessions remain independent.
    pub fn bind_curve(
        &mut self,
        source: participant_properties::Property,
        curve: curve::Curve,
        apply: bool,
    ) -> Result<(), Error> {
        source.validate()?;
        if self.started || self.state.linked_curves.contains_key(&source) {
            return Err(Error::InvalidGraph);
        }
        if self.state.linked_curves.len() >= super::participant_sets::MAX_SETS {
            return Err(Error::Bound);
        }
        self.state.linked_curves.insert(source, (curve, apply));
        Ok(())
    }

    /// Bind an EventDetails asset's exact UInt32 identifier for this context.
    /// The accessor publishes only at bind; it does not observe later changes.
    pub fn bind_event_id(
        &mut self,
        source: participant_properties::Property,
        value: u32,
    ) -> Result<(), Error> {
        source.validate()?;
        if self.started || self.state.event_ids.contains_key(&source) {
            return Err(Error::InvalidGraph);
        }
        if self.state.event_ids.len() >= super::participant_sets::MAX_SETS {
            return Err(Error::Bound);
        }
        self.state.event_ids.insert(source, value);
        Ok(())
    }

    pub fn bind_event_string(
        &mut self,
        source: participant_properties::Property,
        value: super::value::Text,
    ) -> Result<(), Error> {
        source.validate()?;
        if self.started || self.state.event_strings.contains_key(&source) {
            return Err(Error::InvalidGraph);
        }
        if self.state.event_strings.len() >= 256 {
            return Err(Error::Bound);
        }
        self.state.event_strings.insert(source, value);
        Ok(())
    }

    /// Notify queries of one owned sorter's new ordered table. This also works
    /// before start to seed the table, without synthesizing a bind-time update.
    /// Other logic instances and event lifetimes own separate tables.
    pub fn import_rank_table(
        &mut self,
        sorter: &participant_properties::Property,
        rows: &[super::rank_query::Row],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        sorter.validate()?;
        super::rank_query::validate(rows)?;
        self.transaction(inputs, |candidate, dispatch| {
            candidate.import_owned(
                events::native::Import::RankTable {
                    sorter: sorter.clone(),
                    rows: rows.to_vec(),
                },
                dispatch,
            )
        })
    }

    /// Deliver a terminal result from the owning durable ledger. Repeated or
    /// out-of-order replies for already completed requests do not emit twice.
    /// Unknown future IDs fail. Destroying the session discards its callbacks.
    pub fn complete_bank(
        &mut self,
        request: u64,
        success: bool,
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        if !self.started || request == 0 || request > self.state.bank_serial {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            if let Some(node) = candidate.state.bank_pending.remove(&request) {
                dispatch.emit(
                    candidate,
                    Endpoint {
                        node,
                        event: if success {
                            super::bank::SUCCESS
                        } else {
                            super::bank::FAILURE
                        },
                    },
                )?;
            }
            Ok(())
        })
    }

    /// Run the compiled startup sequence once: bus construction, binding,
    /// listener registration and insertion-order activation.
    pub fn start(&mut self, inputs: &Inputs) -> Result<Output, Error> {
        if self.started {
            return Err(Error::InvalidGraph);
        }
        let mut failed = None;
        let output = self.transaction(inputs, |candidate, dispatch| {
            candidate.startup(&mut failed, dispatch)
        });
        self.failed_step = failed;
        let output = output?;
        self.started = true;
        Ok(output)
    }

    /// Index of the startup step that failed in the last `start` call, for
    /// diagnostics. `None` after a successful start.
    pub fn failed_step(&self) -> Option<usize> {
        self.failed_step
    }

    /// Seed before start or deliver another owned logic's changes afterwards.
    /// Imports notify bound accessors but do not echo the imported writes.
    /// Publications caused by those notifications remain ordinary local writes.
    pub fn set_participant_properties(
        &mut self,
        values: &[(participant_properties::Property, i32)],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        if values.len() > participant_properties::MAX_VALUES {
            return Err(Error::Bound);
        }
        self.set_participant_values(
            &values
                .iter()
                .map(|(p, v)| (p.clone(), Value::Integer(*v)))
                .collect::<Vec<_>>(),
            inputs,
        )
    }

    pub fn set_participant_values(
        &mut self,
        values: &[(participant_properties::Property, Value)],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        self.import_participant_values(inputs.participant, values, inputs)
    }

    /// Import ordered membership changes from the owned event host without
    /// echoing them. Any resulting graph commands remain transactional.
    pub fn import_participant_sets(
        &mut self,
        changes: &[super::participant_sets::Change],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        if changes.len() > MAX_OUTPUT {
            return Err(Error::Bound);
        }
        for change in changes {
            change.set.validate()?;
        }
        self.transaction(inputs, |candidate, dispatch| {
            for change in changes {
                candidate.change_set(change, None, dispatch)?;
            }
            Ok(())
        })
    }

    /// Update a participant-specific set provider, such as that participant's
    /// event or group. It is distinct from a global set at the same channel.
    pub fn import_scoped_set(
        &mut self,
        source: &super::participant_sets::Key,
        participant: u64,
        members: &[u64],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        use super::participant_sets::{MAX_MEMBERS, MAX_SETS};
        source.validate()?;
        if members.len() > MAX_MEMBERS {
            return Err(Error::Bound);
        }
        let desired = members
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if desired.len() != members.len() {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs,|c,d| {
            let key=(source.clone(),participant);
            if !c.state.scoped_sets.contains_key(&key) && c.state.scoped_sets.len()>=MAX_SETS { return Err(Error::Bound); }
            c.state.scoped_sets.insert(key,desired);
            for node in c.state.set_observers.clone() {
                if matches!(&c.program.node(node)?.behavior,
                    Behavior::ParticipantSetAdapter { source:Some(key),provider:Some(provider) }
                    if key==source && c.state.participant_providers.get(provider)==Some(&participant)) {
                    c.refresh_set(node,d)?;
                }
            }
            Ok(())
        })
    }

    /// Import another participant's values while retaining this logic's own
    /// execution inputs. Derived writes keep their bound provider identity.
    pub fn import_participant_values(
        &mut self,
        participant: u64,
        values: &[(participant_properties::Property, Value)],
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        if values.len() > participant_properties::MAX_VALUES {
            return Err(Error::Bound);
        }
        let mut seen = std::collections::BTreeSet::new();
        for (property, value) in values {
            property.validate()?;
            if !seen.insert((property, participant_properties::kind(value)?)) {
                return Err(Error::InvalidGraph);
            }
        }
        self.transaction(inputs, |candidate, dispatch| {
            for (property, value) in values {
                candidate.write_participant(
                    participant,
                    property,
                    value.clone(),
                    None,
                    dispatch,
                )?;
            }
            Ok(())
        })
    }

    pub fn participant_property(
        &self,
        participant: u64,
        property: &participant_properties::Property,
    ) -> Option<i32> {
        match self.participant_value(participant, property, Kind::Integer) {
            Some(Value::Integer(v)) => Some(v),
            _ => None,
        }
    }

    pub fn participant_value(
        &self,
        participant: u64,
        property: &participant_properties::Property,
        kind: Kind,
    ) -> Option<Value> {
        self.state
            .participant_values
            .get(&(participant, property.clone(), kind))
            .cloned()
    }

    /// One scheduler update: every active query at interval one, then the
    /// stopwatch pass, then the delay pass. `delta` is the elapsed time of this
    /// update in seconds.
    pub fn tick(&mut self, delta: f32, inputs: &Inputs) -> Result<Output, Error> {
        if !self.started {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            candidate.tick_owned(delta, dispatch)
        })
    }

    /// Deliver a component transition immediately, without ticking unrelated
    /// physics queries, timers or buildup observers. Multiple transitions in
    /// one network batch remain separate notifications.
    pub fn update_nitrous(&mut self, inputs: &Inputs) -> Result<Output, Error> {
        if !self.started {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            let nodes: Vec<_> = candidate.state.nos.keys().copied().collect();
            for node in nodes {
                candidate.nos(node, dispatch)?;
            }
            Ok(())
        })
    }

    /// Deliver a decoded, ownership-checked query report to its query node.
    pub fn report(&mut self, node: u16, report: &Report, inputs: &Inputs) -> Result<Output, Error> {
        if !self.started {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            candidate.report_owned(node, report, dispatch)
        })
    }

    /// Emit an external lifecycle event at a node, such as the participant
    /// root's disable notification that banks the pending category scores.
    pub fn event(&mut self, node: u16, event: u32, inputs: &Inputs) -> Result<Output, Error> {
        if !self.started {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            dispatch.emit(candidate, Endpoint { node, event })
        })
    }
    /// Deliver a registry-82 route already resolved against this instance and
    /// the sending connection's owned local players. No arbitrary output port.
    pub fn event_sync(
        &mut self,
        routed: super::event_sync::Routed,
        inputs: &Inputs,
    ) -> Result<Output, Error> {
        let node = self.program.node(routed.node)?;
        if !self.started || !node.created || node.behavior != Behavior::EventSync {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            candidate.emit_with(
                Endpoint {
                    node: routed.node,
                    event: nodes::OUT,
                },
                routed.context.map(Context::Entity),
                dispatch,
            )
        })
    }
}

impl Candidate<'_> {
    fn burnout_publish(&mut self, id: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        for index in 0..3 {
            // A synchronous listener can reset the query while an earlier
            // output is publishing. Read each subsequent field after it.
            let (field, value) = self
                .state
                .burnouts
                .get(&id)
                .ok_or(Error::InvalidGraph)?
                .outputs()[index]
                .clone();
            let port = Port { node: id, field };
            if self.bound(port) && self.state.store.read(port)?.as_ref() != Some(&value) {
                dispatch.publish(self, port, value)?;
            }
        }
        Ok(())
    }
    fn burnout_reset(&mut self, id: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        self.state
            .burnouts
            .get_mut(&id)
            .ok_or(Error::InvalidGraph)?
            .reset();
        self.burnout_publish(id, dispatch)
    }
    fn burnout(
        &mut self,
        id: u16,
        config: super::burnout::Configuration,
        delta: f32,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let Some(vehicle) = self.inputs.vehicle else {
            return Ok(());
        };
        let Some(controls) = vehicle.burnout else {
            return Ok(());
        };
        self.state
            .burnouts
            .get_mut(&id)
            .ok_or(Error::InvalidGraph)?
            .sample(
                config,
                vehicle.velocity,
                vehicle.angular_velocity,
                controls,
                delta,
            )?;
        self.burnout_publish(id, dispatch)
    }
    fn burnout_change(&mut self, id: u16, field: Option<u32>) -> Result<(), Error> {
        use super::burnout_override::{ENABLED, VALUE};
        let Some(Value::Boolean(enabled)) = self.resolve(Port {
            node: id,
            field: ENABLED,
        })?
        else {
            return Err(Error::MissingInput);
        };
        let Some(Value::Float(value)) = self.resolve(Port {
            node: id,
            field: VALUE,
        })?
        else {
            return Err(Error::MissingInput);
        };
        let vehicle = self.inputs.vehicle.map(|v| v.identity);
        let changes = if let Some(state) = self.state.burnout_overrides.get_mut(&id) {
            if let Some(field) = field {
                state.changed(field, enabled, value, vehicle)?
            } else {
                state.resolve(vehicle, value)?
            }
        } else {
            let (state, changes) = super::burnout_override::State::bind(enabled, vehicle, value)?;
            self.state.burnout_overrides.insert(id, state);
            changes
        };
        if self.state.burnout_changes.len() + changes.len() > MAX_OUTPUT {
            return Err(Error::Bound);
        }
        self.state.burnout_changes.extend(changes);
        Ok(())
    }
    fn accessor_value(&self, node: u16, kind: Kind) -> Result<Value, Error> {
        let value = self
            .resolve(Port {
                node,
                field: nodes::FLOAT_VALUE,
            })?
            .ok_or(Error::MissingInput)?;
        kind.validate(&value)?;
        Ok(value)
    }

    fn write_participant(
        &mut self,
        written_participant: u64,
        property: &participant_properties::Property,
        value: Value,
        origin: Option<u16>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let kind = participant_properties::kind(&value)?;
        if let Some(node) = origin
            && dispatch.is_hosted()
        {
            // The owner mutates its store and calls back into every observer,
            // including this graph. Do not update this cache first: doing so
            // would suppress the originating accessor's notification.
            return dispatch.native(
                self,
                node,
                events::native::Call::ParticipantWrite(participant_properties::Write {
                    participant: written_participant,
                    property: property.clone(),
                    value,
                }),
            );
        }
        let key = (written_participant, property.clone(), kind);
        if self.state.participant_values.get(&key) == Some(&value) {
            return Ok(());
        }
        if !self.state.participant_values.contains_key(&key)
            && self.state.participant_values.len() >= participant_properties::MAX_VALUES
        {
            return Err(Error::Bound);
        }
        self.state.participant_values.insert(key, value.clone());
        if origin.is_some() {
            if self.state.property_writes.len() >= MAX_OUTPUT {
                return Err(Error::Bound);
            }
            self.state
                .property_writes
                .push(participant_properties::Write {
                    participant: written_participant,
                    property: property.clone(),
                    value: value.clone(),
                });
        }
        // Native store notifications visit the registered accessor observers in
        // order. Binding is separate from activation and a late reader catches
        // up from the store during its own activation.
        for (node, participant) in self.state.bound_accessors.clone() {
            if participant == written_participant
                && matches!(&self.program.node(node)?.behavior,
                Behavior::ParticipantProperty { property: p, kind: k, .. } if p == property && *k == kind)
            {
                self.publish_bound(node, nodes::OUTPUT_VALUE, value.clone(), dispatch)?;
            }
            if kind == Kind::Integer
                && let Behavior::ParticipantRank {
                    property: p,
                    descending,
                } = self.program.node(node)?.behavior.clone()
                && p == *property
            {
                self.participant_rank(node, participant, &p, descending, dispatch)?;
            }
        }
        Ok(())
    }

    /// Native getter contract: a ready bound cell, otherwise the authored
    /// fallback. Unbound and bound-but-unready are both fallback cases.
    fn resolve(&self, port: Port) -> Result<Option<Value>, Error> {
        match self.state.store.read(port) {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) | Err(Error::MissingInput) => self.program.default(port),
            Err(error) => Err(error),
        }
    }
    fn bound(&self, port: Port) -> bool {
        self.state.store.read(port).is_ok()
    }
    fn apply_effects(
        &mut self,
        node: u16,
        effects: Vec<nodes::Effect>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        for effect in effects {
            match effect {
                nodes::Effect::Publish(field, value) => {
                    let port = Port { node, field };
                    if self.bound(port) {
                        dispatch.publish(self, port, value)?;
                    }
                }
                nodes::Effect::Emit(event) => dispatch.emit(self, Endpoint { node, event })?,
            }
        }
        Ok(())
    }
    fn scalar(
        &mut self,
        node: u16,
        action: nodes::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let effects = {
            let reader = |field: u32| self.resolve(Port { node, field }).ok().flatten();
            let mut state = self
                .state
                .scalars
                .get(&node)
                .ok_or(Error::UnknownNode)?
                .clone();
            let effects = state.apply(action, reader)?;
            self.state.scalars.insert(node, state);
            effects
        };
        self.apply_effects(node, effects, dispatch)
    }
    fn math(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let Behavior::Math { program, .. } = &self.program.node(node)?.behavior else {
            return Err(Error::InvalidGraph);
        };
        let result = program.evaluate(|field, _| self.resolve(Port { node, field }).ok().flatten());
        let value = match result {
            Ok(Some(value)) => value,
            // Division by zero retains the prior output. An input that is not
            // ready suppresses evaluation as a documented modeling decision.
            Ok(None) | Err(Error::MissingInput) => return Ok(()),
            Err(error) => return Err(error),
        };
        let port = Port {
            node,
            field: nodes::OUT,
        };
        if self.bound(port) {
            dispatch.publish(self, port, value)?;
        }
        Ok(())
    }
    fn context_match(
        &mut self,
        node: u16,
        initial: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let Behavior::ContextReporter {
            source,
            provider,
            categories,
        } = &self.program.node(node)?.behavior
        else {
            return Err(Error::InvalidGraph);
        };
        let value = self.state.context_active.contains(&node)
            && self
                .state
                .participant_providers
                .get(provider)
                .and_then(|participant| {
                    self.state
                        .context_categories
                        .get(&(source.clone(), *participant))
                })
                .is_some_and(|present| initial || categories.iter().all(|c| present.contains(c)));
        self.publish_bound(node, CONTEXT_MATCH, Value::Boolean(value), dispatch)
    }

    fn context_activation(
        &mut self,
        node: u16,
        initial: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let active = match self.resolve(Port {
            node,
            field: CONTEXT_ACTIVE,
        })? {
            Some(Value::Boolean(v)) => v,
            None => return Err(Error::MissingInput),
            _ => return Err(Error::InvalidGraph),
        };
        let previous = self.state.context_active.contains(&node);
        if active {
            self.state.context_active.insert(node);
        } else {
            self.state.context_active.remove(&node);
        }
        if initial || active != previous {
            self.context_match(node, initial, dispatch)?;
        }
        Ok(())
    }

    fn curve(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let (curve, apply) = match &self.program.node(node)?.behavior {
            Behavior::Curve { curve, apply, .. } => (curve, *apply),
            Behavior::LinkedCurve { source } => {
                let Some((curve, apply)) = self.state.linked_curves.get(source) else {
                    return Err(Error::MissingInput);
                };
                (curve, *apply)
            }
            _ => return Err(Error::InvalidGraph),
        };
        let output = Port {
            node,
            field: nodes::OUTPUT,
        };
        if !self.bound(output) {
            return Ok(());
        }
        let value = if apply {
            let Some(Value::Float(input)) = self.resolve(Port {
                node,
                field: nodes::INPUT,
            })?
            else {
                return Err(Error::MissingInput);
            };
            curve::Curve::evaluate(curve, input)?
        } else {
            0.0
        };
        dispatch.publish(self, output, Value::Float(value))
    }
    fn arithmetic(
        &mut self,
        node: u16,
        action: nodes::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let effect = {
            let mut state = self
                .state
                .arithmetic
                .get(&node)
                .ok_or(Error::UnknownNode)?
                .clone();
            let effect = state.apply(action, |field| self.resolve(Port { node, field }))?;
            self.state.arithmetic.insert(node, state);
            effect
        };
        self.apply_effects(node, effect.into_iter().collect(), dispatch)
    }
    fn fold(
        &mut self,
        node: u16,
        action: nodes::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let effect = {
            let mut state = self
                .state
                .folds
                .get(&node)
                .ok_or(Error::UnknownNode)?
                .clone();
            let effect = state.apply(action, |field| self.resolve(Port { node, field }))?;
            self.state.folds.insert(node, state);
            effect
        };
        self.apply_effects(node, effect.into_iter().collect(), dispatch)
    }
    fn vector_dot(
        &mut self,
        node: u16,
        action: nodes::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let mut state = self
            .state
            .dots
            .get(&node)
            .ok_or(Error::UnknownNode)?
            .clone();
        let value = state.apply(action, |field| self.resolve(Port { node, field }))?;
        self.state.dots.insert(node, state);
        if let Some(value) = value {
            self.publish_bound(node, vector_dot::OUT, Value::Float(value), dispatch)?;
        }
        Ok(())
    }
    fn query_initial(
        &mut self,
        node: u16,
        query: Query,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        for output in query.outputs() {
            let port = Port {
                node,
                field: output.field,
            };
            if let Some(value) = output.initial.clone()
                && self.bound(port)
            {
                dispatch.initialize(self, port, value, super::properties::DefaultMode::Override)?;
            }
        }
        Ok(())
    }
    fn item_info(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let Behavior::ItemInfo { source, provider } = &self.program.node(node)?.behavior else {
            return Err(Error::InvalidGraph);
        };
        let sample = self
            .state
            .participant_providers
            .get(provider)
            .and_then(|participant| self.state.item_samples.get(&(source.clone(), *participant)))
            .and_then(Option::as_ref);
        for (field, value) in super::item_info::outputs(sample) {
            let port = Port { node, field };
            if self.bound(port) && self.state.store.read(port)?.as_ref() != Some(&value) {
                dispatch.publish(self, port, value)?;
            }
        }
        Ok(())
    }

    fn bind(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let behavior = self.program.node(node)?.behavior.clone();
        match behavior {
            Behavior::Native => dispatch.native(self, node, events::native::Call::Bind),
            Behavior::Constant { field, value } => {
                let port = Port { node, field };
                if self.bound(port) {
                    dispatch.initialize(
                        self,
                        port,
                        value,
                        super::properties::DefaultMode::Override,
                    )?;
                }
                Ok(())
            }
            Behavior::FloatSelect => self.float_select(node, true, dispatch),
            Behavior::RandomFloat { true_random } => self.random_float(node, true_random, dispatch),
            Behavior::ItemInfo { .. }
            | Behavior::ContextReporter { .. }
            | Behavior::ParticipantSetAccessor { .. } => {
                // Constructor-owned Boolean defaults precede registration.
                // Their provider subscriptions and refreshes run at +78.
                let field = match behavior {
                    Behavior::ItemInfo { .. } => super::item_info::VALID,
                    Behavior::ContextReporter { .. } => CONTEXT_MATCH,
                    _ => super::participant_sets::CONTAINS,
                };
                let port = Port { node, field };
                if self.bound(port) {
                    dispatch.initialize(
                        self,
                        port,
                        Value::Boolean(false),
                        super::properties::DefaultMode::Override,
                    )?;
                }
                Ok(())
            }
            Behavior::Burnout(_) => {
                for (field, value) in super::burnout::State::default().outputs() {
                    let port = Port { node, field };
                    if self.bound(port) {
                        dispatch.initialize(
                            self,
                            port,
                            value,
                            super::properties::DefaultMode::Override,
                        )?;
                    }
                }
                Ok(())
            }
            Behavior::VehicleTuning { .. } => {
                let port = Port {
                    node,
                    field: VEHICLE_TUNING,
                };
                if self.bound(port) {
                    dispatch.initialize(
                        self,
                        port,
                        Value::Float(5.0),
                        super::properties::DefaultMode::Override,
                    )?;
                }
                Ok(())
            }
            Behavior::ParticipantProvider => {
                self.state
                    .participant_providers
                    .insert(node, self.inputs.participant);
                Ok(())
            }
            Behavior::FloatSwitch(definition) => self.float_switch(
                node,
                &definition,
                super::float_switch::Action::Bind,
                dispatch,
            ),
            Behavior::Snapshot(values) => {
                for (field, value) in values {
                    dispatch.initialize(
                        self,
                        Port { node, field },
                        value,
                        super::properties::DefaultMode::Override,
                    )?;
                }
                Ok(())
            }
            Behavior::IntegerSum { count } => self.integer_sum(node, count, dispatch),
            Behavior::IntegerArithmetic { operations } => {
                self.integer_fold(node, &operations, dispatch)
            }
            Behavior::RankIntegers { count, .. } => self.integer_rank(node, count, dispatch),
            Behavior::Scalar(_) => {
                let effects = {
                    let state = self.state.scalars.get(&node).ok_or(Error::UnknownNode)?;
                    state.initial_outputs(|field| {
                        self.resolve(Port { node, field }).ok().flatten()
                    })?
                };
                for effect in effects {
                    if let nodes::Effect::Publish(field, value) = effect {
                        let port = Port { node, field };
                        if self.bound(port) {
                            dispatch.initialize(
                                self,
                                port,
                                value,
                                super::properties::DefaultMode::Override,
                            )?;
                        }
                    }
                }
                Ok(())
            }
            Behavior::Curve { .. } => self.curve(node, dispatch),
            Behavior::TransformSplitter => self.split_transform(node, dispatch),
            Behavior::IntegerHub { count } => {
                self.integer_hub(node, count, super::integer_hub::Action::Bind, dispatch)
            }
            Behavior::FloatHub { count } => {
                self.float_hub(node, count, super::integer_hub::Action::Bind, dispatch)
            }
            Behavior::RaceTime => {
                let port = Port {
                    node,
                    field: super::stopwatch::TIME,
                };
                if self.bound(port) {
                    dispatch.initialize(
                        self,
                        port,
                        Value::Float(0.0),
                        super::properties::DefaultMode::Override,
                    )?;
                }
                self.state.scheduled.push(node);
                self.race_time(node, dispatch)
            }
            Behavior::Gate { .. } => {
                dispatch.property_gate(self, node, property_gate::Action::Initialize)
            }
            Behavior::Delay(_) => dispatch.initialize_delay(self, node, None),
            Behavior::Stopwatch { time_bound: true } => {
                let port = Port {
                    node,
                    field: stopwatch::TIME,
                };
                dispatch
                    .initialize(
                        self,
                        port,
                        Value::Float(0.0),
                        super::properties::DefaultMode::Override,
                    )
                    .map(|_| ())
            }
            Behavior::Synced(_) => dispatch
                .synchronized(self, node, synced::Action::Bind)
                .map(|_| ()),
            Behavior::VehicleQuery(bindings) => self.query_initial(node, bindings.query, dispatch),
            Behavior::Collision(outputs) if outputs.querying => {
                let port = Port {
                    node,
                    field: collision::QUERYING,
                };
                dispatch
                    .initialize(
                        self,
                        port,
                        Value::Boolean(false),
                        super::properties::DefaultMode::Override,
                    )
                    .map(|_| ())
            }
            _ => Ok(()),
        }
    }
    fn activate(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let behavior = self.program.node(node)?.behavior.clone();
        match behavior {
            Behavior::Native => dispatch.native(self, node, events::native::Call::Activate),
            Behavior::Ai(definition) => self.ai_activate(node, definition, dispatch),
            Behavior::ItemInfo { .. } => {
                self.state.item_observers.push(node);
                self.item_info(node, dispatch)
            }
            Behavior::VehicleInfo => {
                self.state.vehicle_info_observers.push(node);
                for (field, value) in super::vehicle_info::outputs(None)? {
                    self.publish_bound(node, field, value, dispatch)?;
                }
                Ok(())
            }
            Behavior::BurnoutOverride => self.burnout_change(node, None),
            Behavior::ParticipantSetAdapter { .. }
            | Behavior::ParticipantSetPredicate { .. }
            | Behavior::ParticipantSetCounter { .. } => {
                if self.state.set_observers.contains(&node) {
                    return Err(Error::InvalidGraph);
                }
                self.state.set_observers.push(node);
                self.refresh_set(node, dispatch)
            }
            Behavior::ParticipantSetAccessor { set, provider } => {
                if self.state.set_observers.contains(&node) {
                    return Err(Error::InvalidGraph);
                }
                self.state.set_observers.push(node);
                if let (Some(set), Some(participant)) = (
                    set,
                    provider
                        .and_then(|p| self.state.participant_providers.get(&p))
                        .copied(),
                ) {
                    if dispatch.is_hosted() {
                        return dispatch.native(
                            self,
                            node,
                            events::native::Call::SetObserve { set, participant },
                        );
                    }
                    let member = self
                        .state
                        .participant_sets
                        .get(&set)
                        .is_some_and(|s| s.contains(&participant));
                    self.set_membership_output(node, member, dispatch)?;
                }
                Ok(())
            }
            Behavior::ContextReporter { .. } => {
                if self.state.context_observers.contains(&node) {
                    return Err(Error::InvalidGraph);
                }
                self.state.context_observers.push(node);
                // 04784650 attaches and refreshes BEFORE appending Categories.
                // Activation refresh therefore checks the empty list; later
                // notifications check the actual authored categories.
                self.context_activation(node, true, dispatch)
            }
            Behavior::ParticipantRankQuery { .. } => {
                if self.state.rank_observers.contains(&node) {
                    return Err(Error::InvalidGraph);
                }
                self.state.rank_observers.push(node);
                self.state.active_rank_queries.insert(node);
                Ok(())
            }
            Behavior::ParticipantRank {
                property,
                descending,
            } => {
                if self.state.bound_accessors.iter().any(|(id, _)| *id == node) {
                    return Err(Error::InvalidGraph);
                }
                self.state
                    .bound_accessors
                    .push((node, self.inputs.participant));
                self.participant_rank(
                    node,
                    self.inputs.participant,
                    &property,
                    descending,
                    dispatch,
                )
            }
            Behavior::ParticipantProperty {
                property,
                kind,
                write,
            } => {
                if self.state.bound_accessors.iter().any(|(id, _)| *id == node) {
                    return Err(Error::InvalidGraph);
                }
                self.state
                    .bound_accessors
                    .push((node, self.inputs.participant));
                // 046db7a0/046dae50/046db4d0 test readiness bit 6 before
                // writing. Connected but unready is a store-read path.
                let input = if write {
                    self.state.store.read(Port {
                        node,
                        field: nodes::FLOAT_VALUE,
                    })?
                } else {
                    None
                };
                if dispatch.is_hosted() {
                    dispatch.native(
                        self,
                        node,
                        events::native::Call::ParticipantObserve {
                            property: property.clone(),
                            participant: self.inputs.participant,
                            kind,
                            seed: input.is_none(),
                        },
                    )?;
                    if input.is_none() {
                        return Ok(());
                    }
                }
                if let Some(value) = input {
                    kind.validate(&value)?;
                    self.write_participant(
                        self.inputs.participant,
                        &property,
                        value,
                        Some(node),
                        dispatch,
                    )
                } else if let Some(value) = self
                    .state
                    .participant_values
                    .get(&(self.inputs.participant, property, kind))
                    .cloned()
                {
                    self.publish_bound(node, nodes::OUTPUT_VALUE, value, dispatch)
                } else {
                    Ok(())
                }
            }
            Behavior::EventDetails { source } => {
                if let Some(value) = source
                    .as_ref()
                    .and_then(|s| self.state.event_ids.get(s))
                    .copied()
                {
                    self.publish_bound(node, EVENT_ID, Value::Unsigned32(value), dispatch)?;
                }
                if let Some(value) = source
                    .as_ref()
                    .and_then(|s| self.state.event_strings.get(s))
                    .cloned()
                {
                    self.publish_bound(node, EVENT_ID_STRING, Value::Text(value), dispatch)?;
                }
                Ok(())
            }
            Behavior::GameId => {
                if let Some(game_id) = self.state.game_id {
                    self.publish_bound(node, GAME_ID, Value::Unsigned64(game_id), dispatch)?;
                }
                Ok(())
            }
            Behavior::VehicleTuning { .. } => {
                self.state.tuning_observers.push(node);
                Ok(())
            }
            Behavior::Scalar(_) => self.scalar(node, nodes::Action::Activate, dispatch),
            Behavior::Math {
                evaluate_on_event: false,
                ..
            } => self.math(node, dispatch),
            Behavior::Arithmetic { .. } => self.arithmetic(node, nodes::Action::Activate, dispatch),
            Behavior::Fold { .. } => self.fold(node, nodes::Action::Activate, dispatch),
            Behavior::VectorLength { .. } => {
                dispatch.vector_length(self, node, nodes::Action::Activate)
            }
            Behavior::VectorDot => self.vector_dot(node, nodes::Action::Activate, dispatch),
            Behavior::Cast(_) => dispatch
                .property_cast(self, node, property_cast::Action::Activate)
                .map(|_| ()),
            Behavior::ScoreContext { keepers, .. } => {
                self.state.contexts.insert(
                    node,
                    score::Context {
                        participant: self.inputs.participant,
                        keepers: keepers.iter().map(|k| u64::from(k.field)).collect(),
                    },
                );
                Ok(())
            }
            Behavior::VehicleQuery(bindings) => {
                self.state.scheduled.push(node);
                // Binding the owner immediately updates the physics query.
                if bindings.query == Query::Physics {
                    dispatch.vehicle_query(self, node)?;
                }
                Ok(())
            }
            Behavior::NosBuildup => {
                self.state.scheduled.push(node);
                self.buildup(node)
            }
            Behavior::Burnout(_) => {
                self.state.burnout_observers.push(node);
                self.state.scheduled.push(node);
                Ok(())
            }
            Behavior::Road { .. } | Behavior::Proximity | Behavior::Nos | Behavior::Donut(_) => {
                self.state.scheduled.push(node);
                Ok(())
            }
            Behavior::Stats => {
                // Figures are constant for the session: publish them at activation
                // and again only if the owned inputs change.
                self.state.scheduled.push(node);
                self.stats(node, dispatch)
            }
            _ => Ok(()),
        }
    }
    fn random_float(
        &mut self,
        node: u16,
        true_random: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let entity = *self
            .state
            .random_entities
            .get(&node)
            .ok_or(Error::MissingInput)?;
        let seed = if true_random {
            self.state
                .random_seeds
                .pop_front()
                .ok_or(Error::MissingInput)?
                .wrapping_add(entity)
        } else {
            self.state
                .random_context
                .ok_or(Error::MissingInput)?
                .seed(entity)
        };
        let float = |field| match self.resolve(Port { node, field })? {
            Some(Value::Float(value)) => Ok(value),
            None => Err(Error::MissingInput),
            _ => Err(Error::InvalidGraph),
        };
        let value = super::random_float::draw(
            seed,
            float(super::random_float::MIN)?,
            float(super::random_float::MAX)?,
        )?;
        self.publish_bound(
            node,
            super::random_float::OUT,
            Value::Float(value),
            dispatch,
        )
    }

    fn float_select(
        &mut self,
        node: u16,
        initialize: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let value = super::float_select::evaluate(|field| self.resolve(Port { node, field }))?;
        let port = Port {
            node,
            field: super::float_select::OUT,
        };
        if self.bound(port) {
            if initialize {
                dispatch.initialize(
                    self,
                    port,
                    Value::Float(value),
                    super::properties::DefaultMode::Override,
                )?;
            } else {
                dispatch.publish(self, port, Value::Float(value))?;
            }
        }
        Ok(())
    }

    fn integer_sum(
        &mut self,
        node: u16,
        count: u16,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let (field, value) =
            super::integers::sum(count, |field| self.resolve(Port { node, field }))?;
        if self.bound(Port { node, field }) {
            dispatch.publish(self, Port { node, field }, value)?;
        }
        Ok(())
    }
    fn integer_fold(
        &mut self,
        node: u16,
        operations: &[super::integers::Operation],
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let (field, value) =
            super::integers::fold(operations, |field| self.resolve(Port { node, field }))?;
        self.publish_bound(node, field, value, dispatch)
    }
    fn integer_rank(
        &mut self,
        node: u16,
        count: u16,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let mut values = Vec::with_capacity(usize::from(count));
        for slot in 0..count {
            values.push(
                match self.resolve(Port {
                    node,
                    field: super::integers::port("In", slot),
                })? {
                    Some(Value::Integer(v)) => v,
                    None => 0,
                    _ => return Err(Error::InvalidGraph),
                },
            );
        }
        let ranks = super::integers::rank(&values)?;
        for (slot, rank) in ranks.into_iter().enumerate() {
            let port = Port {
                node,
                field: super::integers::port("Out", slot as u16),
            };
            if self.bound(port) {
                dispatch.publish(self, port, Value::Integer(rank))?;
            }
        }
        Ok(())
    }
    fn vehicle(&self) -> Result<Vehicle, Error> {
        self.inputs.vehicle.ok_or(Error::MissingInput)
    }
}

impl Candidate<'_> {
    fn buildup(&mut self, node: u16) -> Result<(), Error> {
        use super::buildup::*;
        if self.inputs.vehicle.and_then(|v| v.nos).is_none() {
            return Ok(());
        }
        let boolean = |field| match self.resolve(Port { node, field })? {
            Some(Value::Boolean(v)) => Ok(v),
            _ => Err(Error::MissingInput),
        };
        let float = |field| match self.resolve(Port { node, field })? {
            Some(Value::Float(v)) => Ok(v),
            _ => Err(Error::MissingInput),
        };
        let counts = *self
            .state
            .buildup_counts
            .get(&node)
            .ok_or(Error::UnknownNode)?;
        let value = Buildup {
            drifting: boolean(DRIFTING)?,
            drafting: boolean(DRAFTING)?,
            oncoming: boolean(ONCOMING)?,
            airborne: boolean(AIRBORNE)?,
            speed: float(SPEED)?,
            pacing: float(PACING)?,
            near_misses: counts[0],
            cop_slams: counts[1],
        };
        self.state.buildup.merge(value)?;
        self.state.buildup_counts.insert(node, [0; 2]);
        Ok(())
    }
    /// Publish a scheduled node's output when its port has a typed cell.
    fn race_time(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        // RVA 0402b7a0 / 0402bfd0 read the monotonic clock and cvtsd2ss it.
        // This is not elapsed time since this logic was spawned.
        let value = Value::Float(finite(self.inputs.now as f32)?);
        let port = Port {
            node,
            field: super::stopwatch::TIME,
        };
        if self.bound(port) && self.state.store.read(port)?.as_ref() != Some(&value) {
            dispatch.publish(self, port, value)?;
        }
        Ok(())
    }

    fn split_transform(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        // Native getter requires a ready bound input; the authored transform is
        // not published when its input has never become ready (RVA 0376df10).
        let input = Port {
            node,
            field: 0x8752_5469,
        };
        if !self.bound(input) {
            return Ok(());
        }
        let transform = match self.state.store.read(input)? {
            Some(Value::Transform(value)) => value,
            None => return Ok(()),
            _ => return Err(Error::InvalidGraph),
        };
        let mut values = Vec::new();
        // All four are stored first, then notified Trans, Left, Up, Forward.
        for (field, index) in [
            (0x0d8a_a67f, 3),
            (0x7c83_f1be, 0),
            (0x0059_7380, 1),
            (0x7667_213e, 2),
        ] {
            let port = Port { node, field };
            if self.bound(port) {
                values.push((port, Value::Vec3(transform[index])));
            }
        }
        dispatch.publish_together(self, &values)
    }

    fn integer_hub(
        &mut self,
        node: u16,
        count: u16,
        action: super::integer_hub::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        use super::integer_hub::{self, Read};
        let value = integer_hub::evaluate(count, action, |field| {
            let port = Port { node, field };
            if !self.bound(port) {
                return Ok(Read::Unbound);
            }
            match self.state.store.read(port)? {
                Some(Value::Integer(value)) => Ok(Read::Ready(value)),
                None => Ok(Read::Unready),
                _ => Err(Error::InvalidGraph),
            }
        })?;
        if let Some(value) = value {
            self.publish_bound(node, nodes::OUT, Value::Integer(value), dispatch)?;
        }
        Ok(())
    }

    fn float_hub(
        &mut self,
        node: u16,
        count: u16,
        action: super::integer_hub::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        use super::integer_hub::{self, Action, Read};
        let selector = Port {
            node,
            field: integer_hub::SELECT,
        };
        let selector = if matches!(action, Action::Select(_)) || !self.bound(selector) {
            Read::Unbound
        } else {
            match self.state.store.read(selector)? {
                Some(Value::Integer(value)) => Read::Ready(value),
                None => Read::Unready,
                _ => return Err(Error::InvalidGraph),
            }
        };
        let value = integer_hub::evaluate_value(count, action, selector, 0.0, |field| {
            let port = Port { node, field };
            if !self.bound(port) {
                return Ok(Read::Unbound);
            }
            match self.state.store.read(port)? {
                Some(Value::Float(value)) => Ok(Read::Ready(value)),
                None => Ok(Read::Unready),
                _ => Err(Error::InvalidGraph),
            }
        })?;
        if let Some(value) = value {
            self.publish_bound(node, nodes::OUT, Value::Float(value), dispatch)?;
        }
        Ok(())
    }

    fn publish_bound(
        &mut self,
        node: u16,
        field: u32,
        value: Value,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let port = Port { node, field };
        if self.bound(port) {
            dispatch.publish(self, port, value)?;
        }
        Ok(())
    }
    /// Participant-proximity count of the other cars inside the bound radius.
    fn proximity(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let Some(vehicle) = self.inputs.vehicle else {
            return Ok(());
        };
        let radius = match self.resolve(Port {
            node,
            field: proximity::RADIUS,
        })? {
            Some(Value::Float(value)) => value,
            Some(_) => return Err(Error::InvalidGraph),
            None => return Err(Error::MissingInput),
        };
        let own = draft::Vehicle {
            identity: vehicle.identity,
            position: vehicle.transform[3],
            velocity: vehicle.velocity,
        };
        let count = proximity::count(own, &self.inputs.others, radius)?;
        let changed = self
            .state
            .proximity
            .get_mut(&node)
            .ok_or(Error::UnknownNode)?
            .update(count);
        if let Some(count) = changed {
            self.publish_bound(node, proximity::COUNT, Value::Integer(count), dispatch)?;
        }
        Ok(())
    }
    /// Vehicle figures, published when they first appear or change.
    fn stats(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let Some(figures) = self.inputs.stats else {
            return Ok(());
        };
        figures.validate()?;
        let last = self.state.stats.get_mut(&node).ok_or(Error::UnknownNode)?;
        if *last == Some(figures) {
            return Ok(());
        }
        *last = Some(figures);
        let top = i32::try_from(figures.top_speed_mph).map_err(|_| Error::ConversionRange)?;
        let stock_top =
            i32::try_from(figures.stock_top_speed_mph).map_err(|_| Error::ConversionRange)?;
        self.publish_bound(node, stats::TOP_SPEED, Value::Integer(top), dispatch)?;
        self.publish_bound(
            node,
            stats::STOCK_TOP_SPEED,
            Value::Integer(stock_top),
            dispatch,
        )?;
        self.publish_bound(
            node,
            stats::ZERO_TO_SIXTY,
            Value::Float(figures.zero_to_sixty),
            dispatch,
        )?;
        self.publish_bound(
            node,
            stats::STOCK_ZERO_TO_SIXTY,
            Value::Float(figures.stock_zero_to_sixty),
            dispatch,
        )
    }
    /// Nitrous transitions of the owned car.
    fn nos(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let Some(sample) = self.inputs.vehicle.and_then(|v| v.nos) else {
            return Ok(());
        };
        let effects = self
            .state
            .nos
            .get_mut(&node)
            .ok_or(Error::UnknownNode)?
            .step(sample)?;
        for effect in effects {
            match effect {
                nos::Effect::Publish(field, value) => {
                    self.publish_bound(node, field, Value::Float(value), dispatch)?;
                }
                nos::Effect::Emit(event) => dispatch.emit(self, Endpoint { node, event })?,
            }
        }
        Ok(())
    }
    /// Doughnut detection from the owned motion over `elapsed` seconds.
    fn donut(
        &mut self,
        node: u16,
        configuration: donut::Configuration,
        elapsed: f32,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let Some(vehicle) = self.inputs.vehicle else {
            return Ok(());
        };
        let [right, up, forward, _] = vehicle.transform;
        let sample = donut::Sample {
            basis: [right, up, forward],
            velocity: vehicle.velocity,
            angular_velocity: vehicle.angular_velocity,
        };
        let completed = self
            .state
            .donuts
            .get_mut(&node)
            .ok_or(Error::UnknownNode)?
            .step(configuration, sample, elapsed)?;
        if completed {
            dispatch.emit(
                self,
                Endpoint {
                    node,
                    event: donut::ON_DONUT_COMPLETED,
                },
            )?;
        }
        Ok(())
    }
}

impl Handler for Candidate<'_> {
    fn enabled(&self, node: u16) -> bool {
        !self.state.deactivated.contains(&node)
    }
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        self.deliver(target, None, dispatch)
    }
    fn handle_context(
        &mut self,
        target: Endpoint,
        context: EntityContext,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.deliver(target, Some(Context::Entity(context)), dispatch)
    }
    fn handle_integer(
        &mut self,
        target: Endpoint,
        value: i32,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.deliver(target, Some(Context::Integer(value)), dispatch)
    }
    fn handle_participant(
        &mut self,
        target: Endpoint,
        participant: u64,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.deliver(target, Some(Context::Participant(participant)), dispatch)
    }
}

impl Candidate<'_> {
    fn set_membership_output(
        &mut self,
        node: u16,
        member: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let port = Port {
            node,
            field: super::participant_sets::CONTAINS,
        };
        if self.bound(port) && self.state.store.read(port)? != Some(Value::Boolean(member)) {
            dispatch.publish(self, port, Value::Boolean(member))?;
        }
        Ok(())
    }
    fn change_set(
        &mut self,
        change: &super::participant_sets::Change,
        origin: Option<u16>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        use super::participant_sets::{MAX_MEMBERS, MAX_SETS};
        if let Some(node) = origin
            && dispatch.is_hosted()
        {
            return dispatch.native(self, node, events::native::Call::SetChange(change.clone()));
        }
        if !self.state.participant_sets.contains_key(&change.set)
            && self.state.participant_sets.len() >= MAX_SETS
        {
            return Err(Error::Bound);
        }
        let members = self
            .state
            .participant_sets
            .entry(change.set.clone())
            .or_default();
        if change.member && !members.contains(&change.participant) && members.len() >= MAX_MEMBERS {
            return Err(Error::Bound);
        }
        let changed = if change.member {
            members.insert(change.participant)
        } else {
            members.remove(&change.participant)
        };
        if !changed {
            return Ok(());
        }
        if origin.is_some() {
            if self.state.set_changes.len() >= MAX_OUTPUT {
                return Err(Error::Bound);
            }
            self.state.set_changes.push(change.clone());
        }
        self.notify_set(
            &super::participant_sets::Source::Channel(change.set.clone()),
            Some((change.participant, change.member)),
            dispatch,
        )
    }
    fn notify_set(
        &mut self,
        source: &super::participant_sets::Source,
        change: Option<(u64, bool)>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        use super::participant_sets::Source;
        for node in self.state.set_observers.clone() {
            if let Behavior::ParticipantSetAccessor {
                set: Some(set),
                provider: Some(provider),
            } = &self.program.node(node)?.behavior
                && source == &Source::Channel(set.clone())
                && let Some((participant, member)) = change
                && self.state.participant_providers.get(provider) == Some(&participant)
            {
                self.set_membership_output(node, member, dispatch)?;
            } else if matches!(&self.program.node(node)?.behavior,
                Behavior::ParticipantSetPredicate { operands,.. } if operands.contains(source))
                || matches!(&self.program.node(node)?.behavior,
                    Behavior::ParticipantSetCounter { source:Some(key) } if key==source)
            {
                self.refresh_set(node, dispatch)?;
            }
        }
        Ok(())
    }
    fn set_members(
        &self,
        source: &super::participant_sets::Source,
    ) -> std::collections::BTreeSet<u64> {
        match source {
            super::participant_sets::Source::Channel(key) => self.state.participant_sets.get(key),
            super::participant_sets::Source::Node(node) => self.state.local_sets.get(node),
        }
        .cloned()
        .unwrap_or_default()
    }
    fn refresh_set(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        use super::participant_sets::{MAX_MEMBERS, MAX_SETS, Source};
        let desired = match self.program.node(node)?.behavior.clone() {
            Behavior::ParticipantSetAdapter { source, provider } => source
                .zip(provider.and_then(|p| self.state.participant_providers.get(&p).copied()))
                .and_then(|key| self.state.scoped_sets.get(&key).cloned())
                .unwrap_or_default(),
            Behavior::ParticipantSetPredicate {
                operation,
                operands,
            } => {
                let operands = operands
                    .iter()
                    .map(|s| self.set_members(s))
                    .collect::<Vec<_>>();
                super::participant_sets::evaluate(operation, &operands)?
            }
            Behavior::ParticipantSetCounter { source } => {
                let count = source.as_ref().map_or(0, |s| self.set_members(s).len());
                return self.publish_bound(
                    node,
                    super::participant_sets::COUNT,
                    Value::Integer(count as i32),
                    dispatch,
                );
            }
            _ => return Err(Error::InvalidGraph),
        };
        if desired.len() > MAX_MEMBERS
            || (!self.state.local_sets.contains_key(&node)
                && self.state.local_sets.len() >= MAX_SETS)
        {
            return Err(Error::Bound);
        }
        let old = self.state.local_sets.entry(node).or_default().clone();
        // Adapter callbacks remove stale members before adding replacements.
        // Notify after each mutation, preserving intermediate counter values.
        for participant in old.difference(&desired).copied().collect::<Vec<_>>() {
            self.state
                .local_sets
                .get_mut(&node)
                .ok_or(Error::InvalidGraph)?
                .remove(&participant);
            self.notify_set(&Source::Node(node), Some((participant, false)), dispatch)?;
        }
        for participant in desired.difference(&old).copied().collect::<Vec<_>>() {
            self.state
                .local_sets
                .get_mut(&node)
                .ok_or(Error::InvalidGraph)?
                .insert(participant);
            self.notify_set(&Source::Node(node), Some((participant, true)), dispatch)?;
        }
        Ok(())
    }
    fn rank_query(
        &mut self,
        node: u16,
        sorter: &participant_properties::Property,
    ) -> Result<(), Error> {
        let index = match self.resolve(Port {
            node,
            field: super::rank_query::INDEX,
        })? {
            Some(Value::Integer(index)) => index,
            None => return Err(Error::MissingInput),
            _ => return Err(Error::InvalidGraph),
        };
        let row = self
            .state
            .rank_tables
            .get(sorter)
            .and_then(|rows| super::rank_query::select(rows, index));
        if self.state.rank_rows.get(&node).copied() == row {
            return Ok(());
        }
        if self.state.rank_updates.len() >= MAX_OUTPUT {
            return Err(Error::Bound);
        }
        if let Some(row) = row {
            self.state.rank_rows.insert(node, row);
        } else {
            self.state.rank_rows.remove(&node);
        }
        self.state
            .rank_updates
            .push(super::rank_query::Update { node, row });
        Ok(())
    }
    fn participant_rank(
        &mut self,
        node: u16,
        participant: u64,
        property: &participant_properties::Property,
        descending: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        // Native bind and both property-store callbacks require the local
        // participant to have a stored value. Absent competitors are not zero.
        if !self.state.participant_values.contains_key(&(
            participant,
            property.clone(),
            Kind::Integer,
        )) {
            return Ok(());
        }
        let mut values = self
            .state
            .participant_values
            .iter()
            .filter_map(|((id, p, kind), value)| {
                if p == property
                    && *kind == Kind::Integer
                    && let Value::Integer(v) = value
                {
                    return Some((*id, *v));
                }
                None
            })
            .collect::<Vec<_>>();
        if values.len() > usize::from(super::integers::MAX_RANK_INPUTS) {
            return Err(Error::Bound);
        }
        // Native small-set insertion sort is stable over the participant-key
        // tree traversal. Owned participant IDs replace native pointer keys.
        values.sort_by(|a, b| {
            if descending {
                b.1.cmp(&a.1)
            } else {
                a.1.cmp(&b.1)
            }
        });
        let rank = values
            .iter()
            .position(|(id, _)| *id == participant)
            .ok_or(Error::InvalidGraph)?
            + 1;
        // Native publishes Rank before Count (not one grouped notification).
        self.publish_bound(
            node,
            super::integers::RANK,
            Value::Integer(rank as i32),
            dispatch,
        )?;
        self.publish_bound(
            node,
            super::integers::COUNT,
            Value::Integer(values.len() as i32),
            dispatch,
        )
    }
    fn float_switch(
        &mut self,
        node: u16,
        definition: &super::float_switch::Definition,
        action: super::float_switch::Action,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let result = definition.evaluate(action, |field| {
            let port = Port { node, field };
            if self.bound(port) {
                self.state.store.read(port)
            } else {
                Ok(None)
            }
        })?;
        if let Some((value, emit)) = result {
            let port = Port {
                node,
                field: super::float_switch::OUTPUT,
            };
            if self.bound(port) {
                if action == super::float_switch::Action::Bind {
                    dispatch.initialize(
                        self,
                        port,
                        Value::Float(value),
                        super::properties::DefaultMode::Override,
                    )?;
                } else {
                    dispatch.publish(self, port, Value::Float(value))?;
                }
            }
            if emit {
                dispatch.emit(
                    self,
                    Endpoint {
                        node,
                        event: super::float_switch::SELECTED,
                    },
                )?;
            }
        }
        Ok(())
    }
    fn emit_with(
        &mut self,
        endpoint: Endpoint,
        context: Option<Context>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        match context {
            None => dispatch.emit(self, endpoint),
            Some(Context::Entity(c)) => dispatch.emit_context(self, endpoint, c),
            Some(Context::Integer(v)) => dispatch.emit_integer(self, endpoint, v),
            Some(Context::Participant(v)) => dispatch.emit_participant(self, endpoint, v),
        }
    }
    fn deliver(
        &mut self,
        target: Endpoint,
        context: Option<Context>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let node = self.program.node(target.node)?;
        if !self.enabled(target.node) {
            return Ok(());
        }
        if let Some(forward) = node.forward {
            return self.emit_with(
                Endpoint {
                    node: forward,
                    event: target.event,
                },
                context,
                dispatch,
            );
        }
        if !node.events {
            return Ok(());
        }
        let behavior = node.behavior.clone();
        let id = target.node;
        let event = target.event;
        match behavior {
            Behavior::Native => {
                dispatch.native(self, id, events::native::Call::Event { event, context })
            }
            Behavior::RandomFloat { true_random } => self.random_float(id, true_random, dispatch),
            Behavior::Ai(definition) => self.ai_event(id, event, context, definition, dispatch),
            Behavior::ParticipantSetAccessor {
                set: Some(set),
                provider: Some(provider),
            } => {
                let member = match event {
                    super::participant_sets::ADD => true,
                    super::participant_sets::REMOVE => false,
                    _ => return Ok(()),
                };
                if let Some(&participant) = self.state.participant_providers.get(&provider) {
                    self.change_set(
                        &super::participant_sets::Change {
                            set,
                            participant,
                            member,
                        },
                        Some(id),
                        dispatch,
                    )?;
                    self.set_membership_output(id, member, dispatch)?;
                }
                Ok(())
            }
            Behavior::Burnout(_) if event == super::burnout::RESET => {
                self.burnout_reset(id, dispatch)
            }
            Behavior::TeleportLocation {
                provider,
                location,
                manager,
            } if event == super::teleport_location::SET => {
                let participant = *self
                    .state
                    .participant_providers
                    .get(&provider)
                    .ok_or(Error::MissingInput)?;
                if self.state.teleport_locations.len() >= MAX_OUTPUT {
                    return Err(Error::Bound);
                }
                self.state
                    .teleport_locations
                    .push(super::teleport_location::Request {
                        node: id,
                        participant,
                        location,
                        manager,
                    });
                Ok(())
            }
            Behavior::ParticipantEvent { provider, filter } => {
                let Some(participant) = provider
                    .and_then(|id| self.state.participant_providers.get(&id))
                    .copied()
                else {
                    return Ok(());
                };
                let send = if filter {
                    context == Some(Context::Participant(participant))
                } else {
                    event == nodes::IN
                };
                if send {
                    dispatch.emit_participant(
                        self,
                        Endpoint {
                            node: id,
                            event: nodes::OUT,
                        },
                        participant,
                    )?;
                }
                Ok(())
            }
            Behavior::ParticipantTransition {
                provider,
                sources,
                target,
            } if event == super::transitions::TRIGGER => {
                let participant = *self
                    .state
                    .participant_providers
                    .get(&provider)
                    .ok_or(Error::MissingInput)?;
                self.transition_request(
                    super::transitions::Request {
                        node: id,
                        selection: super::transitions::Selection::FirstCompleted(participant),
                        sources,
                        target,
                    },
                    dispatch,
                )
            }
            Behavior::TriggeredTransition { sources, target } => {
                use super::transitions::{Request, Selection, TRIGGER, TRIGGER_ALL};
                let selection = match (event, context) {
                    (TRIGGER, Some(Context::Participant(participant))) => {
                        Selection::FirstCompleted(participant)
                    }
                    (TRIGGER_ALL, _) => Selection::AllMembers,
                    _ => return Ok(()),
                };
                self.transition_request(
                    Request {
                        node: id,
                        selection,
                        sources,
                        target,
                    },
                    dispatch,
                )
            }
            Behavior::ParticipantEventChannel if event == super::transitions::CHANNEL_IN => {
                let &(channel, participant) = self
                    .state
                    .event_channels
                    .get(&id)
                    .ok_or(Error::MissingInput)?;
                let change = super::transitions::ChannelEvent {
                    channel,
                    event: super::transitions::CHANNEL_OUT,
                    participant,
                };
                if dispatch.is_hosted() {
                    return dispatch.native(self, id, events::native::Call::ChannelEvent(change));
                }
                if self.state.channel_events.len() >= MAX_OUTPUT {
                    return Err(Error::Bound);
                }
                self.state.channel_events.push(change);
                Ok(())
            }
            Behavior::ParticipantName if event == nodes::IN => {
                if let Some(Context::Participant(participant)) = context {
                    if self.state.participant_names.len() >= MAX_OUTPUT {
                        return Err(Error::Bound);
                    }
                    self.state.participant_names.push((id, participant));
                }
                Ok(())
            }
            Behavior::Bank {
                bank,
                provider,
                kind,
            } if event == super::bank::TRANSACT => {
                let participant = *self
                    .state
                    .participant_providers
                    .get(&provider)
                    .ok_or(Error::MissingInput)?;
                let amount = match self.resolve(Port {
                    node: id,
                    field: super::bank::AMOUNT,
                })? {
                    Some(Value::Integer(amount)) => amount,
                    None => return Err(Error::MissingInput),
                    _ => return Err(Error::InvalidGraph),
                };
                if self.state.bank_pending.len() >= super::bank::MAX_PENDING
                    || self.state.bank_requests.len() >= MAX_OUTPUT
                {
                    return Err(Error::Bound);
                }
                let request = self.state.bank_serial.checked_add(1).ok_or(Error::Bound)?;
                self.state.bank_serial = request;
                self.state.bank_pending.insert(request, id);
                self.state.bank_requests.push(super::bank::Request {
                    id: request,
                    node: id,
                    bank,
                    participant,
                    kind,
                    amount,
                });
                Ok(())
            }
            Behavior::FloatSwitch(definition) => self.float_switch(
                id,
                &definition,
                super::float_switch::Action::Event(event),
                dispatch,
            ),
            Behavior::NosBuildup => {
                use super::buildup::{COP_SLAM, NEAR_MISS};
                if self.inputs.vehicle.and_then(|v| v.nos).is_some() {
                    let counts = self
                        .state
                        .buildup_counts
                        .get_mut(&id)
                        .ok_or(Error::UnknownNode)?;
                    let slot = match event {
                        NEAR_MISS => Some(0),
                        COP_SLAM => Some(1),
                        _ => None,
                    };
                    if let Some(slot) = slot {
                        counts[slot] = counts[slot].checked_add(1).ok_or(Error::Bound)?;
                    }
                }
                Ok(())
            }
            Behavior::IntegerHub { count } => {
                if let Some(index) =
                    (0..count).find(|&index| super::integers::port("SelectIn", index) == event)
                {
                    self.integer_hub(
                        id,
                        count,
                        super::integer_hub::Action::Select(index),
                        dispatch,
                    )?;
                }
                Ok(())
            }
            Behavior::FloatHub { count } => {
                if let Some(index) =
                    (0..count).find(|&index| super::integers::port("SelectIn", index) == event)
                {
                    self.float_hub(
                        id,
                        count,
                        super::integer_hub::Action::Select(index),
                        dispatch,
                    )?;
                }
                Ok(())
            }
            Behavior::Scalar(definition) => {
                // Gates and splitters forward the event they received, so a
                // contextual event keeps its context through them.
                let forwards = matches!(
                    definition,
                    nodes::Definition::Gate { .. }
                        | nodes::Definition::Splitter { .. }
                        | nodes::Definition::CompareIntegerRange { .. }
                        | nodes::Definition::Compare {
                            kind: nodes::Comparison::Unsigned64,
                            ..
                        }
                );
                if forwards && context.is_some() {
                    let effects = {
                        let reader =
                            |field: u32| self.resolve(Port { node: id, field }).ok().flatten();
                        let mut state = self
                            .state
                            .scalars
                            .get(&id)
                            .ok_or(Error::UnknownNode)?
                            .clone();
                        let effects = state.apply(nodes::Action::Event(event), reader)?;
                        self.state.scalars.insert(id, state);
                        effects
                    };
                    for effect in effects {
                        match effect {
                            nodes::Effect::Emit(out) => self.emit_with(
                                Endpoint {
                                    node: id,
                                    event: out,
                                },
                                context,
                                dispatch,
                            )?,
                            nodes::Effect::Publish(..) => return Err(Error::InvalidGraph),
                        }
                    }
                    Ok(())
                } else {
                    self.scalar(id, nodes::Action::Event(event), dispatch)
                }
            }
            Behavior::Math { .. } => self.math(id, dispatch),
            Behavior::RankIntegers { count, .. } if event == super::integers::SORT => {
                self.integer_rank(id, count, dispatch)
            }
            Behavior::Gate { .. } => {
                dispatch.property_gate(self, id, property_gate::Action::Event(event))
            }
            Behavior::Delay(_) => dispatch.delay_event(self, id, event, context),
            Behavior::Stopwatch { .. } => dispatch.stopwatch_event(self, id, event),
            Behavior::Synced(_) => dispatch
                .synchronized(self, id, synced::Action::Event(event))
                .map(|_| ()),
            Behavior::Score { .. } => dispatch.score(self, id, event),
            Behavior::Collision(_) => dispatch.collision(self, id, collision::Action::Event(event)),
            _ => Ok(()),
        }
    }
}

impl PropertyHandler for Candidate<'_> {
    fn native_update(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        if self.program.node(node)?.behavior != Behavior::Native {
            return Err(Error::InvalidGraph);
        }
        if !self.enabled(node) {
            return Ok(());
        }
        dispatch.native(self, node, events::native::Call::Update)
    }
    fn retire(&mut self, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        self.retire_owned(dispatch)
    }
    fn import(
        &mut self,
        value: events::native::Import,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.import_owned(value, dispatch)
    }
    fn properties(&mut self) -> &mut Store {
        &mut self.state.store
    }
    fn property_changed(
        &mut self,
        target: Port,
        _first: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let node = self.program.node(target.node)?;
        if !self.enabled(target.node) {
            return Ok(());
        }
        if !node.properties {
            return Ok(());
        }
        let id = target.node;
        let field = target.field;
        match node.behavior.clone() {
            Behavior::Native => dispatch.native(
                self,
                id,
                events::native::Call::Property {
                    field,
                    first: _first,
                },
            ),
            Behavior::ScaleClamp => {
                let value =
                    super::scale_clamp::evaluate(|field| self.resolve(Port { node: id, field }))?;
                self.publish_bound(id, super::scale_clamp::OUT, Value::Float(value), dispatch)
            }
            Behavior::FloatSelect => self.float_select(id, false, dispatch),
            Behavior::BurnoutOverride if self.state.burnout_overrides.contains_key(&id) => {
                self.burnout_change(id, Some(field))
            }
            Behavior::ContextReporter { .. }
                if field == CONTEXT_ACTIVE && self.state.context_observers.contains(&id) =>
            {
                self.context_activation(id, false, dispatch)
            }
            Behavior::ParticipantRankQuery { sorter }
                if self.state.active_rank_queries.contains(&id) =>
            {
                self.rank_query(id, &sorter)
            }
            Behavior::FloatSwitch(definition) => self.float_switch(
                id,
                &definition,
                super::float_switch::Action::Changed(field),
                dispatch,
            ),
            Behavior::ParticipantProperty {
                property,
                kind,
                write: true,
            } if field == nodes::FLOAT_VALUE
                && self
                    .state
                    .bound_accessors
                    .iter()
                    .any(|(node, _)| *node == id) =>
            {
                let value = self.accessor_value(id, kind)?;
                let participant = self
                    .state
                    .bound_accessors
                    .iter()
                    .find(|(node, _)| *node == id)
                    .map(|(_, participant)| *participant)
                    .ok_or(Error::InvalidGraph)?;
                self.write_participant(participant, &property, value, Some(id), dispatch)
            }
            Behavior::Scalar(_) => self.scalar(id, nodes::Action::PropertyChanged(field), dispatch),
            Behavior::TransformSplitter => self.split_transform(id, dispatch),
            Behavior::IntegerHub { count } => self.integer_hub(
                id,
                count,
                super::integer_hub::Action::Changed(field),
                dispatch,
            ),
            Behavior::FloatHub { count } => self.float_hub(
                id,
                count,
                super::integer_hub::Action::Changed(field),
                dispatch,
            ),
            Behavior::Math {
                evaluate_on_event: false,
                ..
            } => self.math(id, dispatch),
            Behavior::Curve { .. } | Behavior::LinkedCurve { .. } => self.curve(id, dispatch),
            Behavior::Arithmetic { .. } => {
                self.arithmetic(id, nodes::Action::PropertyChanged(field), dispatch)
            }
            Behavior::IntegerSum { count } => self.integer_sum(id, count, dispatch),
            Behavior::IntegerArithmetic { operations } => {
                self.integer_fold(id, &operations, dispatch)
            }
            Behavior::RankIntegers {
                count,
                on_change: true,
            } => self.integer_rank(id, count, dispatch),
            Behavior::Fold { .. } => self.fold(id, nodes::Action::PropertyChanged(field), dispatch),
            Behavior::VectorLength { .. } => {
                dispatch.vector_length(self, id, nodes::Action::PropertyChanged(field))
            }
            Behavior::VectorDot => {
                self.vector_dot(id, nodes::Action::PropertyChanged(field), dispatch)
            }
            Behavior::Cast(_) => dispatch
                .property_cast(self, id, property_cast::Action::PropertyChanged(field))
                .map(|_| ()),
            Behavior::Gate { .. } => {
                dispatch.property_gate(self, id, property_gate::Action::PropertyChanged(field))
            }
            Behavior::Synced(_) if field == synced::IN => dispatch
                .synchronized(self, id, synced::Action::InputChanged)
                .map(|_| ()),
            _ => Ok(()),
        }
    }
}

impl score::Handler for Candidate<'_> {
    fn score_binding(&self, node: u16) -> Result<score::Binding, Error> {
        match self.program.node(node)?.behavior {
            Behavior::Score { default, context } => Ok(score::Binding { context, default }),
            _ => Err(Error::InvalidGraph),
        }
    }
    fn score_input(&self, node: u16) -> Result<Option<Value>, Error> {
        match self.state.store.read(Port {
            node,
            field: score::SCORE,
        }) {
            Ok(value) => Ok(value),
            Err(Error::MissingInput) => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn score_context(&self, context: u16) -> Result<score::Context, Error> {
        if !matches!(
            self.program.node(context)?.behavior,
            Behavior::ScoreContext { .. }
        ) {
            return Err(Error::InvalidGraph);
        }
        Ok(self
            .state
            .contexts
            .get(&context)
            .cloned()
            .unwrap_or(score::Context {
                participant: self.inputs.participant,
                keepers: Vec::new(),
            }))
    }
    fn score_request(
        &mut self,
        request: score::Request,
        _: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        if self.state.awards.len() >= MAX_OUTPUT {
            return Err(Error::Bound);
        }
        self.state.awards.push(Award {
            keeper: u32::try_from(request.keeper).map_err(|_| Error::InvalidGraph)?,
            participant: request.participant,
            amount: request.amount,
        });
        Ok(())
    }
}

impl synced::Handler for Candidate<'_> {
    fn synchronized_node(&mut self, node: u16) -> Result<&mut synced::Node, Error> {
        self.state.synced.get_mut(&node).ok_or(Error::UnknownNode)
    }
    fn synchronized_input(&self, node: u16) -> Result<Option<Value>, Error> {
        match self.state.store.read(Port {
            node,
            field: synced::IN,
        }) {
            Ok(value) => Ok(value),
            Err(Error::MissingInput) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl collision::Handler for Candidate<'_> {
    fn collision_query(&mut self, node: u16) -> Result<&mut collision::Node, Error> {
        self.state
            .collisions
            .get_mut(&node)
            .ok_or(Error::UnknownNode)
    }
    fn collision_control(
        &mut self,
        node: u16,
        command: collision::Command,
        _: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        if self.state.commands.len() >= MAX_OUTPUT {
            return Err(Error::Bound);
        }
        self.state.commands.push((node, command));
        Ok(())
    }
}

impl road::Handler for Candidate<'_> {
    fn road_query(&mut self, node: u16) -> Result<&mut road::Node, Error> {
        self.state.roads.get_mut(&node).ok_or(Error::UnknownNode)
    }
}

impl stopwatch::Handler for Candidate<'_> {
    fn stopwatches(&mut self) -> &mut stopwatch::Manager {
        &mut self.state.stopwatches
    }
    fn stopwatch_inputs(&self, node: u16) -> Result<stopwatch::Inputs, Error> {
        let float = |field: u32| match self.resolve(Port { node, field })? {
            Some(Value::Float(value)) => Ok(value),
            Some(_) => Err(Error::InvalidGraph),
            None => Err(Error::MissingInput),
        };
        Ok(stopwatch::Inputs {
            multiplier: float(stopwatch::MULTIPLIER)?,
            trigger_time: float(stopwatch::TRIGGER_TIME)?,
            time_scale: self.inputs.time_scale,
        })
    }
}

impl delay::Handler for Candidate<'_> {
    type Context = Option<Context>;
    fn delays(&mut self) -> &mut delay::Manager<Self::Context> {
        &mut self.state.delays
    }
    fn delay_input(&self, node: u16) -> Result<f32, Error> {
        match self.resolve(Port {
            node,
            field: delay::DELAY,
        })? {
            Some(Value::Float(value)) => Ok(value),
            Some(_) => Err(Error::InvalidGraph),
            None => Err(Error::MissingInput),
        }
    }
    fn delay_scale(&self, _: u16) -> Result<f32, Error> {
        Ok(self.inputs.time_scale)
    }
    fn delay_output(
        &mut self,
        node: u16,
        context: Self::Context,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.emit_with(
            Endpoint {
                node,
                event: delay::OUT,
            },
            context,
            dispatch,
        )
    }
}

impl property_gate::Handler for Candidate<'_> {
    fn property_gate(&mut self, node: u16) -> Result<&mut property_gate::Gate, Error> {
        self.state.gates.get_mut(&node).ok_or(Error::UnknownNode)
    }
    fn property_gate_input(
        &self,
        node: u16,
        channel: property_gate::Channel,
    ) -> Result<Option<Value>, Error> {
        self.resolve(Port {
            node,
            field: channel.input(),
        })
    }
}

impl property_cast::Handler for Candidate<'_> {
    fn property_cast(&self, node: u16) -> Result<property_cast::Bindings, Error> {
        match self.program.node(node)?.behavior {
            Behavior::Cast(bindings) => Ok(bindings),
            _ => Err(Error::InvalidGraph),
        }
    }
}

impl vector_length::Handler for Candidate<'_> {
    fn vector_length(&mut self, node: u16) -> Result<&mut vector_length::Node, Error> {
        self.state.vectors.get_mut(&node).ok_or(Error::UnknownNode)
    }
    fn vector_length_input(&self, node: u16) -> Result<Option<Value>, Error> {
        match self.state.store.read(Port {
            node,
            field: vector_length::INPUT,
        }) {
            Ok(value) => Ok(value),
            Err(Error::MissingInput) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl vehicle_query::Handler for Candidate<'_> {
    type Vehicle = u32;
    fn vehicle_query(&self, node: u16) -> Result<vehicle_query::Bindings, Error> {
        match self.program.node(node)?.behavior {
            Behavior::VehicleQuery(bindings) => Ok(bindings),
            _ => Err(Error::InvalidGraph),
        }
    }
    fn query_vehicle(&mut self, _: u16) -> Result<Option<Self::Vehicle>, Error> {
        Ok(self.inputs.vehicle.map(|v| v.identity))
    }
    fn physics_measurement(
        &mut self,
        _: Self::Vehicle,
        field: u32,
    ) -> Result<Option<Value>, Error> {
        let vehicle = self.vehicle()?;
        Ok(match field {
            vehicle_query::VELOCITY => Some(Value::Vec3(vehicle.velocity)),
            vehicle_query::TRANSFORM => Some(Value::Transform(vehicle.transform)),
            vehicle_query::AIRBORNE => vehicle.airborne.map(Value::Boolean),
            vehicle_query::STATE_3C230E35 => vehicle.state_3c230e35.map(Value::Boolean),
            vehicle_query::STATE_CD5AE2F0 => vehicle.state_cd5ae2f0.map(Value::Boolean),
            vehicle_query::MAX_RPM => vehicle.max_rpm.map(Value::Float),
            vehicle_query::RPM => vehicle.rpm.map(Value::Float),
            _ => return Err(Error::InvalidGraph),
        })
    }
    fn drift_measurement(&mut self, _: Self::Vehicle) -> Result<DriftSample, Error> {
        let vehicle = self.vehicle()?;
        Ok(DriftSample {
            transform: vehicle.transform,
            velocity: vehicle.velocity,
        })
    }
    fn draft_measurement(
        &mut self,
        node: u16,
        _: Self::Vehicle,
    ) -> Result<draft::Measurement, Error> {
        let vehicle = self.vehicle()?;
        let mut values = [0.0f32; 5];
        for (value, field) in values.iter_mut().zip(draft::INPUTS) {
            *value = match self.resolve(Port { node, field })? {
                Some(Value::Float(v)) => v,
                Some(_) => return Err(Error::InvalidGraph),
                None => return Err(Error::MissingInput),
            };
        }
        let configuration = draft::Configuration {
            min_other_speed: values[0],
            max_delta_speed: values[1],
            cone_scale: values[2],
            min_draft_time: values[3],
            max_draft_time: values[4],
        };
        configuration.validate()?;
        configuration.evaluate(
            draft::Vehicle {
                identity: vehicle.identity,
                position: vehicle.transform[3],
                velocity: vehicle.velocity,
            },
            &self.inputs.others,
        )
    }
}

#[cfg(test)]
mod tests;
