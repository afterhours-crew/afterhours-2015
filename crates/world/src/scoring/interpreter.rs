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
        Context, Dispatch, Endpoint, EntityContext, Graph, Handler, PropertyHandler, RemoteEmission,
    },
    finite, nodes, nos,
    program::{Behavior, Program, Step},
    properties::{Definition, Port, Store},
    property_cast, property_gate, proximity,
    reports::Report,
    road, score, stats, stopwatch, synced,
    value::Value,
    vector_length,
    vehicle_query::{self, DriftSample, Query},
};
use crate::nitrous_fuel::Buildup;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const MAX_OUTPUT: usize = 4096;

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
}

impl Vehicle {
    /// Owned measurements from a decoded chassis report. Airborne, the two
    /// unlabeled state flags and engine speed are not replicated, so they are
    /// absent; the query preserves those outputs.
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
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
    pub buildup: Buildup,
    pub remote: Vec<RemoteEmission>,
    pub commands: Vec<(u16, collision::Command)>,
    pub awards: Vec<Award>,
    /// Synchronized nodes whose Out changed, with the current value.
    pub synced: Vec<(u16, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    store: Store,
    scalars: BTreeMap<u16, nodes::Node>,
    arithmetic: BTreeMap<u16, arithmetic::Node>,
    folds: BTreeMap<u16, boolean_fold::Node>,
    vectors: BTreeMap<u16, vector_length::Node>,
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
            store,
            scalars: BTreeMap::new(),
            arithmetic: BTreeMap::new(),
            folds: BTreeMap::new(),
            vectors: BTreeMap::new(),
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
        let candidate = Candidate {
            program: &self.program,
            inputs,
            state: self.state.clone(),
        };
        let (mut next, remote) = self.graph.transaction(&candidate, action)?;
        if next.state.awards.len() > MAX_OUTPUT
            || next.state.commands.len() > MAX_OUTPUT
            || remote.len() > MAX_OUTPUT
        {
            return Err(Error::Bound);
        }
        let awards = std::mem::take(&mut next.state.awards);
        let commands = std::mem::take(&mut next.state.commands);
        let mut synced = Vec::new();
        for (&node, state) in &mut next.state.synced {
            if state.take_dirty() {
                let value = next
                    .state
                    .store
                    .read(Port {
                        node,
                        field: synced::OUT,
                    })?
                    .ok_or(Error::MissingInput)?;
                synced.push((node, value));
            }
        }
        self.state = next.state;
        Ok(Output {
            buildup: std::mem::take(&mut self.state.buildup),
            remote,
            commands,
            awards,
            synced,
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
            for (index, step) in candidate.program.startup.iter().enumerate() {
                failed = Some(index);
                match step {
                    Step::Bind(node) => candidate.bind(*node, dispatch)?,
                    Step::Register(registration) => dispatch.register(candidate, *registration)?,
                    Step::CopyReady { source, target } => {
                        if let Some(value) = candidate.state.store.read(*source)? {
                            dispatch.initialize(
                                candidate,
                                *target,
                                value,
                                super::properties::DefaultMode::Override,
                            )?;
                        }
                    }
                    Step::Default { port, value, mode } => {
                        dispatch.initialize(candidate, *port, *value, *mode)?;
                    }
                    Step::Activate(node) => candidate.activate(*node, dispatch)?,
                }
            }
            failed = None;
            Ok(())
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

    /// One scheduler update: every active query at interval one, then the
    /// stopwatch pass, then the delay pass. `delta` is the elapsed time of this
    /// update in seconds.
    pub fn tick(&mut self, delta: f32, inputs: &Inputs) -> Result<Output, Error> {
        if !self.started {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |candidate, dispatch| {
            let scheduled = candidate.state.scheduled.clone();
            for node in scheduled {
                match &candidate.program.node(node)?.behavior {
                    Behavior::VehicleQuery(_) => dispatch.vehicle_query(candidate, node)?,
                    Behavior::Road { .. } => {
                        let frame = road::Frame {
                            now: candidate.inputs.now,
                            elapsed: delta,
                        };
                        dispatch.road(candidate, node, frame, candidate.inputs.road)?;
                    }
                    Behavior::Proximity => candidate.proximity(node, dispatch)?,
                    Behavior::Stats => candidate.stats(node, dispatch)?,
                    Behavior::Nos => candidate.nos(node, dispatch)?,
                    Behavior::NosBuildup => candidate.buildup(node)?,
                    Behavior::Donut(configuration) => {
                        let configuration = *configuration;
                        candidate.donut(node, configuration, delta, dispatch)?;
                    }
                    _ => return Err(Error::InvalidGraph),
                }
            }
            dispatch.update_stopwatches(candidate, delta)?;
            dispatch.update_delays(candidate, delta)
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
        let publication = report.publication();
        self.transaction(inputs, |candidate, dispatch| {
            match (&candidate.program.node(node)?.behavior, report) {
                (Behavior::Collision(_), Report::Collision { value, reference }) => dispatch
                    .collision(
                        candidate,
                        node,
                        collision::Action::Report {
                            value: *value,
                            context: EntityContext { object: *reference },
                        },
                    ),
                (Behavior::NearMiss, Report::NearMiss(_))
                | (Behavior::AccumulatedCollisions, Report::CollisionWeight(_)) => {
                    for (field, value) in &publication.properties {
                        let port = Port {
                            node,
                            field: *field,
                        };
                        if candidate.state.store.read(port).is_ok() {
                            dispatch.publish(candidate, port, *value)?;
                        }
                    }
                    dispatch.emit(
                        candidate,
                        Endpoint {
                            node,
                            event: publication.event,
                        },
                    )
                }
                _ => Err(Error::InvalidGraph),
            }
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
}

impl Candidate<'_> {
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
    fn curve(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let Behavior::Curve { curve, apply, .. } = &self.program.node(node)?.behavior else {
            return Err(Error::InvalidGraph);
        };
        let output = Port {
            node,
            field: nodes::OUTPUT,
        };
        if !self.bound(output) {
            return Ok(());
        }
        let value = if *apply {
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
            if let Some(value) = output.initial
                && self.bound(port)
            {
                dispatch.initialize(self, port, value, super::properties::DefaultMode::Override)?;
            }
        }
        Ok(())
    }
    fn bind(&mut self, node: u16, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        let behavior = self.program.node(node)?.behavior.clone();
        match behavior {
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
}

impl Candidate<'_> {
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
        }
    }
    fn deliver(
        &mut self,
        target: Endpoint,
        context: Option<Context>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let node = self.program.node(target.node)?;
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
            Behavior::Scalar(definition) => {
                // Gates and splitters forward the event they received, so a
                // contextual event keeps its context through them.
                let forwards = matches!(
                    definition,
                    nodes::Definition::Gate { .. } | nodes::Definition::Splitter { .. }
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
        if !node.properties {
            return Ok(());
        }
        let id = target.node;
        let field = target.field;
        match node.behavior.clone() {
            Behavior::Scalar(_) => self.scalar(id, nodes::Action::PropertyChanged(field), dispatch),
            Behavior::Math {
                evaluate_on_event: false,
                ..
            } => self.math(id, dispatch),
            Behavior::Curve { .. } => self.curve(id, dispatch),
            Behavior::Arithmetic { .. } => {
                self.arithmetic(id, nodes::Action::PropertyChanged(field), dispatch)
            }
            Behavior::Fold { .. } => self.fold(id, nodes::Action::PropertyChanged(field), dispatch),
            Behavior::VectorLength { .. } => {
                dispatch.vector_length(self, id, nodes::Action::PropertyChanged(field))
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
