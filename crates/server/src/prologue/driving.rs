// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-owned intro modes. Layouts and transitions come from the installed
//! composition; the session stages these owners with replication and delivery.
use crate::{
    event_logic::{
        self,
        lifecycle::{Effect, Host, Plan},
    },
    sequence_content::authored::{Authored, Instances},
};
use nfs_content::{
    assets::AssetIndex,
    events::{
        Binding, intro,
        lifecycle::{StateConfiguration, StateKind},
    },
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
    reporters: Option<reporters::Reporters>,
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
        let data = intro::extract(index, name, None)?;
        let mut sequences = BTreeMap::new();
        for s in &data.sequences {
            let stops =
                nfs_content::prologue::sequences::direct_stop_signals(index, &s.layout.blueprint)?;
            if stops.is_empty() {
                return Err(nfs_content::Error::Content(
                    "intro driving sequence completion signal".into(),
                ));
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
            reporters: None,
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
    logics: BTreeMap<usize, u16>,
    sequences: BTreeMap<usize, Instances>,
    goals: Vec<u16>,
    routes: BTreeMap<Identity, (u64, Option<u16>)>,
    seconds: f64,
    progress: Option<(u16, nfs_world::race_progress::Race)>,
    arrived: bool,
    /// Local observations belong to this mode, even when they have no wire field.
    contexts: BTreeMap<Property, BTreeSet<u32>>,
    booleans: BTreeMap<Identity, bool>,
    reporters: BTreeMap<Identity, i32>,
    callbacks: Option<reporters::Reporters>,
}
impl Active {
    /// Bootstrap before publication: initial membership is included in creation.
    /// No transient entry/exit RPC is sent for a root the client does not own yet.
    pub fn start(
        content: &Mode,
        players: &mut Players,
        owner: Owner,
        seconds: f64,
    ) -> Result<(Self, Output), Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        };
        let mut staged = players.clone();
        let parent = content.parent(staged.objects(), owner.participant)?;
        let mut host = Host::new(content.plan.clone()).map_err(invalid)?;
        let mut callbacks = content.reporters.clone();
        let effects = host
            .apply(
                &[Action::Enter {
                    state: content.data.game_mode.lifecycle.initial as u16,
                    participant: owner.participant.into(),
                }],
                &mut |_, e| {
                    callbacks
                        .as_mut()
                        .map_or_else(|| Ok(vec![]), |r| r.effect(e))
                },
            )
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
            goals: vec![],
            routes: BTreeMap::new(),
            seconds,
            progress: None,
            arrived: false,
            contexts: BTreeMap::new(),
            booleans: BTreeMap::new(),
            reporters: BTreeMap::new(),
            callbacks,
        };
        active.effects(content, &mut staged, &effects, true, &mut out)?;
        let (target, selector) = field(staged.objects(), parent, content.data.launcher)?;
        out.calls.push(HostRpc::GameMode(event_logic::call(
            staged.objects(),
            target,
            selector,
            4,
            Some(active.ghost),
            Arguments::None,
        )?));
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
                    if let Some((state, participant, method)) = call.filter(|_| !bootstrap) {
                        let (target, selector) =
                            field(players.objects(), self.ghost, state.into())?;
                        out.calls.push(HostRpc::GameMode(event_logic::call(
                            players.objects(),
                            target,
                            selector,
                            method,
                            Some(u16::try_from(participant).map_err(invalid)?),
                            Arguments::None,
                        )?));
                    }
                }
                Effect::Hook(Notification::Entered { state, .. }) => {
                    let data = content
                        .data
                        .game_mode
                        .lifecycle
                        .states
                        .iter()
                        .find(|s| s.serializer == usize::from(*state))
                        .ok_or(Error::UnknownObject)?;
                    match data.kind {
                        StateKind::Sequence => {
                            let s = content
                                .data
                                .sequences
                                .iter()
                                .find(|s| s.spawner == Some(usize::from(*state)))
                                .ok_or(Error::Unsupported)?;
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
                            if self.progress.is_some() {
                                return Err(Error::Shape);
                            }
                            self.progress = Some((
                                *state,
                                nfs_world::race_progress::Race::new(
                                    content.data.game_mode.checkpoints.len(),
                                    content.data.game_mode.laps,
                                    &[self.owner.participant],
                                    self.seconds,
                                )?,
                            ));
                        }
                        StateKind::GameMode | StateKind::Participant => {}
                        _ => return Err(Error::Unsupported),
                    }
                }
                Effect::Hook(_) => {} // Completion/leave already changes Host membership; specialized entry is above.
                Effect::Logic {
                    spawner,
                    participant,
                    create,
                } => {
                    if *participant != u64::from(self.owner.participant) {
                        return Err(Error::UnknownObject);
                    };
                    if *create {
                        let layout = content
                            .data
                            .logics
                            .get(*spawner)
                            .ok_or(Error::Unsupported)?;
                        let creation = event_logic::logic_creation(
                            layout,
                            self.ghost,
                            self.owner.participant,
                            |b| {
                                content.binding(players.objects(), self.parent, Some(self.ghost), b)
                            },
                            &|_, _| None,
                        )?;
                        let record = players
                            .spawn_entities(vec![creation], &content.content)?
                            .remove(0);
                        if self.logics.insert(*spawner, record.id).is_some() {
                            return Err(Error::DuplicateObject);
                        };
                        out.records.push(record);
                    } else if let Some(ghost) = self.logics.remove(spawner) {
                        players.remove_entities(&[ghost])?;
                        out.deleted.push(ghost);
                    }
                }
                Effect::Context { manager, change } => {
                    if change.participant != u64::from(self.owner.participant) {
                        return Err(Error::UnknownObject);
                    };
                    self.contexts
                        .insert(manager.clone(), change.categories.clone());
                }
                Effect::Boolean { identity, change } => {
                    if change.participant != u64::from(self.owner.participant) {
                        return Err(Error::UnknownObject);
                    };
                    self.booleans.insert(identity.clone(), change.value);
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
                Effect::Timing(event_logic::lifecycle::timed::Effect::Finish {
                    participant,
                    position: 1,
                    remaining,
                    ..
                }) if *participant == u64::from(self.owner.participant) && remaining.is_empty() => {
                    self.arrived = true;
                }
                _ => return Err(Error::Unsupported),
            }
        }
        Ok(())
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
                if *participant != u64::from(self.owner.participant)
                    || self.routes.contains_key(identity)
                {
                    return Err(Error::Shape);
                }
                let [command] = commands.as_slice() else {
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
                let changes = players.start_player_route(
                    self.owner.connection,
                    self.owner.persona,
                    self.owner.participant,
                    event_logic::route_goal(&content.data.game_mode)?,
                    None,
                )?;
                self.routes
                    .insert(identity.clone(), (*token, Some(changes.goals[0])));
                changes
            }
            Effect::Release {
                identity,
                participant,
                token,
            } => {
                if *participant != u64::from(self.owner.participant) {
                    return Err(Error::UnknownObject);
                }
                let &(owned, goal) = self.routes.get(identity).ok_or(Error::UnknownObject)?;
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
                self.routes.remove(identity);
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
        if !next.goals.is_empty() {
            out.records.push(staged.release_controlled_goals(
                next.owner.connection,
                next.owner.persona,
                next.owner.participant,
                &next.goals,
            )?);
            out.deleted.append(&mut next.goals);
        }
        let effects = next
            .host
            .apply(
                &[Action::Complete {
                    state: state as u16,
                    participant: next.owner.participant.into(),
                }],
                &mut |_, e| {
                    next.callbacks
                        .as_mut()
                        .map_or_else(|| Ok(vec![]), |r| r.effect(e))
                },
            )
            .map_err(invalid)?;
        next.effects(content, &mut staged, &effects, false, &mut out)?;
        for state in &content.data.game_mode.lifecycle.states {
            if matches!(state.configuration, StateConfiguration::Waypoints { .. })
                && next
                    .host
                    .machine()
                    .members(state.serializer as u16)
                    .map_err(invalid)?
                    .contains(&u64::from(next.owner.participant))
            {
                let (target, selector) = field(staged.objects(), next.ghost, state.serializer)?;
                out.calls.push(HostRpc::GameMode(event_logic::call(
                    staged.objects(),
                    target,
                    selector,
                    3,
                    Some(next.owner.participant),
                    Arguments::Word(car_id),
                )?));
            }
        }
        *self = next;
        *players = staged;
        Ok(out)
    }
}
