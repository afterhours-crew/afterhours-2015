// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-owned intro modes. Layouts and transitions come from the installed
//! composition; the session stages these owners with replication and delivery.
use crate::{
    event_logic::{
        self,
        lifecycle::{Effect, Host, Plan, commands::Command},
    },
    sequence_content::authored::{Authored, Instances},
};
use nfs_content::{
    assets::AssetIndex,
    events::{Binding, intro, lifecycle::StateKind},
    scoring::{events::Identity, participant_properties::Property},
};
use nfs_world::{
    game_mode::{
        Arguments,
        authored::{Blueprint, Instance},
    },
    participant_states::{Action, Notification},
    participants::HostRpc,
    replication::{
        self, Error, Record,
        entity::{
            self,
            creation::{Asset, Catalog, Content as CreationContent},
        },
        players::Players,
        state::World,
    },
    sequences::{Completed, Owner},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
mod arrival;
mod garage;
pub(crate) mod opponents;
mod reporters;
#[cfg(test)]
pub(crate) mod tests;

#[derive(Clone, Debug)]
pub struct Mode {
    pub data: intro::Mode,
    parent: Asset,
    plan: Arc<Plan>,
    content: CreationContent,
    sequences: BTreeMap<usize, Authored>,
    timing: BTreeMap<usize, f64>,
    reporters: Option<reporters::Reporters>,
    opponents: Option<Arc<crate::event_ai::Content>>,
}
fn invalid<T>(_: T) -> Error {
    Error::Shape
}
impl Mode {
    pub fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        name: &str,
    ) -> Result<Self, nfs_content::Error> {
        Self::prepare_from(index, parent, name, None)
    }
    /// Reused modes keep the channels and route of their declaring story prefab.
    pub fn prepare_from(
        index: &mut AssetIndex,
        parent: Asset,
        name: &str,
        launcher: Option<&str>,
    ) -> Result<Self, nfs_content::Error> {
        let data = intro::extract(index, name, launcher)?;
        tracing::info!(
            mode = name,
            launch_event = ?data.launch_event,
            "intro mode prepared; a launch event means the client launches it"
        );
        let mut sequences = BTreeMap::new();
        let mut timing = BTreeMap::new();
        for s in &data.sequences {
            let countdown = data.game_mode.lifecycle.states.iter().any(|state| {
                Some(state.serializer) == s.spawner && state.kind == StateKind::Countdown
            });
            let stops = if countdown {
                vec![]
            } else {
                nfs_content::prologue::sequences::direct_stop_signals(index, &s.layout.blueprint)?
            };
            let deadline = if countdown {
                None
            } else {
                nfs_content::prologue::sequences::timing::extract(index, &s.layout.blueprint)?
            };
            if let Some(seconds) = deadline {
                timing.insert(
                    s.spawner.ok_or_else(|| {
                        nfs_content::Error::Content("timed sequence spawner".into())
                    })?,
                    seconds,
                );
            }
            if !countdown && stops.is_empty() && deadline.is_none() {
                return Err(nfs_content::Error::Content(format!(
                    "intro driving sequence {} completion signal",
                    s.layout.blueprint
                )));
            }
            let authored = Authored::from_mode(s, parent, data.game_mode.layout.asset, stops)
                .map_err(|e| nfs_content::Error::Content(format!("intro sequence: {e:?}")))?;
            if sequences
                .insert(
                    s.spawner.ok_or_else(|| {
                        nfs_content::Error::Content("intro sequence spawner".into())
                    })?,
                    authored,
                )
                .is_some()
            {
                return Err(nfs_content::Error::Content(
                    "duplicate intro sequence state".into(),
                ));
            }
        }
        let mut mode = Self::new(data, parent, sequences)
            .map_err(|e| nfs_content::Error::Content(format!("intro mode: {e:?}")))?;
        mode.timing = timing;
        mode.reporters = Some(reporters::Reporters::prepare(
            index, &mode.data, &mode.plan,
        )?);
        Ok(mode)
    }
    fn new(
        data: intro::Mode,
        parent: Asset,
        sequences: BTreeMap<usize, Authored>,
    ) -> Result<Self, Error> {
        let layout = &data.game_mode.layout;
        let entries = layout
            .catalog
            .iter()
            .map(|&(k, n)| {
                Ok((
                    u16::try_from(k).map_err(invalid)?,
                    u16::try_from(n).map_err(invalid)?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut profiles = vec![(layout.asset, layout.profile().map_err(invalid)?)];
        for child in &data.logics {
            if child.asset.bundle != layout.asset.bundle || child.catalog != layout.catalog {
                return Err(Error::Shape);
            };
            profiles.push((child.asset, child.profile().map_err(invalid)?));
        }
        let content =
            CreationContent::new(vec![Catalog::new(layout.asset.bundle, &entries)?], profiles)?;
        let plan = Arc::new(Plan::installed(&data.game_mode.lifecycle).map_err(invalid)?);
        Ok(Self {
            data,
            parent,
            plan,
            content,
            sequences,
            timing: BTreeMap::new(),
            reporters: None,
            opponents: None,
        })
    }
    fn parent(&self, world: &World, participant: u16) -> Result<u16, Error> {
        let found=world.iter().filter_map(|(id,o)|match &o.initial{
            replication::Initial::Entity{prefix,fields} if prefix.asset==self.parent && matches!(fields.first(),Some(entity::Initial::Root{reference,..}) if *reference==participant)=>Some(id),_=>None,
        }).collect::<Vec<_>>();
        match found.as_slice() {
            [id] => Ok(*id),
            [] => Err(Error::UnknownObject),
            _ => Err(Error::DuplicateObject),
        }
    }
    fn binding(
        &self,
        world: &World,
        parent: u16,
        mode: Option<u16>,
        b: Binding,
    ) -> Result<(u16, u16), Error> {
        match b {
            Binding::Scene {
                content_key,
                serializer,
            } => world
                .scene_endpoint(content_key, serializer)
                .ok_or(Error::UnknownObject),
            Binding::Participant { serializer } => field(world, parent, serializer),
            Binding::GameMode { serializer } => {
                field(world, mode.ok_or(Error::UnknownObject)?, serializer)
            }
        }
    }
    fn creation(
        &self,
        world: &World,
        owner: Owner,
        parent: u16,
        host: &Host,
        seconds: f64,
    ) -> Result<entity::creation::Creation, Error> {
        let d = &self.data.game_mode;
        let mut bindings = [(0, 0); 3];
        for (target, &(key, s)) in bindings.iter_mut().zip(&d.root_bindings) {
            *target = world.scene_endpoint(key, s).ok_or(Error::UnknownObject)?;
        }
        let members = d
            .lifecycle
            .states
            .iter()
            .filter_map(|s| {
                host.machine()
                    .members(s.serializer as u16)
                    .ok()
                    .filter(|m| m.contains(&u64::from(owner.participant)))
                    .map(|_| s.serializer)
            })
            .collect();
        let shape = Blueprint::new(
            d.layout.asset,
            d.layout.sub_id,
            d.layout.bus_count,
            &d.layout.fields.iter().map(|f| f.kind).collect::<Vec<_>>(),
        )?;
        shape.creation(
            Instance {
                bindings,
                participant: owner.participant,
                start_seconds: seconds,
                age_seconds: 0.,
                members,
                laps: d.laps,
                waypoints: d
                    .checkpoints
                    .iter()
                    .map(|&(k, s)| world.scene_endpoint(k, s).ok_or(Error::UnknownObject))
                    .collect::<Result<_, _>>()?,
            },
            |i| {
                self.binding(
                    world,
                    parent,
                    None,
                    d.layout.fields[i].binding.ok_or(Error::Shape)?,
                )
            },
        )
    }
}
fn field(world: &World, ghost: u16, serializer: usize) -> Result<(u16, u16), Error> {
    let replication::Initial::Entity { fields, .. } =
        &world.get(ghost).ok_or(Error::UnknownObject)?.initial
    else {
        return Err(Error::TypeMismatch);
    };
    Ok((
        ghost,
        fields
            .get(serializer)
            .and_then(entity::Initial::rpc)
            .ok_or(Error::TypeMismatch)?
            .selector,
    ))
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    /// Publish before every detach/deletion in this output.
    pub departures: Vec<HostRpc>,
    pub records: Vec<Record>,
    pub deleted: Vec<u16>,
    pub calls: Vec<HostRpc>,
}
#[derive(Clone, Debug)]
pub struct Active {
    owner: Owner,
    parent: u16,
    pub ghost: u16,
    host: Host,
    logics: BTreeMap<(usize, u16), u16>,
    sequences: BTreeMap<usize, Instances>,
    sequence_deadlines: BTreeMap<usize, f64>,
    goals: Vec<u16>,
    routes: BTreeMap<(Identity, u16), (u64, Option<u16>)>,
    pacing: Option<(Identity, BTreeMap<u16, u64>, crate::event_ai::Commands)>,
    opponents: Option<crate::event_ai::Racers>,
    opponent_deadlines: BTreeMap<(u16, u16), f64>,
    seconds: f64,
    progress: Option<(u16, nfs_world::race_progress::Race)>,
    car_id: Option<u32>,
    arrived: bool,
    ending: bool,
    left: bool,
    /// Membership hooks may complete synchronously, but the owner's final-to-
    /// dependency publications follow its native leave acknowledgement.
    shutdown_calls: Vec<HostRpc>,
    shutdown_barrier: bool,
    /// Local observations belong to this mode, even when they have no wire field.
    contexts: BTreeMap<(Property, u16), BTreeSet<u32>>,
    booleans: BTreeMap<(Identity, u16), bool>,
    reporters: BTreeMap<Identity, i32>,
    callbacks: Option<reporters::Reporters>,
    results: BTreeMap<u16, event_logic::lifecycle::results::Snapshot>,
    completion_queued: bool,
    result_barrier: bool,
    result_calls: Vec<HostRpc>,
    /// A client-enabled launcher's launch call, held until the client answers
    /// the launch event on the launcher (or the handshake deadline passes).
    pending_launch: Option<(HostRpc, f64)>,
}
/// Seconds the host waits for the client's launcher answer before launching
/// anyway. The official answer arrives within 50 ms (career start 891569 ms);
/// launching without it is logged as a handshake failure.
pub const LAUNCH_HANDSHAKE_SECONDS: f64 = 5.;
impl Active {
    fn dispatch(&mut self, commands: Vec<Command>) -> Result<Vec<Effect>, Error> {
        self.host
            .dispatch(commands, &mut |_, e| {
                self.callbacks
                    .as_mut()
                    .map_or_else(|| Ok(vec![]), |r| r.effect(e))
            })
            .map_err(invalid)
    }
    /// This mode's launcher on the owning participant logic (scene, selector).
    pub fn launcher_endpoint(
        &self,
        content: &Mode,
        players: &Players,
    ) -> Result<(u16, u16), Error> {
        field(players.objects(), self.parent, content.data.launcher)
    }
    /// The client answered the launch event on this mode's launcher: launch.
    /// `None` when the call is not that answer or nothing is pending.
    pub fn launch_request(
        &mut self,
        content: &Mode,
        players: &Players,
        ghost: u16,
        selector: u16,
        method: u32,
    ) -> Result<Option<Output>, Error> {
        if self.pending_launch.is_none() || method != 6 {
            return Ok(None);
        }
        if field(players.objects(), self.parent, content.data.launcher)? != (ghost, selector) {
            return Ok(None);
        }
        let (launch, _) = self.pending_launch.take().ok_or(Error::Shape)?;
        Ok(Some(Output {
            calls: vec![launch],
            ..Output::default()
        }))
    }
    /// A launch still waiting for the client's answer past its deadline goes
    /// out anyway, logged: the handshake failed, the drive must not hang.
    fn launch_overdue(&mut self, seconds: f64, out: &mut Output) {
        if let Some((_, deadline)) = &self.pending_launch
            && seconds >= *deadline
        {
            let (launch, _) = self.pending_launch.take().expect("checked above");
            tracing::warn!(
                mode = self.ghost,
                participant = self.owner.participant,
                "launcher answer missing; launched without the client's request"
            );
            out.calls.push(launch);
        }
    }
    /// Authored collector output is retained until the story owner consumes it.
    /// This snapshot alone does not grant rewards or complete an objective.
    pub fn result(&self, participant: u16) -> Option<&event_logic::lifecycle::results::Snapshot> {
        self.results.get(&participant)
    }
    /// Only the linked human reporter can advance this mode's story capability.
    pub fn pending_completion(
        &self,
        content: &Mode,
        participant: u16,
    ) -> Result<Option<u64>, Error> {
        if participant != self.owner.participant {
            return Err(Error::UnknownObject);
        }
        if self.completion_queued {
            return Ok(None);
        }
        let Some(binding) = &content.data.completion else {
            return Ok(None);
        };
        let Some(result) = self.results.get(&participant) else {
            return Ok(None);
        };
        if result.triggering_participant != u64::from(participant)
            || result.identity != binding.source
            || result.collector != binding.collector
        {
            return Err(Error::Shape);
        }
        Ok(Some(binding.objective))
    }
    pub(crate) fn mark_completion_queued(&mut self) {
        self.completion_queued = true;
    }
    /// The account owner calls this only after durable objective completion.
    pub fn completion_committed(
        &mut self,
        content: &Mode,
        participant: u16,
        objective: u64,
    ) -> Result<Vec<HostRpc>, Error> {
        if participant != self.owner.participant {
            return Err(Error::UnknownObject);
        }
        let binding = content.data.completion.as_ref().ok_or(Error::Unsupported)?;
        let result = self.results.get(&participant).ok_or(Error::Shape)?;
        if objective != binding.objective
            || result.identity != binding.source
            || result.collector != binding.collector
            || result.triggering_participant != u64::from(participant)
        {
            return Err(Error::Shape);
        }
        self.result_barrier = false;
        Ok(std::mem::take(&mut self.result_calls))
    }
    /// Bootstrap before publication: initial membership is included in creation.
    /// No transient entry/exit RPC is sent for a root the client does not own yet.
    pub fn start(
        content: &Mode,
        players: &mut Players,
        owner: Owner,
        seconds: f64,
    ) -> Result<(Self, Output), Error> {
        Self::start_bound(content, players, owner, seconds, None)
    }
    /// Modes without an arrival-sequence acknowledgement receive their selected
    /// vehicle identity from the participant's already validated inventory.
    pub fn start_with_car(
        content: &Mode,
        players: &mut Players,
        owner: Owner,
        seconds: f64,
        car_id: u32,
    ) -> Result<(Self, Output), Error> {
        Self::start_bound(content, players, owner, seconds, Some(car_id))
    }
    fn start_bound(
        content: &Mode,
        players: &mut Players,
        owner: Owner,
        seconds: f64,
        car_id: Option<u32>,
    ) -> Result<(Self, Output), Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        };
        let mut staged = players.clone();
        let parent = content.parent(staged.objects(), owner.participant)?;
        let mut host = Host::new(content.plan.clone()).map_err(invalid)?;
        host.clocks(event_logic::lifecycle::timed::Clocks {
            receipt_seconds: seconds,
            channels: content
                .plan
                .data()
                .bindings
                .clocks
                .values()
                .cloned()
                .map(|key| (key, seconds))
                .collect(),
        })
        .map_err(invalid)?;
        // The human is not a pending AI spawn. Native intro construction has
        // already advanced their membership when the mode is first published.
        let mut callbacks = content.reporters.clone();
        if let Some(callbacks) = &mut callbacks {
            callbacks.bind_human(owner.participant);
        }
        let mut actions = vec![Command::State(Action::Enter {
            state: content.data.game_mode.lifecycle.initial as u16,
            participant: owner.participant.into(),
        })];
        if let Some(callbacks) = &mut callbacks {
            actions.extend(callbacks.start(seconds).map_err(invalid)?);
        }
        let effects = host
            .dispatch(actions, &mut |_, e| {
                callbacks
                    .as_mut()
                    .map_or_else(|| Ok(vec![]), |r| r.effect(e))
            })
            .map_err(invalid)?;
        let creation = content.creation(staged.objects(), owner, parent, &host, seconds)?;
        let root = staged
            .spawn_entities(vec![creation], &content.content)?
            .remove(0);
        let mut out = Output {
            records: vec![root.clone()],
            ..Output::default()
        };
        let mut active = Self {
            owner,
            parent,
            ghost: root.id,
            host,
            logics: BTreeMap::new(),
            sequences: BTreeMap::new(),
            sequence_deadlines: BTreeMap::new(),
            goals: vec![],
            routes: BTreeMap::new(),
            pacing: None,
            opponents: None,
            opponent_deadlines: BTreeMap::new(),
            seconds,
            progress: None,
            car_id,
            arrived: false,
            ending: false,
            left: false,
            shutdown_calls: vec![],
            shutdown_barrier: content.data.game_mode.lifecycle.root_state.is_some(),
            contexts: BTreeMap::new(),
            booleans: BTreeMap::new(),
            reporters: BTreeMap::new(),
            callbacks,
            results: BTreeMap::new(),
            completion_queued: false,
            result_barrier: false,
            result_calls: vec![],
            pending_launch: None,
        };
        active.effects(content, &mut staged, &effects, true, &mut out)?;
        let (target, selector) = field(staged.objects(), parent, content.data.launcher)?;
        let launch = HostRpc::GameMode(event_logic::call(
            staged.objects(),
            target,
            selector,
            4,
            Some(active.ghost),
            Arguments::None,
        )?);
        match content.data.launch_event {
            // A client-enabled launcher is told to launch by its networked
            // event; the client answers on the launcher (method 6) and only
            // then does the host launch (career start 891523, 891569 and
            // 891923 ms). Server-started launchers launch at once.
            Some(event) => {
                out.calls.push(HostRpc::Event(nfs_world::logic::Fire {
                    target: nfs_world::logic::EntityRef {
                        ghost: parent,
                        entity: event.bus,
                    },
                    event: event.ordinal,
                    player: None,
                }));
                active.pending_launch = Some((launch, seconds + LAUNCH_HANDSHAKE_SECONDS));
            }
            None => out.calls.push(launch),
        }
        for &(key, s) in &content.data.game_mode.checkpoints {
            let (target, selector) = staged
                .objects()
                .scene_endpoint(key, s)
                .ok_or(Error::UnknownObject)?;
            out.calls.push(HostRpc::GameMode(event_logic::call(
                staged.objects(),
                target,
                selector,
                0,
                None,
                Arguments::None,
            )?));
        }
        *players = staged;
        Ok((active, out))
    }
    fn effects(
        &mut self,
        content: &Mode,
        players: &mut Players,
        effects: &[Effect],
        bootstrap: bool,
        out: &mut Output,
    ) -> Result<(), Error> {
        for effect in effects {
            match effect {
                Effect::State(n) => {
                    let call = match *n {
                        Notification::Entered {
                            state,
                            participant,
                            send_rpc: true,
                        } => Some((state, participant, 0)),
                        Notification::Left {
                            state,
                            participant,
                            send_rpc: true,
                        } => Some((state, participant, 1)),
                        _ => None,
                    };
                    if let Some((state, participant, method)) = call.filter(|(state, _, _)| {
                        !bootstrap && content.plan.external_state(*state).is_none()
                    }) {
                        let (target, selector) =
                            field(players.objects(), self.ghost, state.into())?;
                        let call = HostRpc::GameMode(event_logic::call(
                            players.objects(),
                            target,
                            selector,
                            method,
                            Some(u16::try_from(participant).map_err(invalid)?),
                            Arguments::None,
                        )?);
                        let human = participant == u64::from(self.owner.participant);
                        self.state_call(call, human, out)?;
                        if human
                            && method == 0
                            && usize::from(state) == content.data.game_mode.lifecycle.final_state
                        {
                            self.begin_finish(players, out)?;
                        }
                        // The root's state-entry observer runs after the state's
                        // entry RPC. Final membership removes an AI from the
                        // primary set; root-state entry registers its dependency.
                        if method == 0 {
                            let root_method = if !human
                                && usize::from(state)
                                    == content.data.game_mode.lifecycle.final_state
                            {
                                Some(3)
                            } else if Some(usize::from(state))
                                == content.data.game_mode.lifecycle.root_state
                            {
                                Some(4)
                            } else {
                                None
                            };
                            if let Some(method) = root_method {
                                let call = HostRpc::GameMode(event_logic::call(
                                    players.objects(),
                                    self.ghost,
                                    0,
                                    method,
                                    Some(self.member(players, participant)?),
                                    Arguments::None,
                                )?);
                                self.state_call(call, human, out)?;
                            }
                        }
                    }
                }
                Effect::Hook(Notification::Entered {
                    state, participant, ..
                }) => {
                    let participant = self.member(players, *participant)?;
                    if content.plan.external_state(*state).is_some() {
                        continue;
                    }
                    let data = content
                        .data
                        .game_mode
                        .lifecycle
                        .states
                        .iter()
                        .find(|s| s.serializer == usize::from(*state))
                        .ok_or(Error::UnknownObject)?;
                    match data.kind {
                        StateKind::Sequence | StateKind::Countdown => {
                            let s = content
                                .data
                                .sequences
                                .iter()
                                .find(|s| s.spawner == Some(usize::from(*state)))
                                .ok_or(Error::Unsupported)?;
                            if participant != self.owner.participant {
                                self.opponent_sequence(
                                    content,
                                    players,
                                    participant,
                                    *state,
                                    s,
                                    out,
                                )?;
                                continue;
                            }
                            let mut instances = Instances::default();
                            let record = instances
                                .start_bound(
                                    content
                                        .sequences
                                        .get(&usize::from(*state))
                                        .ok_or(Error::Unsupported)?,
                                    players,
                                    self.owner,
                                    Some(self.ghost),
                                )?
                                .ok_or(Error::Shape)?;
                            out.records.push(record);
                            if let Some(duration) = content.timing.get(&usize::from(*state)) {
                                self.sequence_deadlines
                                    .insert(usize::from(*state), self.seconds + duration);
                            }
                            if let Some(goto) = s.goto {
                                if !self.goals.is_empty() {
                                    return Err(Error::Shape);
                                };
                                let records = players.start_controlled_goals(
                                    self.owner.connection,
                                    self.owner.persona,
                                    self.owner.participant,
                                    event_logic::goto_goals(goto)?,
                                )?;
                                self.goals =
                                    records[..records.len() - 1].iter().map(|r| r.id).collect();
                                out.records.extend(records);
                            }
                            if self
                                .sequences
                                .insert(usize::from(*state), instances)
                                .is_some()
                            {
                                return Err(Error::DuplicateObject);
                            };
                        }
                        StateKind::Waypoints => {
                            if self.progress.is_none() {
                                self.progress = Some((
                                    *state,
                                    nfs_world::race_progress::Race::new(
                                        content.data.game_mode.checkpoints.len(),
                                        content.data.game_mode.laps,
                                        &self.members(),
                                        self.seconds,
                                    )?,
                                ));
                            } else if self.progress.as_ref().is_none_or(|(s, r)| {
                                s != state || r.participant(participant).is_none()
                            }) {
                                return Err(Error::Shape);
                            }
                            let (target, selector) =
                                field(players.objects(), self.ghost, (*state).into())?;
                            out.calls.push(HostRpc::GameMode(event_logic::call(
                                players.objects(),
                                target,
                                selector,
                                3,
                                Some(participant),
                                Arguments::Word(self.participant_car_id(content, participant)?),
                            )?));
                        }
                        StateKind::GameMode
                        | StateKind::Participant
                        | StateKind::Simple
                        | StateKind::WaitForAi => {}
                        _ => return Err(Error::Unsupported),
                    }
                }
                Effect::Hook(_) => {} // Completion/leave already changes Host membership; specialized entry is above.
                Effect::Logic {
                    spawner,
                    participant,
                    create,
                } => {
                    let participant = self.member(players, *participant)?;
                    if *create {
                        let layout = content
                            .data
                            .logics
                            .get(*spawner)
                            .ok_or(Error::Unsupported)?;
                        let creation = event_logic::logic_creation(
                            layout,
                            self.ghost,
                            participant,
                            |b| {
                                content.binding(players.objects(), self.parent, Some(self.ghost), b)
                            },
                            &|_, _| None,
                        )?;
                        let record = players
                            .spawn_entities(vec![creation], &content.content)?
                            .remove(0);
                        if self
                            .logics
                            .insert((*spawner, participant), record.id)
                            .is_some()
                        {
                            return Err(Error::DuplicateObject);
                        };
                        out.records.push(record);
                    } else if let Some(ghost) = self.logics.remove(&(*spawner, participant)) {
                        players.remove_entities(&[ghost])?;
                        out.deleted.push(ghost);
                    }
                }
                Effect::Context { manager, change } => {
                    let participant = self.member(players, change.participant)?;
                    self.contexts
                        .insert((manager.clone(), participant), change.categories.clone());
                }
                Effect::Boolean { identity, change } => {
                    let participant = self.member(players, change.participant)?;
                    self.booleans
                        .insert((identity.clone(), participant), change.value);
                }
                Effect::Reporter(event_logic::lifecycle::reporters::Effect::Count {
                    identity,
                    value,
                }) => {
                    self.reporters.insert(identity.clone(), *value);
                }
                Effect::Reporter(event_logic::lifecycle::reporters::Effect::Event { .. }) => {} // Dispatched synchronously by the Host callback before its next observer.
                Effect::Ai(event_logic::lifecycle::ai::Effect::Event { .. }) => {} // Same synchronous graph dispatch as reporter events.
                Effect::Ai(effect) => self.ai(content, players, effect, out)?,
                Effect::Timing(event_logic::lifecycle::timed::Effect::CountdownDeadline {
                    state,
                    deadline,
                }) => {
                    let (target, selector) = field(players.objects(), self.ghost, (*state).into())?;
                    out.calls.push(HostRpc::GameMode(event_logic::call(
                        players.objects(),
                        target,
                        selector,
                        2,
                        None,
                        Arguments::time(*deadline),
                    )?));
                }
                Effect::Timing(event_logic::lifecycle::timed::Effect::CountdownStop { state }) => {
                    let instances = self
                        .sequences
                        .get_mut(&usize::from(*state))
                        .ok_or(Error::UnknownObject)?;
                    if let Some(stop) = instances.stop(
                        content
                            .sequences
                            .get(&usize::from(*state))
                            .ok_or(Error::Unsupported)?,
                        players,
                        self.owner,
                    )? {
                        out.calls.push(HostRpc::SequenceStop(stop));
                    }
                }
                Effect::Timing(event_logic::lifecycle::timed::Effect::CountdownEvent {
                    ..
                }) => {} // Dispatched by the graph callback.
                Effect::Timing(event_logic::lifecycle::timed::Effect::DnfDeadline {
                    transition,
                    participant,
                    deadline,
                    ..
                }) => {
                    let participant = self.member(players, *participant)?;
                    let (target, selector) = field(players.objects(), self.ghost, *transition)?;
                    out.calls.push(HostRpc::GameMode(event_logic::call(
                        players.objects(),
                        target,
                        selector,
                        0,
                        Some(participant),
                        Arguments::time(*deadline),
                    )?));
                }
                Effect::Timing(event_logic::lifecycle::timed::Effect::Finish {
                    participant,
                    ..
                }) => {
                    self.member(players, *participant)?;
                    if *participant == u64::from(self.owner.participant) {
                        self.arrived = true;
                    }
                }
                Effect::Timing(event_logic::lifecycle::timed::Effect::Dnf {
                    transition,
                    participant,
                    ..
                }) => {
                    let p = self.member(players, *participant)?;
                    self.progress.as_mut().ok_or(Error::Shape)?.1.withdraw(p);
                    let (target, selector) = field(players.objects(), self.ghost, *transition)?;
                    out.calls.push(HostRpc::GameMode(event_logic::call(
                        players.objects(),
                        target,
                        selector,
                        1,
                        Some(p),
                        Arguments::None,
                    )?));
                }
                Effect::Results(result) => {
                    let participant = self.member(players, result.triggering_participant)?;
                    if participant == self.owner.participant
                        && content.data.completion.is_some()
                        && !self.results.contains_key(&participant)
                    {
                        self.result_barrier = true;
                    }
                    if let Some(previous) = self.results.insert(participant, result.clone())
                        && previous != *result
                    {
                        return Err(Error::Shape);
                    }
                }
                _ => return Err(Error::Unsupported),
            }
        }
        // Final state can be reached by a scene callback, a clock transition,
        // or sequence completion. All paths publish the same native shutdown.
        self.control_updates(players, out)?;
        self.finish_if_final(content, players, out)
    }
    fn ai(
        &mut self,
        content: &Mode,
        players: &mut Players,
        effect: &event_logic::lifecycle::ai::Effect,
        out: &mut Output,
    ) -> Result<(), Error> {
        use event_logic::lifecycle::ai::Effect;
        use nfs_content::events::lifecycle::bindings::ai::Command;
        let changes = match effect {
            Effect::Start {
                identity,
                participant,
                token,
                commands,
                ..
            } => {
                let participant_id = self.member(players, *participant)?;
                if self
                    .routes
                    .contains_key(&(identity.clone(), participant_id))
                {
                    return Err(Error::Shape);
                }
                let Some(command) = commands.first() else {
                    return Err(Error::Unsupported);
                };
                let Some(Command::Waypoints {
                    state,
                    route,
                    drift: false,
                }) = content
                    .data
                    .game_mode
                    .lifecycle
                    .bindings
                    .ai_commands
                    .get(command)
                else {
                    return Err(Error::Unsupported);
                };
                if state.port != 0
                    || route.class != "ChannelWaypointListProviderEntityData"
                    || route.port != 0
                    || !self
                        .host
                        .machine()
                        .members(state.serializer as u16)
                        .map_err(invalid)?
                        .contains(participant)
                {
                    return Err(Error::Shape);
                }
                if let [_, pacing] = commands.as_slice() {
                    let Some(Command::Pacing {
                        state: pacing_state,
                        route: pacing_route,
                        ..
                    }) = content
                        .data
                        .game_mode
                        .lifecycle
                        .bindings
                        .ai_commands
                        .get(pacing)
                    else {
                        return Err(Error::Unsupported);
                    };
                    if pacing_state != state
                        || pacing_route != route
                        || content.data.game_mode.pacing.is_none()
                    {
                        return Err(Error::Shape);
                    }
                    if let Some((owned, tokens, _)) = &mut self.pacing {
                        if owned != identity || tokens.insert(participant_id, *token).is_some() {
                            return Err(Error::Shape);
                        }
                        return Ok(());
                    }
                    let (command, changes) = self.start_race_commands(content, players)?;
                    self.pacing = Some((
                        identity.clone(),
                        BTreeMap::from([(participant_id, *token)]),
                        command,
                    ));
                    out.records.extend(changes.records);
                    out.deleted.extend(changes.deleted);
                    return Ok(());
                }
                if commands.len() != 1 || participant_id != self.owner.participant {
                    return Err(Error::Unsupported);
                }
                let changes = players.start_player_route(
                    self.owner.connection,
                    self.owner.persona,
                    self.owner.participant,
                    event_logic::route_goal(&content.data.game_mode)?,
                    None,
                )?;
                self.routes.insert(
                    (identity.clone(), participant_id),
                    (*token, Some(changes.goals[0])),
                );
                changes
            }
            Effect::Release {
                identity,
                participant,
                token,
            } => {
                let participant_id = self.member(players, *participant)?;
                if let Some((id, tokens, command)) = &mut self.pacing
                    && id == identity
                {
                    if tokens.get(&participant_id) != Some(token) {
                        return Err(Error::UnknownObject);
                    }
                    let mut changes = command.withdraw(players, participant_id)?;
                    tokens.remove(&participant_id);
                    if tokens.is_empty() {
                        let cleared = command.clear(players)?;
                        changes.records.extend(cleared.records);
                        changes.deleted.extend(cleared.deleted);
                        self.pacing = None;
                    }
                    out.records.extend(changes.records);
                    out.deleted.extend(changes.deleted);
                    return Ok(());
                }
                let key = (identity.clone(), participant_id);
                let &(owned, goal) = self.routes.get(&key).ok_or(Error::UnknownObject)?;
                if owned != *token {
                    return Err(Error::UnknownObject);
                }
                let changes = if let Some(goal) = goal {
                    players.release_player_route(
                        self.owner.connection,
                        self.owner.persona,
                        self.owner.participant,
                        goal,
                        None,
                    )?
                } else {
                    nfs_world::replication::players::ai::GoalChanges::default()
                };
                self.routes.remove(&key);
                changes
            }
            Effect::Event { .. } => return Err(Error::Unsupported),
        };
        out.records.extend(changes.records);
        out.deleted.extend(changes.deleted);
        Ok(())
    }
    pub fn signal(
        &mut self,
        content: &Mode,
        players: &Players,
        message: &nfs_world::logic::Message,
    ) -> Result<Option<Vec<HostRpc>>, Error> {
        let nfs_world::logic::Message::Reached { target, .. } = message else {
            return Ok(None);
        };
        for (&state, instances) in &mut self.sequences {
            if instances.owner(target.ghost) != Some(self.owner) {
                continue;
            };
            let stop = instances.signal(
                content.sequences.get(&state).ok_or(Error::Unsupported)?,
                players,
                self.owner,
                message,
            )?;
            if stop.is_some() {
                self.sequence_deadlines.remove(&state);
            }
            return Ok(Some(stop.into_iter().map(HostRpc::SequenceStop).collect()));
        }
        Ok(None)
    }
    pub fn sequence_owner(&self, ghost: u16, selector: u16) -> bool {
        self.sequences
            .values()
            .any(|s| s.has_endpoint(ghost, selector))
    }
    pub fn completed(
        &mut self,
        content: &Mode,
        players: &mut Players,
        call: Completed,
        car_id: u32,
    ) -> Result<Output, Error> {
        if call.participant != self.owner.participant {
            return Err(Error::UnknownObject);
        };
        let mut next = self.clone();
        let mut staged = players.clone();
        let mut out = Output::default();
        let (&state, instances) = next
            .sequences
            .iter_mut()
            .find(|(_, s)| s.has_endpoint(call.sequence, 0))
            .ok_or(Error::UnknownObject)?;
        let Some(deleted) = instances.complete(&mut staged, next.owner, call)? else {
            return Ok(out);
        };
        out.deleted.push(deleted);
        // The clock already completed countdown membership before native
        // acknowledgement; this acknowledgement only releases its sequence.
        if content
            .data
            .game_mode
            .lifecycle
            .states
            .iter()
            .any(|s| s.serializer == state && s.kind == StateKind::Countdown)
        {
            *self = next;
            *players = staged;
            return Ok(out);
        }
        if !next.goals.is_empty() {
            out.records.push(staged.release_controlled_goals(
                next.owner.connection,
                next.owner.persona,
                next.owner.participant,
                &next.goals,
            )?);
            out.deleted.append(&mut next.goals);
        }
        if next.car_id.is_some_and(|old| old != car_id) {
            return Err(Error::Shape);
        }
        next.car_id = Some(car_id);
        let completion = if content.timing.contains_key(&state) {
            Action::Complete {
                state: state as u16,
                participant: next.owner.participant.into(),
            }
        } else {
            Action::CompleteAll {
                state: state as u16,
                only_incomplete: true,
            }
        };
        let effects = next.dispatch(vec![Command::State(completion)])?;
        next.effects(content, &mut staged, &effects, false, &mut out)?;
        *self = next;
        *players = staged;
        Ok(out)
    }
}
