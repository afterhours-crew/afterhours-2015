// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored event membership, Boolean latches, contexts and logic lifetimes.
//! The edge supplies sequence/AI/waypoint completion and commits effects with
//! its replicated objects. There is no sprint-specific phase or blueprint list.
use nfs_content::{
    events::lifecycle::{
        self as data,
        bindings::{
            BooleanOperation, Controller, IntegerSource, Link, Observer, PredicateKind, SetKind,
            SetSource,
        },
    },
    scoring::events::Identity,
};
use nfs_world::{
    participant_states::{
        self as states, Action, Error, Machine, Notification, Participant, StateId, contexts,
        properties,
    },
    scoring::{participant_properties::Property, participant_sets::Operation},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_OBSERVERS: usize = 1024;

fn resolve_endpoints(
    machine: &dyn nfs_world::participant_states::Membership,
    e: data::Endpoint,
    resolve: &mut impl FnMut(data::Endpoint) -> Result<Vec<StateId>, Error>,
) -> Result<Vec<data::Endpoint>, Error> {
    let ids = if e.port == 0 {
        vec![StateId::try_from(e.serializer).map_err(|_| Error::Bound)?]
    } else {
        resolve(e)?
    };
    if ids.len() > states::MAX_STATES {
        return Err(Error::Bound);
    }
    ids.into_iter()
        .map(|id| {
            machine.members(id)?;
            Ok(data::Endpoint {
                serializer: usize::from(id),
                port: 0,
            })
        })
        .collect()
}

pub mod accessors;
mod activation;
pub mod ai;
pub mod booleans;
pub mod channels;
pub mod commands;
pub use activation::SceneInputs;
pub mod dispatch;
pub mod distance;
pub mod execution;
pub mod floats;
pub mod graphs;
pub mod links;
mod listeners;
pub mod mirrors;
pub mod owner;
pub mod rankings;
pub mod reporters;
pub mod results;
pub mod root_updates;
pub mod scene_links;
pub mod set_accessors;
pub mod sets;
pub mod shared_properties;
pub mod timed;
pub mod updates;
pub mod values;
pub mod world;

#[derive(Clone, Debug)]
pub struct Plan {
    data: data::Scope,
    root: Option<data::Root>,
    external: BTreeMap<StateId, data::ImportedState>,
    properties: BTreeMap<Identity, usize>,
    predicates: BTreeMap<Identity, usize>,
    sets: BTreeMap<Identity, usize>,
}

impl Plan {
    /// Bind the installed scene endpoints to distinct membership aliases. The
    /// scene owner supplies their specialized hooks and RPC routing; they do
    /// not become ordinary automatically completing game-mode states.
    pub fn installed(authored: &data::Lifecycle) -> Result<Self, Error> {
        Self::installed_scope(&authored.scope(), Some(authored.root()))
    }

    /// Compile shared scene state and observers without a game-mode root.
    pub fn scene(authored: &data::Scope) -> Result<Self, Error> {
        Self::installed_scope(authored, None)
    }

    fn installed_scope(authored: &data::Scope, root: Option<data::Root>) -> Result<Self, Error> {
        let mut expanded = authored.clone();
        let mut external = BTreeMap::new();
        let mut endpoints = BTreeMap::new();
        let mut targets = BTreeMap::new();
        let mut next = authored
            .states
            .iter()
            .map(|s| s.serializer)
            .max()
            .unwrap_or(0);
        for (endpoint, imported) in &authored.imports {
            let mut aliases = Vec::new();
            for imported in imported {
                let key = (imported.scene_content_key, imported.state.serializer);
                let alias = if let Some(&alias) = targets.get(&key) {
                    if external.get(&alias) != Some(imported) {
                        return Err(Error::Invalid);
                    }
                    alias
                } else {
                    next = next.checked_add(1).ok_or(Error::Bound)?;
                    let alias = StateId::try_from(next).map_err(|_| Error::Bound)?;
                    let mut state = imported.state.clone();
                    state.serializer = next;
                    expanded.states.push(state);
                    targets.insert(key, alias);
                    external.insert(alias, imported.clone());
                    alias
                };
                aliases.push(alias);
            }
            endpoints.insert(*endpoint, aliases);
        }
        let mut plan = Self::new_many(&expanded, root, |e| {
            endpoints.get(&e).cloned().ok_or(Error::UnknownState)
        })?;
        plan.external = external;
        Ok(plan)
    }

    pub fn external_state(&self, state: StateId) -> Option<&data::ImportedState> {
        self.external.get(&state)
    }
    /// Resolve linked channel state ports explicitly. A channel's serializer
    /// number is never a state ID. Resolved states must belong to this machine.
    pub fn new(
        authored: &data::Lifecycle,
        mut resolve: impl FnMut(data::Endpoint) -> Result<StateId, Error>,
    ) -> Result<Self, Error> {
        Self::new_many(&authored.scope(), Some(authored.root()), |e| {
            resolve(e).map(|s| vec![s])
        })
    }

    fn new_many(
        authored: &data::Scope,
        root: Option<data::Root>,
        mut resolve: impl FnMut(data::Endpoint) -> Result<Vec<StateId>, Error>,
    ) -> Result<Self, Error> {
        let mut data = authored.clone();
        let machine = data.machine().map_err(|_| Error::Invalid)?;
        let mut endpoints = |es: &mut Vec<data::Endpoint>| -> Result<(), Error> {
            let mut result = Vec::new();
            for &e in es.iter() {
                result.extend(resolve_endpoints(&machine, e, &mut resolve)?);
                if result.len() > states::MAX_STATES {
                    return Err(Error::Bound);
                }
            }
            *es = result;
            Ok(())
        };
        for t in &mut data.transitions {
            endpoints(&mut t.sources)?;
        }
        for p in &mut data.bindings.contexts {
            endpoints(&mut p.states)?;
        }
        for p in &mut data.bindings.ai_controls {
            endpoints(&mut p.states)?;
        }
        for p in &mut data.bindings.reporters {
            endpoints(&mut p.states)?;
            let mut seen = BTreeSet::new();
            p.states.retain(|s| seen.insert(*s));
            if !p.compound && p.states.len() > 1 {
                return Err(Error::Invalid);
            }
        }
        for s in &mut data.bindings.sets {
            if let SetKind::States(es) = &mut s.kind {
                endpoints(es)?;
            }
        }
        for p in &mut data.bindings.properties {
            if let properties::Definition::List(es) = &mut p.definition {
                endpoints(es)?;
            }
        }
        let mut endpoint = |e: &data::Endpoint| -> Result<data::Endpoint, Error> {
            let resolved = resolve_endpoints(&machine, *e, &mut resolve)?;
            match resolved.as_slice() {
                [e] => Ok(*e),
                _ => Err(Error::Invalid),
            }
        };
        for t in &mut data.transitions {
            t.target = t.target.as_ref().map(&mut endpoint).transpose()?;
            if let data::TransitionConfiguration::AiFilter { human_target, .. } =
                &mut t.configuration
            {
                *human_target = human_target.as_ref().map(&mut endpoint).transpose()?;
            }
        }
        for distance in &mut data.bindings.distances {
            distance.state = endpoint(&distance.state)?;
            if !data.states.iter().any(|s| {
                s.serializer == distance.state.serializer && s.kind == data::StateKind::Waypoints
            }) {
                return Err(Error::Invalid);
            }
        }
        for command in data.bindings.ai_commands.values_mut() {
            if let data::bindings::ai::Command::Waypoints { state, .. }
            | data::bindings::ai::Command::Pacing { state, .. } = command
            {
                *state = endpoint(state)?;
                if !data.states.iter().any(|s| {
                    s.serializer == state.serializer && s.kind == data::StateKind::Waypoints
                }) {
                    return Err(Error::Invalid);
                }
            }
        }
        for p in &mut data.bindings.properties {
            if let properties::Definition::Range { begin, end } = &mut p.definition {
                *begin = endpoint(begin)?;
                *end = endpoint(end)?;
            }
        }
        let properties: BTreeMap<_, _> = data
            .bindings
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| (p.identity.clone(), i))
            .collect();
        let sets: BTreeMap<_, _> = data
            .bindings
            .sets
            .iter()
            .enumerate()
            .map(|(i, s)| (s.identity.clone(), i))
            .collect();
        let predicates: BTreeMap<_, _> = data
            .bindings
            .predicates
            .iter()
            .enumerate()
            .map(|(i, p)| (p.identity.clone(), i))
            .collect();
        if properties.len() != data.bindings.properties.len()
            || sets.len() != data.bindings.sets.len()
            || predicates.len() != data.bindings.predicates.len()
        {
            return Err(Error::Invalid);
        }
        let expected: BTreeSet<_> = (0..properties.len())
            .map(Observer::Property)
            .chain((0..data.bindings.ai_controls.len()).map(Observer::Ai))
            .chain((0..predicates.len()).map(Observer::Predicate))
            .chain((0..sets.len()).map(Observer::Set))
            .chain((0..data.bindings.set_counters.len()).map(Observer::SetCounter))
            .chain((0..data.bindings.contexts.len()).map(Observer::Context))
            .chain((0..data.bindings.reporters.len()).map(Observer::Reporter))
            .chain((0..data.bindings.sorters.len()).map(Observer::Sorter))
            .chain((0..data.bindings.distances.len()).map(Observer::Distance))
            .chain((0..data.bindings.mirrors.len()).map(Observer::Mirror))
            .chain((0..data.bindings.mirror_providers.len()).map(Observer::MirrorProvider))
            .chain((0..data.bindings.spawners.len()).map(Observer::Spawner))
            .chain(
                data.transitions
                    .iter()
                    .map(|t| Observer::Transition(t.serializer)),
            )
            .collect();
        if expected.len() > MAX_OBSERVERS
            || data.bindings.order.len() != expected.len()
            || data.bindings.order.iter().copied().collect::<BTreeSet<_>>() != expected
        {
            return Err(Error::Invalid);
        }
        let plan = Self {
            data,
            root,
            external: BTreeMap::new(),
            properties,
            predicates,
            sets,
        };
        if plan.data.bindings.integers.len() > 512
            || plan
                .data
                .bindings
                .integers
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != plan.data.bindings.integers.len()
        {
            return Err(Error::Bound);
        }
        for sorter in &plan.data.bindings.sorters {
            plan.validate_integer(&sorter.source)?;
        }
        for counter in &plan.data.bindings.set_counters {
            if let Some(source) = &counter.source {
                plan.validate_set(source)?;
            }
        }
        for query in &plan.data.bindings.rank_queries {
            if !plan
                .data
                .bindings
                .sorters
                .iter()
                .any(|s| s.identity == query.sorter)
            {
                return Err(Error::Invalid);
            }
        }
        for control in &plan.data.bindings.ai_controls {
            control
                .command_manager
                .validate()
                .map_err(|_| Error::Invalid)?;
            control
                .actor_manager
                .validate()
                .map_err(|_| Error::Invalid)?;
            if control.commands.len() > nfs_world::scoring::ai_commands::MAX_COMMANDS
                || control
                    .commands
                    .iter()
                    .any(|id| !plan.data.bindings.ai_commands.contains_key(id))
            {
                return Err(Error::Invalid);
            }
        }
        for distance in &plan.data.bindings.distances {
            plan.validate_integer(&distance.output)?;
        }
        for mirror in &plan.data.bindings.mirrors {
            plan.validate_integer(&IntegerSource::Local(mirror.identity.clone()))?;
            if let Some(manager) = &mirror.manager {
                plan.validate_mirror_manager(manager)?;
            }
        }
        for provider in &plan.data.bindings.mirror_providers {
            if let Some(source) = &provider.source {
                plan.validate_integer(source)?;
            }
            if let Some(manager) = &provider.manager {
                plan.validate_mirror_manager(manager)?;
            }
        }
        for result in &plan.data.bindings.results {
            if result.scores.len() > data::bindings::results::MAX_TAGS {
                return Err(Error::Bound);
            }
            for (_, source) in &result.scores {
                if let Some(source) = source {
                    plan.validate_integer(source)?;
                }
            }
        }
        for (id, key) in &plan.data.bindings.clocks {
            key.validate().map_err(|_| Error::Invalid)?;
            if !plan
                .data
                .states
                .iter()
                .any(|s| s.serializer == *id && s.kind == data::StateKind::Countdown)
                && !plan
                    .data
                    .transitions
                    .iter()
                    .any(|t| t.serializer == *id && t.kind == data::TransitionKind::Dnf)
            {
                return Err(Error::Invalid);
            }
        }
        for predicate in &plan.data.bindings.predicates {
            if let PredicateKind::Boolean { operands, .. } = &predicate.kind {
                if operands.len() > 32 {
                    return Err(Error::Bound);
                }
                for operand in operands {
                    plan.boolean_source(operand)?;
                }
            }
        }
        for s in &plan.data.bindings.sets {
            match &s.kind {
                SetKind::Boolean(link) => {
                    plan.boolean_source(link)?;
                }
                SetKind::Predicate {
                    operation,
                    operands,
                } => {
                    if operands.len() > 32
                        || *operation == Operation::Difference && operands.len() != 2
                    {
                        return Err(Error::Bound);
                    }
                    for operand in operands {
                        plan.validate_set(operand)?;
                    }
                }
                SetKind::Players { manager } => manager.validate().map_err(|_| Error::Invalid)?,
                _ => {}
            }
        }
        for s in &plan.data.bindings.spawners {
            match &s.controller {
                Some(Controller::Boolean(link)) => {
                    plan.boolean_source(link)?;
                }
                Some(Controller::Set(link)) => {
                    plan.set_source(link)?;
                }
                None => {}
            }
        }
        Ok(plan)
    }

    pub fn data(&self) -> &data::Scope {
        &self.data
    }

    pub fn root(&self) -> Option<&data::Root> {
        self.root.as_ref()
    }

    pub fn boolean_source(&self, link: &Link) -> Result<BooleanSource, Error> {
        if let Some(channel) = &link.channel {
            channel.validate().map_err(|_| Error::Invalid)?;
            Ok(BooleanSource::Channel(channel.clone()))
        } else if link.port == 0 {
            self.properties
                .get(&link.identity)
                .copied()
                .map(BooleanSource::Local)
                .or_else(|| {
                    self.predicates
                        .get(&link.identity)
                        .copied()
                        .map(BooleanSource::Predicate)
                })
                .ok_or(Error::Invalid)
        } else {
            Err(Error::Invalid)
        }
    }

    pub fn set_source(&self, link: &Link) -> Result<SetSource, Error> {
        let source = if let Some(channel) = &link.channel {
            SetSource::Channel(channel.clone())
        } else if link.port == 0 {
            SetSource::Local(link.identity.clone())
        } else {
            return Err(Error::Invalid);
        };
        self.validate_set(&source)?;
        Ok(source)
    }

    fn validate_set(&self, source: &SetSource) -> Result<(), Error> {
        match source {
            SetSource::Local(id) if self.sets.contains_key(id) => Ok(()),
            SetSource::Channel(p) => p.validate().map_err(|_| Error::Invalid),
            _ => Err(Error::Invalid),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum BooleanSource {
    Local(usize),
    Predicate(usize),
    Channel(Property),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Ai(ai::Effect),
    Distance(distance::Effect),
    Mirror(mirrors::Effect),
    Results(results::Snapshot),
    Ranking(rankings::Effect),
    Integer(rankings::Notification),
    BooleanCallback(booleans::Notification),
    Float(floats::Notification),
    SetCallback(sets::Notification),
    Timing(timed::Effect),
    Reporter(reporters::Effect),
    /// RPC/observation boundary, before registered observers of this callback.
    State(Notification),
    /// Specialized entry/exit hook, after registered observers. Plain states
    /// complete here automatically; sequence, countdown and waypoint hosts
    /// act on their authored configuration instead.
    Hook(Notification),
    Boolean {
        identity: Identity,
        change: properties::Change,
    },
    Set {
        source: SetSource,
        participant: Participant,
        member: bool,
    },
    Context {
        manager: Property,
        change: contexts::Change,
    },
    Logic {
        spawner: usize,
        participant: Participant,
        create: bool,
    },
}

#[derive(Clone, Debug)]
enum Work {
    Activate {
        observer: Observer,
        binding: bool,
    },
    RegisterSource {
        observer: Observer,
        sources: Vec<StateId>,
        next: usize,
    },
    SetCount(usize),
    Ai(ai::Work),
    Results {
        identity: Identity,
        event: u32,
        context: Option<nfs_world::scoring::events::Context>,
    },
    Distance(distance::Effect),
    Mirror(mirrors::Work),
    Ranking(rankings::Work),
    Reporter(reporters::Work),
    StateHook(Notification),
    Timing(timed::Work),
    Boolean(BooleanSource, Participant, bool),
    BooleanDispatch {
        source: BooleanSource,
        participant: Participant,
        value: bool,
        index: usize,
    },
    BooleanCallback(booleans::Notification),
    Float(floats::Work),
    Set(SetSource, Participant, bool),
    SetDispatch {
        source: SetSource,
        change: sets::Change,
        index: usize,
    },
    SetEmptyCheck(SetSource),
    SetCallback(sets::Notification),
    AssignSet(usize, Participant, bool),
    Recompute(usize, Participant),
    RecomputePredicate(usize, Participant),
    AssignPredicate(usize, Participant, bool),
    Spawner(usize, Participant, bool),
}

#[derive(Clone, Debug)]
struct Observers {
    ai_controls: ai::State,
    distance_routes: BTreeMap<usize, nfs_world::race_pacing::Route>,
    mirrors: mirrors::State,
    rankings: rankings::State,
    timed: timed::Timers,
    enabled: BTreeSet<Observer>,
    registering: BTreeMap<Observer, BTreeSet<StateId>>,
    properties: Vec<properties::Property>,
    predicates: Vec<BTreeMap<Participant, bool>>,
    contexts: Vec<contexts::StateContext>,
    managers: BTreeMap<Property, contexts::Manager>,
    sets: Vec<BTreeSet<Participant>>,
    set_listeners: sets::Listeners,
    logics: Vec<BTreeSet<Participant>>,
    booleans: BTreeMap<(Property, Participant), bool>,
    boolean_listeners: booleans::Listeners,
    floats: floats::State,
    channel_sets: BTreeMap<Property, BTreeSet<Participant>>,
    players: BTreeMap<Property, BTreeSet<Participant>>,
    ai: BTreeMap<Property, BTreeSet<Participant>>,
    pending: BTreeMap<u64, Work>,
    serial: u64,
}

#[derive(Clone, Debug)]
pub struct Host {
    plan: Arc<Plan>,
    machine: Machine,
    observers: Observers,
}

type Callback<'a> = dyn FnMut(&dyn nfs_world::participant_states::Membership, &Effect) -> Result<Vec<Action>, Error>
    + 'a;

impl Host {
    pub fn new(plan: Arc<Plan>) -> Result<Self, Error> {
        let machine = plan.data.machine().map_err(|_| Error::Invalid)?;
        let properties = plan
            .data
            .bindings
            .properties
            .iter()
            .map(|p| {
                properties::Property::new(
                    p.definition.try_map(|e| Ok(e.serializer as StateId))?,
                    &machine,
                )
            })
            .collect::<Result<_, Error>>()?;
        let contexts = plan
            .data
            .bindings
            .contexts
            .iter()
            .enumerate()
            .map(|(i, c)| {
                contexts::StateContext::new(
                    i as u64,
                    c.states.iter().map(|s| s.serializer as StateId).collect(),
                    c.categories.clone(),
                    &machine,
                )
            })
            .collect::<Result<_, _>>()?;
        let observers = Observers {
            ai_controls: ai::State::default(),
            distance_routes: BTreeMap::new(),
            mirrors: mirrors::State::default(),
            rankings: rankings::State::new(&plan),
            timed: timed::Timers::default(),
            enabled: plan.data.bindings.order.iter().copied().collect(),
            registering: BTreeMap::new(),
            properties,
            predicates: vec![BTreeMap::new(); plan.data.bindings.predicates.len()],
            contexts,
            managers: BTreeMap::new(),
            sets: vec![BTreeSet::new(); plan.data.bindings.sets.len()],
            set_listeners: sets::Listeners::new(&plan)?,
            logics: vec![BTreeSet::new(); plan.data.bindings.spawners.len()],
            booleans: BTreeMap::new(),
            boolean_listeners: booleans::Listeners::new(&plan)?,
            floats: floats::State::default(),
            channel_sets: BTreeMap::new(),
            players: BTreeMap::new(),
            ai: BTreeMap::new(),
            pending: BTreeMap::new(),
            serial: 0,
        };
        Ok(Self {
            plan,
            machine,
            observers,
        })
    }

    pub fn machine(&self) -> &Machine {
        &self.machine
    }
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
    pub fn logic_members(&self, spawner: usize) -> Result<&BTreeSet<Participant>, Error> {
        self.observers.logics.get(spawner).ok_or(Error::Invalid)
    }
    pub fn set_members(&self, identity: &Identity) -> Result<&BTreeSet<Participant>, Error> {
        Ok(&self.observers.sets[*self.plan.sets.get(identity).ok_or(Error::Invalid)?])
    }

    /// The callback must mutate only its own candidate state and commit after
    /// success. Host membership/lifetimes roll back together on any error.
    pub fn apply(
        &mut self,
        actions: &[Action],
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let mut next = self.clone();
        let output = next.execute(actions, callback)?;
        *self = next;
        Ok(output)
    }

    fn execute(
        &mut self,
        actions: &[Action],
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        self.execute_queued(actions, callback, None)
    }

    fn execute_queued(
        &mut self,
        actions: &[Action],
        callback: &mut Callback<'_>,
        commands: Option<&std::cell::RefCell<commands::Queue>>,
    ) -> Result<Vec<Effect>, Error> {
        let mut effects = Vec::new();
        let observers = &mut self.observers;
        let plan = &self.plan;
        self.machine.run_ordered(
            actions,
            plan.data.bindings.order.len() + 2,
            |machine, event, observer| {
                if let Notification::Invoked { token } = event {
                    if observer != 0 {
                        return Err(Error::Invalid);
                    }
                    if token & commands::COMMAND_BIT != 0 {
                        let command = commands.ok_or(Error::Invalid)?.borrow_mut().take(token)?;
                        return observers.command(plan, command);
                    }
                    let work = observers.pending.remove(&token).ok_or(Error::Invalid)?;
                    return observers.work(plan, work, machine, &mut effects, callback);
                }
                if observer == 0 {
                    return emit(machine, Effect::State(event), &mut effects, callback);
                }
                if observer == plan.data.bindings.order.len() + 1 {
                    let mut actions = emit(machine, Effect::Hook(event), &mut effects, callback)?;
                    actions.extend(observers.enqueue([Work::StateHook(event)])?);
                    return Ok(actions);
                }
                observers.observe(
                    plan,
                    plan.data.bindings.order[observer - 1],
                    event,
                    machine,
                    &mut effects,
                    callback,
                )
            },
        )?;
        if !observers.pending.is_empty() || commands.is_some_and(|queue| !queue.borrow().is_empty())
        {
            return Err(Error::Invalid);
        }
        Ok(effects)
    }

    pub fn boolean(
        &mut self,
        key: Property,
        participant: Participant,
        value: bool,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let mut next = self.clone();
        let actions = next.observers.write_boolean(key, participant, value)?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    /// Set inputs are scoped to this event. Only owned sets may be written;
    /// adapters and predicates change through their actual sources.
    pub fn set(
        &mut self,
        source: SetSource,
        participant: Participant,
        member: bool,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let mut next = self.clone();
        let actions = next
            .observers
            .write_set(&next.plan, source, participant, member)?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    pub fn players(
        &mut self,
        manager: Property,
        participant: Participant,
        present: bool,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        manager.validate().map_err(|_| Error::Invalid)?;
        let mut next = self.clone();
        if next.observers.players.len() >= 128 && !next.observers.players.contains_key(&manager) {
            return Err(Error::Bound);
        }
        if !assign(
            next.observers.players.entry(manager.clone()).or_default(),
            participant,
            present,
        )? {
            return Ok(vec![]);
        }
        let work = next.plan.data.bindings.order.iter().filter(|o| next.observers.enabled.contains(o)).filter_map(|o| match *o {
            Observer::Set(i) if matches!(&next.plan.data.bindings.sets[i].kind, SetKind::Players { manager: m } if *m == manager) => Some(Work::AssignSet(i, participant, present)),
            _ => None,
        }).collect::<Vec<_>>();
        let actions = next.observers.enqueue(work)?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    /// Manager membership is supplied before entry. AI readiness is a separate
    /// completion signal; changing membership does not invent one.
    pub fn ai_member(
        &mut self,
        manager: Property,
        participant: Participant,
        present: bool,
    ) -> Result<(), Error> {
        manager.validate().map_err(|_| Error::Invalid)?;
        if self.observers.ai.len() >= 128 && !self.observers.ai.contains_key(&manager) {
            return Err(Error::Bound);
        }
        assign(
            self.observers.ai.entry(manager).or_default(),
            participant,
            present,
        )?;
        Ok(())
    }

    /// Native TriggeredPredicate handles only these participant-context events.
    /// Unknown events and events without a participant are ignored.
    pub fn predicate_event(
        &mut self,
        identity: &Identity,
        event: u32,
        participant: Option<Participant>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let i = *self.plan.predicates.get(identity).ok_or(Error::Invalid)?;
        if self.plan.data.bindings.predicates[i].kind != PredicateKind::Triggered {
            return Err(Error::Invalid);
        }
        let (Some(participant), value) = (
            participant,
            match event {
                0xa425_3151 => true,
                0x27d4_023a => false,
                _ => return Ok(vec![]),
            },
        ) else {
            return Ok(vec![]);
        };
        let mut next = self.clone();
        let actions = next
            .observers
            .enqueue([Work::AssignPredicate(i, participant, value)])?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    /// Dispatch an authored UpdateSet. The native manager visits its linked
    /// sets at interval one; each state completes only unfinished members.
    pub fn update_set(
        &mut self,
        identity: &Identity,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let mut known = self
            .plan
            .data
            .bindings
            .update_managers
            .iter()
            .any(|m| m.sets.contains(identity));
        let mut actions = Vec::new();
        for state in &self.plan.data.states {
            if let data::StateConfiguration::Updateable { update_set } = &state.configuration
                && update_set == identity
            {
                known = true;
                actions.push(Action::CompleteAll {
                    state: state.serializer as StateId,
                    only_incomplete: true,
                });
            }
        }
        if !known {
            return Err(Error::Invalid);
        }
        self.apply(&actions, callback)
    }
}

fn assign(
    set: &mut BTreeSet<Participant>,
    participant: Participant,
    member: bool,
) -> Result<bool, Error> {
    if member {
        if set.len() >= states::MAX_PARTICIPANTS && !set.contains(&participant) {
            return Err(Error::Bound);
        }
        Ok(set.insert(participant))
    } else {
        Ok(set.remove(&participant))
    }
}

fn emit(
    machine: &dyn nfs_world::participant_states::Membership,
    effect: Effect,
    effects: &mut Vec<Effect>,
    callback: &mut Callback<'_>,
) -> Result<Vec<Action>, Error> {
    if effects.len() >= states::MAX_STEPS {
        return Err(Error::Bound);
    }
    let actions = callback(machine, &effect)?;
    effects.push(effect);
    Ok(actions)
}

impl Observers {
    fn boolean_value(&self, source: &BooleanSource, participant: Participant) -> bool {
        self.stored_boolean(source, participant).unwrap_or(false)
    }
    fn enqueue(&mut self, work: impl IntoIterator<Item = Work>) -> Result<Vec<Action>, Error> {
        let mut actions = Vec::new();
        for work in work {
            if self.pending.len() >= states::MAX_PENDING {
                return Err(Error::Bound);
            }
            self.serial = self.serial.checked_add(1).ok_or(Error::Bound)?;
            if self.serial >= 1 << 61 {
                return Err(Error::Bound);
            }
            self.pending.insert(self.serial, work);
            actions.push(Action::Invoke {
                observer: 0,
                token: self.serial,
            });
        }
        Ok(actions)
    }

    fn contains(&self, plan: &Plan, source: &SetSource, participant: Participant) -> bool {
        match source {
            SetSource::Local(id) => self.sets[plan.sets[id]].contains(&participant),
            SetSource::Channel(p) => self
                .channel_sets
                .get(p)
                .is_some_and(|s| s.contains(&participant)),
        }
    }

    fn observe(
        &mut self,
        plan: &Plan,
        observer: Observer,
        event: Notification,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        if !self.enabled.contains(&observer) {
            return Ok(vec![]);
        }
        if let Some(sources) = self.registering.get(&observer) {
            let source = match event {
                Notification::Entered { state, .. }
                | Notification::Completed { state, .. }
                | Notification::Leaving { state, .. }
                | Notification::Left { state, .. }
                | Notification::Existing { state, .. } => Some(state),
                Notification::Transitioned { source, .. } => Some(source),
                Notification::Invoked { .. } => None,
            };
            if source.is_some_and(|state| !sources.contains(&state)) {
                return Ok(vec![]);
            }
        }
        match observer {
            Observer::Ai(i) => {
                return self.enqueue(ai::observe(plan, i, event));
            }
            Observer::Reporter(i) => {
                return self.enqueue(reporters::observe(plan, i, event));
            }
            Observer::Property(i) => {
                let change = match event {
                    Notification::Existing { participant, .. } => {
                        self.properties[i].seed_member(participant)?
                    }
                    _ => self.properties[i].observe(event)?,
                };
                if let Some(change) = change {
                    let mut actions = emit(
                        machine,
                        Effect::Boolean {
                            identity: plan.data.bindings.properties[i].identity.clone(),
                            change,
                        },
                        effects,
                        callback,
                    )?;
                    actions.extend(self.enqueue([Work::Boolean(
                        BooleanSource::Local(i),
                        change.participant,
                        change.value,
                    )])?);
                    return Ok(actions);
                }
            }
            Observer::Context(i) => {
                let manager = &plan.data.bindings.contexts[i].manager;
                if let Some(change) = self.contexts[i]
                    .observe(self.managers.entry(manager.clone()).or_default(), event)?
                {
                    return emit(
                        machine,
                        Effect::Context {
                            manager: manager.clone(),
                            change,
                        },
                        effects,
                        callback,
                    );
                }
            }
            Observer::Set(i) => {
                if let SetKind::States(watched) = &plan.data.bindings.sets[i].kind {
                    let has = |s| watched.iter().any(|e| e.serializer == usize::from(s));
                    let value = match event {
                        Notification::Entered {
                            state, participant, ..
                        }
                        | Notification::Existing { state, participant }
                            if has(state) =>
                        {
                            Some((participant, true))
                        }
                        Notification::Transitioned {
                            source,
                            participant,
                            target,
                        } if has(source) && !target.is_some_and(has) => Some((participant, false)),
                        _ => None,
                    };
                    if let Some((participant, member)) = value
                        && assign(&mut self.sets[i], participant, member)?
                    {
                        return self.enqueue([Work::Set(
                            SetSource::Local(plan.data.bindings.sets[i].identity.clone()),
                            participant,
                            member,
                        )]);
                    }
                }
            }
            Observer::Transition(id) => {
                let transition = plan
                    .data
                    .transitions
                    .iter()
                    .find(|t| t.serializer == id)
                    .ok_or(Error::Invalid)?;
                let timing = self.timed.observe(plan, transition, event)?;
                let mut actions = Vec::new();
                for timing in timing {
                    actions.extend(emit(machine, Effect::Timing(timing), effects, callback)?);
                }
                if let Notification::Completed { state, participant } = event {
                    let t = plan
                        .data
                        .transitions
                        .iter()
                        .find(|t| t.serializer == id)
                        .ok_or(Error::Invalid)?;
                    if t.sources.iter().any(|s| s.serializer == usize::from(state)) {
                        let target = match (&t.kind, &t.configuration) {
                            (data::TransitionKind::Completed, _) => t.target,
                            (
                                data::TransitionKind::AiFilter,
                                data::TransitionConfiguration::AiFilter {
                                    manager,
                                    human_target,
                                },
                            ) => {
                                if self
                                    .ai
                                    .get(manager)
                                    .is_some_and(|s| s.contains(&participant))
                                {
                                    t.target
                                } else {
                                    *human_target
                                }
                            }
                            _ => return Ok(actions),
                        };
                        actions.push(Action::Move {
                            source: state,
                            participant,
                            target: target.map(|s| s.serializer as StateId),
                        });
                    }
                }
                return Ok(actions);
            }
            Observer::SetCounter(_)
            | Observer::Distance(_)
            | Observer::Spawner(_)
            | Observer::Predicate(_)
            | Observer::Sorter(_)
            | Observer::Mirror(_)
            | Observer::MirrorProvider(_) => {}
        }
        Ok(vec![])
    }

    fn work(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        let mut actions = Vec::new();
        let mut next = Vec::new();
        match work {
            Work::Activate { observer, binding } => {
                return self.activate_observer(plan, observer, binding);
            }
            Work::RegisterSource {
                observer,
                sources,
                next,
            } => {
                return self.register_source(plan, observer, sources, next);
            }
            Work::Ai(work) => return self.ai_work(plan, work, machine, effects, callback),
            Work::Results {
                identity,
                event,
                context,
            } => {
                return self
                    .result_work(plan, &identity, event, context, machine, effects, callback);
            }
            Work::Distance(effect) => {
                return emit(machine, Effect::Distance(effect), effects, callback);
            }
            Work::Mirror(work) => return self.mirror_work(plan, work, machine, effects, callback),
            Work::Ranking(work) => {
                return self.ranking_work(plan, work, machine, effects, callback);
            }
            Work::Reporter(work) => {
                return self.reporter_work(plan, work, machine, effects, callback);
            }
            Work::SetCount(i) => {
                let counter = &plan.data.bindings.set_counters[i];
                let count = match &counter.source {
                    Some(SetSource::Local(id)) => self.sets[plan.sets[id]].len(),
                    Some(SetSource::Channel(key)) => {
                        self.channel_sets.get(key).map_or(0, BTreeSet::len)
                    }
                    None => 0,
                };
                return emit(
                    machine,
                    Effect::Reporter(reporters::Effect::Count {
                        identity: counter.identity.clone(),
                        value: i32::try_from(count).map_err(|_| Error::Bound)?,
                    }),
                    effects,
                    callback,
                );
            }
            Work::Timing(work) => return self.timing_work(plan, work, machine, effects, callback),
            Work::StateHook(event) => {
                if let Notification::Entered {
                    state, participant, ..
                } = event
                {
                    if plan.external.contains_key(&state) {
                        return Ok(actions);
                    }
                    let definition = plan
                        .data
                        .states
                        .iter()
                        .find(|s| s.serializer == usize::from(state))
                        .ok_or(Error::UnknownState)?;
                    if let Some(timing) = self.timed.enter(plan, definition, machine)? {
                        actions.extend(emit(machine, Effect::Timing(timing), effects, callback)?);
                    }
                    let complete = match &definition.configuration {
                        data::StateConfiguration::WaitForAi { manager } => !self
                            .ai
                            .get(manager)
                            .is_some_and(|a| a.contains(&participant)),
                        _ => matches!(
                            definition.kind,
                            data::StateKind::Participant
                                | data::StateKind::GameMode
                                | data::StateKind::Simple
                        ),
                    };
                    if complete {
                        actions.push(Action::Complete { state, participant });
                    }
                }
            }
            Work::Boolean(source, participant, value) => {
                self.boolean_listeners.begin(&source);
                next.push(Work::BooleanDispatch {
                    source,
                    participant,
                    value,
                    index: 0,
                });
            }
            Work::BooleanDispatch {
                source,
                participant,
                value,
                index,
            } => {
                if let Some(work) = self
                    .boolean_listeners
                    .next(&source, participant, value, index)
                {
                    next.push(work);
                    next.push(Work::BooleanDispatch {
                        source,
                        participant,
                        value,
                        index: index + 1,
                    });
                }
            }
            Work::BooleanCallback(notification) => {
                actions.extend(emit(
                    machine,
                    Effect::BooleanCallback(notification),
                    effects,
                    callback,
                )?);
            }
            Work::Float(work) => {
                actions.extend(self.float_work(work, machine, effects, callback)?);
            }
            Work::Set(source, participant, member) => {
                self.set_listeners
                    .begin(&source, sets::Subscription::Membership);
                actions.extend(emit(
                    machine,
                    Effect::Set {
                        source: source.clone(),
                        participant,
                        member,
                    },
                    effects,
                    callback,
                )?);
                next.push(Work::SetDispatch {
                    source: source.clone(),
                    change: sets::Change::Membership {
                        participant,
                        member,
                    },
                    index: 0,
                });
                if !member {
                    // 04062860 checks current membership after removal callbacks.
                    next.push(Work::SetEmptyCheck(source));
                }
            }
            Work::SetDispatch {
                source,
                change,
                index,
            } => {
                if let Some(work) = self.set_listeners.next(&source, change, index) {
                    next.push(work);
                    next.push(Work::SetDispatch {
                        source,
                        change,
                        index: index + 1,
                    });
                }
            }
            Work::SetEmptyCheck(source) => {
                if self
                    .source_members(plan, &source)
                    .is_none_or(BTreeSet::is_empty)
                {
                    self.set_listeners.begin(&source, sets::Subscription::Empty);
                    next.push(Work::SetDispatch {
                        source,
                        change: sets::Change::Empty,
                        index: 0,
                    });
                }
            }
            Work::SetCallback(notification) => {
                actions.extend(emit(
                    machine,
                    Effect::SetCallback(notification),
                    effects,
                    callback,
                )?);
            }
            Work::AssignSet(i, participant, member) => {
                if assign(&mut self.sets[i], participant, member)? {
                    next.push(Work::Set(
                        SetSource::Local(plan.data.bindings.sets[i].identity.clone()),
                        participant,
                        member,
                    ));
                }
            }
            Work::Recompute(i, participant) => {
                let member = match &plan.data.bindings.sets[i].kind {
                    SetKind::Predicate {
                        operation,
                        operands,
                    } => {
                        let mut values =
                            operands.iter().map(|s| self.contains(plan, s, participant));
                        let first = values.next().unwrap_or(false);
                        match operation {
                            Operation::Union => first || values.any(|v| v),
                            Operation::Intersection => first && values.all(|v| v),
                            Operation::Difference => {
                                first && !values.next().ok_or(Error::Invalid)?
                            }
                        }
                    }
                    _ => return Err(Error::Invalid),
                };
                if assign(&mut self.sets[i], participant, member)? {
                    next.push(Work::Set(
                        SetSource::Local(plan.data.bindings.sets[i].identity.clone()),
                        participant,
                        member,
                    ));
                }
            }
            Work::Spawner(i, participant, create) => {
                if assign(&mut self.logics[i], participant, create)? {
                    actions.extend(emit(
                        machine,
                        Effect::Logic {
                            spawner: i,
                            participant,
                            create,
                        },
                        effects,
                        callback,
                    )?);
                }
            }
            Work::RecomputePredicate(i, participant) => {
                let PredicateKind::Boolean {
                    operation,
                    operands,
                } = &plan.data.bindings.predicates[i].kind
                else {
                    return Err(Error::Invalid);
                };
                let mut value = false;
                for (index, operand) in operands.iter().enumerate() {
                    let incoming = self.boolean_value(&plan.boolean_source(operand)?, participant);
                    value = if index == 0 {
                        if *operation == BooleanOperation::Not {
                            !incoming
                        } else {
                            incoming
                        }
                    } else {
                        match operation {
                            BooleanOperation::And => value && incoming,
                            BooleanOperation::Or => value || incoming,
                            BooleanOperation::Not => false,
                        }
                    };
                }
                next.push(Work::AssignPredicate(i, participant, value));
            }
            Work::AssignPredicate(i, participant, value) => {
                let values = &mut self.predicates[i];
                if values.get(&participant) != Some(&value) {
                    if values.len() >= states::MAX_PARTICIPANTS
                        && !values.contains_key(&participant)
                    {
                        return Err(Error::Bound);
                    }
                    values.insert(participant, value);
                    actions.extend(emit(
                        machine,
                        Effect::Boolean {
                            identity: plan.data.bindings.predicates[i].identity.clone(),
                            change: properties::Change { participant, value },
                        },
                        effects,
                        callback,
                    )?);
                    next.push(Work::Boolean(
                        BooleanSource::Predicate(i),
                        participant,
                        value,
                    ));
                }
            }
        }
        actions.extend(self.enqueue(next)?);
        Ok(actions)
    }
}

#[cfg(test)]
mod tests;
