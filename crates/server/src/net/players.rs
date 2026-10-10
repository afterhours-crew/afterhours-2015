// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Accepting;
use crate::vehicle_content::{
    GarageContent,
    population::{Inventory, Population},
};
use nfs_world::{
    application::{HOST_SELECTOR, Listener, Queued},
    frame,
    replication::{
        self,
        players::{Players, Request},
        section::{Section, Setup, split_records as split_scenes},
    },
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod customization;
mod launchers;
mod sequences;

#[cfg(test)]
mod deployment_tests;
mod vehicles;

const MAX_PENDING: usize = 32;
const MAX_WORLD_CARS: usize = 128;

pub(super) struct PlayerListener {
    stats: Accepting,
    persona: u64,
    roles: Option<crate::scene_roles::SceneRoles>,
    players: Players,
    launchers: nfs_world::launchers::Launchers,
    participants: nfs_world::participants::Lifecycle,
    persistent: Option<std::sync::Arc<crate::persistent::Loaded>>,
    progression: Option<std::sync::Arc<crate::progression::Content>>,
    vehicles: Option<std::sync::Arc<GarageContent>>,
    population: Option<Population>,
    garage_presence: nfs_world::garage::presence::Presence,
    spawn_points: nfs_world::spawn_points::SpawnPoints,
    level_poll: nfs_world::level_poll::LevelPoll,
    /// World car ghost -> (participant, spawn assigned).
    world_cars: BTreeMap<u16, (u16, bool)>,
    glass: crate::glass::Glass,
    customization: crate::customization_timer::Timers,
    world_ms: u64,
    sequences: Option<nfs_world::sequences::Sequences>,
    inventory: Option<Inventory>,
    logic: Option<std::sync::Arc<crate::garage_logic::GarageLogic>>,
    logic_ghosts: BTreeMap<u16, BTreeMap<u16, u16>>,
    logic_vehicles_fired: BTreeSet<u16>,
    pub(super) pending_content: VecDeque<nfs_world::content::Message>,
    pub(super) pending_rpcs: VecDeque<nfs_world::participants::HostRpc>,
    unsupported_rpcs: usize,
    unsupported_logic: usize,
    pub(super) pending: VecDeque<Section>,
    pub(super) pending_entries: VecDeque<(Section, Vec<nfs_world::participants::HostRpc>)>,
    refused: usize,
    last_error: Option<replication::Error>,
}
impl std::fmt::Debug for PlayerListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerListener")
            .field("stats", &self.stats)
            .field("objects", &self.players.objects().len())
            .field("pending", &self.pending.len())
            .field("pending_entries", &self.pending_entries.len())
            .field("enabled_launchers", &self.launchers.enabled_count())
            .field("participants", &self.participants)
            .field("progression", &self.progression.is_some())
            .field("pending_rpcs", &self.pending_rpcs.len())
            .field("unsupported_rpcs", &self.unsupported_rpcs)
            .field("unsupported_logic", &self.unsupported_logic)
            .field("refused", &self.refused)
            .field("last_error", &self.last_error)
            .finish()
    }
}
impl PlayerListener {
    pub(super) fn inventory_with_items(
        &mut self,
        garage: [Option<u64>; 5],
        persistent: Option<std::sync::Arc<crate::persistent::Loaded>>,
        items: Option<std::sync::Arc<nfs_world::items::Collection>>,
    ) -> Result<(), replication::Error> {
        if persistent.as_ref().is_some_and(|p| p.garage() != garage)
            || self
                .persistent
                .as_ref()
                .is_some_and(|old| persistent.as_ref() != Some(old))
        {
            return Err(replication::Error::Shape);
        }
        let mut participants = self.participants.clone();
        let mut players = self.players.clone();
        let mut population = self.population.clone();
        let mut garage_presence = self.garage_presence.clone();
        let mut launchers = self.launchers.clone();
        let mut sequences = self.sequences.clone();
        let mut logic_ghosts = self.logic_ghosts.clone();
        let inventory = items
            .map(|items| {
                nfs_world::garage::Slots::new(garage).map(|slots| Inventory { slots, items })
            })
            .transpose()?;
        if self.inventory.as_ref().is_some_and(|old| {
            inventory
                .as_ref()
                .is_none_or(|new| old.slots != new.slots || old.items != new.items)
        }) {
            return Err(replication::Error::Unsupported);
        }
        participants.set_garage(garage)?;
        let mut replies = Self::loaded_replies(&mut participants, persistent.as_deref(), |id| {
            self.players
                .owns_participant(HOST_SELECTOR as u8, self.persona, id)
        });
        let mut sections = Vec::new();
        if let (Some(progression), Some(loaded)) = (&self.progression, persistent.as_deref()) {
            let (spawned, chain) = self.progress(
                progression,
                &mut players,
                &mut participants,
                loaded,
                &mut logic_ghosts,
            )?;
            sections = spawned;
            replies.extend(chain);
        }
        sections.extend(Self::start_sequences(
            &mut players,
            &participants,
            sequences.as_mut(),
            self.persona,
        )?);
        let spawned = Self::populate(
            &mut players,
            &participants,
            population.as_mut(),
            inventory.as_ref(),
            self.vehicles.as_deref(),
            self.persona,
        )?;
        sections.extend(spawned.sections);
        replies.extend(Self::garage_presence(
            self.roles.as_ref(),
            &players,
            &spawned.replies,
            &mut garage_presence,
            self.persona,
        )?);
        let mut logic_vehicles_fired = self.logic_vehicles_fired.clone();
        let vehicle_events = Self::vehicle_events(
            self.logic.as_deref(),
            &mut players,
            self.persona,
            &spawned.replies,
            &mut logic_ghosts,
            &mut logic_vehicles_fired,
        )?;
        replies.extend(spawned.replies);
        replies.extend(vehicle_events);
        Self::bind_progression_launchers(
            self.progression.as_deref(),
            &players,
            &sections,
            &mut launchers,
            self.persona,
        )?;
        if spawned.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || replies.len() > nfs_world::session::MAX_RPC_QUEUE - self.pending_rpcs.len()
            || sections.len() > MAX_PENDING - self.pending.len()
        {
            return Err(replication::Error::Bound);
        }
        self.participants = participants;
        self.launchers = launchers;
        self.players = players;
        self.persistent = persistent;
        self.population = population;
        self.garage_presence = garage_presence;
        self.sequences = sequences;
        self.inventory = inventory;
        self.logic_ghosts = logic_ghosts;
        self.logic_vehicles_fired = logic_vehicles_fired;
        self.pending_content.extend(spawned.messages);
        self.pending_rpcs.extend(replies);
        self.pending.extend(sections);
        Ok(())
    }
    fn progress(
        &self,
        progression: &crate::progression::Content,
        players: &mut Players,
        participants: &mut nfs_world::participants::Lifecycle,
        loaded: &crate::persistent::Loaded,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
    ) -> Result<(Vec<Section>, Vec<nfs_world::participants::HostRpc>), replication::Error> {
        use crate::progression::{Branch, dispatch, entities::SceneEndpoint, ready};
        let roles = self.roles.as_ref().ok_or(replication::Error::Unsupported)?;
        let persona = self.persona;
        let logic = self.logic.as_deref();
        let mut records = Vec::new();
        let mut replies = Vec::new();
        for participant in participants.waiting_progression() {
            if let Some(actor) = players.create_actor(HOST_SELECTOR as u8, persona, participant)? {
                records.push(actor);
            }
            let mut restored = progression
                .catalog
                .restore(loaded, progression.settings)
                .map_err(|_| replication::Error::Shape)?;
            let dispatched = dispatch(&mut restored).map_err(|_| replication::Error::Shape)?;
            let blueprint = players
                .objects()
                .scene(roles.progression)
                .ok_or(replication::Error::UnknownObject)?;
            let mut creations = Vec::new();
            for branch in [Branch::Main, Branch::Intro] {
                let creation = progression
                    .construction
                    .build(
                        branch,
                        restored.branch(branch),
                        participant,
                        blueprint,
                        |key, index| {
                            players
                                .objects()
                                .scene_endpoint(key, index)
                                .map(|(ghost, selector)| SceneEndpoint { ghost, selector })
                        },
                    )
                    .map_err(|_| replication::Error::Unsupported)?;
                creations.push(creation);
            }
            records.extend(players.spawn_entities(creations, progression.construction.content())?);
            if !participants.begin_progression(participant) {
                return Err(replication::Error::Shape);
            }
            let restored_branches = [Branch::Main, Branch::Intro]
                .map(|b| dispatched.get(&b).is_some_and(|d| d.restored()));
            let activated: usize = dispatched.values().map(|d| d.activated.len()).sum();
            tracing::info!(
                participant,
                main_restored = restored_branches[0],
                intro_restored = restored_branches[1],
                activated,
                "progression restored; activation RPCs/events are not emitted"
            );
            if ready(&dispatched, progression.speed_list_bypass) {
                let binding = players
                    .bind_actor(roles.gameplay, HOST_SELECTOR as u8, persona, participant)?
                    .ok_or(replication::Error::Shape)?;
                replies.extend(participants.enter_garage_with_actor(binding)?);
                if let Some(logic) = logic {
                    let ghosts = logic_ghosts.entry(participant).or_default();
                    if !ghosts.is_empty() {
                        return Err(replication::Error::DuplicateObject);
                    }
                    let scores = logic
                        .requires_reputation()
                        .then(|| {
                            crate::garage_logic::ReputationScores::load(loaded)
                                .map_err(|_| replication::Error::Shape)
                        })
                        .transpose()?;
                    let out = logic.produce_with_reputation(
                        crate::garage_logic::Which::Spawn,
                        players,
                        participant,
                        ghosts,
                        scores,
                    )?;
                    records.extend(out.records);
                    replies.extend(
                        out.events
                            .into_iter()
                            .map(nfs_world::participants::HostRpc::Event),
                    );
                }
            }
        }
        let sections = if records.is_empty() {
            Vec::new()
        } else {
            split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS)?
        };
        Ok((sections, replies))
    }
    fn vehicle_events(
        logic: Option<&crate::garage_logic::GarageLogic>,
        players: &mut Players,
        persona: u64,
        replies: &[nfs_world::participants::HostRpc],
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        fired: &mut BTreeSet<u16>,
    ) -> Result<Vec<nfs_world::participants::HostRpc>, replication::Error> {
        let Some(logic) = logic else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut populated: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
        for reply in replies {
            if let nfs_world::participants::HostRpc::Vehicle(v) = reply {
                populated.entry(v.participant).or_default().push(v.vehicle);
            }
        }
        for (participant, _) in populated {
            let Some(ghosts) = logic_ghosts.get_mut(&participant) else {
                continue;
            };
            if ghosts.is_empty() || !fired.insert(participant) {
                continue;
            }
            let produced = logic.produce(
                crate::garage_logic::Which::Vehicles,
                players,
                participant,
                ghosts,
            )?;
            out.extend(
                produced
                    .events
                    .into_iter()
                    .map(nfs_world::participants::HostRpc::Event),
            );
            if !players.owns_participant(HOST_SELECTOR as u8, persona, participant) {
                return Err(replication::Error::UnknownObject);
            }
        }
        Ok(out)
    }
    fn enter_logic(
        logic: &crate::garage_logic::GarageLogic,
        players: &mut Players,
        sequences: &nfs_world::sequences::Sequences,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
    ) -> Result<(Vec<Section>, Vec<nfs_world::logic::Fire>), replication::Error> {
        let mut out = logic.produce(
            crate::garage_logic::Which::Entry,
            players,
            participant,
            ghosts,
        )?;
        if let Some(sequence) = sequences.ghost(participant) {
            players.remove_entities(&[sequence])?;
            out.deleted.push(sequence);
        }
        let mut sections = if out.records.is_empty() {
            vec![Section {
                float_bits: None,
                flag: false,
                deleted: Vec::new(),
                setup: None,
                records: Vec::new(),
            }]
        } else {
            split_scenes(out.records, nfs_world::application::OUTBOUND_FRAME_BITS)?
        };
        sections[0].deleted = out.deleted;
        Ok((sections, out.events))
    }
    fn loaded_replies(
        participants: &mut nfs_world::participants::Lifecycle,
        persistent: Option<&crate::persistent::Loaded>,
        connected: impl Fn(u16) -> bool,
    ) -> Vec<nfs_world::participants::HostRpc> {
        use nfs_world::participants::HostRpc;
        let mut replies: Vec<_> = participants
            .take_garage_bindings()
            .into_iter()
            .map(HostRpc::Garage)
            .collect();
        if persistent.is_some() {
            replies.extend(
                participants
                    .finish_persistent(connected)
                    .into_iter()
                    .map(HostRpc::Participant),
            );
        }
        replies
    }
    pub(super) fn take_file(&mut self, index: usize) -> Option<nfs_world::files::Completed> {
        self.stats.take_file(index)
    }
    pub(super) fn files_alive(&mut self, now: u64) -> bool {
        self.stats.files_alive(now)
    }
    pub(super) fn new(persona: u64) -> Self {
        Self {
            stats: Accepting::default(),
            persona,
            roles: None,
            players: Players::default(),
            launchers: nfs_world::launchers::Launchers::default(),
            participants: nfs_world::participants::Lifecycle::default(),
            persistent: None,
            progression: None,
            vehicles: None,
            population: None,
            garage_presence: nfs_world::garage::presence::Presence::default(),
            spawn_points: nfs_world::spawn_points::SpawnPoints::default(),
            level_poll: nfs_world::level_poll::LevelPoll::default(),
            world_cars: BTreeMap::new(),
            glass: crate::glass::Glass::default(),
            customization: crate::customization_timer::Timers::default(),
            world_ms: 0,
            sequences: None,
            inventory: None,
            logic: None,
            logic_ghosts: BTreeMap::new(),
            logic_vehicles_fired: BTreeSet::new(),
            pending_content: VecDeque::new(),
            pending_rpcs: VecDeque::new(),
            unsupported_rpcs: 0,
            unsupported_logic: 0,
            pending: VecDeque::new(),
            pending_entries: VecDeque::new(),
            refused: 0,
            last_error: None,
        }
    }

    pub(super) fn with_progression(
        mut self,
        progression: Option<std::sync::Arc<crate::progression::Content>>,
    ) -> Self {
        self.progression = progression;
        self
    }
    pub(super) fn with_vehicles(mut self, vehicles: Option<std::sync::Arc<GarageContent>>) -> Self {
        self.vehicles = vehicles;
        self
    }
    pub(super) fn with_sequences(
        mut self,
        sequences: Option<nfs_world::sequences::Sequences>,
    ) -> Self {
        self.sequences = sequences;
        self
    }
    pub(super) fn with_garage_logic(
        mut self,
        logic: Option<std::sync::Arc<crate::garage_logic::GarageLogic>>,
    ) -> Self {
        self.logic = logic;
        self
    }
    pub(super) fn initialize_world(
        &mut self,
        content: &crate::content::WorldContent,
    ) -> Result<(), replication::Error> {
        use nfs_protocol::world::rpc::Serial;
        use nfs_world::content::bindings::Bindings;
        use replication::sublevel::{Rpc, genesis};
        let roles = content
            .roles
            .as_ref()
            .ok_or(replication::Error::Unsupported)?;
        roles.validate().map_err(|_| replication::Error::Shape)?;
        if content.level.level != roles.level {
            return Err(replication::Error::Unsupported);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(replication::Error::Bound);
        }
        let messages = content.generate().map_err(|_| replication::Error::Shape)?;
        let population = self
            .vehicles
            .as_ref()
            .map(|_| Population::new(&messages))
            .transpose()?;
        let bindings = Bindings::from_messages(&messages).map_err(|_| replication::Error::Shape)?;
        let rpc = Rpc {
            selector: 0,
            serial: Serial::new(0).ok_or(replication::Error::Shape)?,
        };
        let profiles = content
            .scene_profiles
            .as_ref()
            .ok_or(replication::Error::Unsupported)?;
        let creations = genesis::hierarchy(rpc, &bindings, profiles, &roles.traffic)?;
        let levels = creations
            .iter()
            .map(|c| c.prefix.level_id)
            .collect::<Vec<_>>();
        let mut players = self.players.clone();
        let records = players.create_scenes(&levels, creations, profiles)?;
        let mut launchers = self.launchers.clone();
        if let Some(catalog) = &content.launchers {
            launchers.bind(&records, catalog)?;
        }
        let mut participants = self.participants.clone();
        participants.bind(&records, roles.gameplay, roles.startup)?;
        match participants.bind_exit(&records, roles.startup, roles.garage) {
            Ok(true) => {}
            Ok(false) => tracing::info!("garage exit scenes absent; garage exit stays unsupported"),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "garage exit endpoints unbound; garage exit stays unsupported"
                )
            }
        }
        let mut spawn_points = nfs_world::spawn_points::SpawnPoints::default();
        match roles
            .spawn_points
            .map(|key| spawn_points.bind(&records, key))
        {
            Some(Ok(true)) => {}
            None | Some(Ok(false)) => {
                tracing::info!("SpawnPoints scene absent; world spawns stay unsupported")
            }
            Some(Err(error)) => {
                tracing::warn!(
                    ?error,
                    "SpawnPoints endpoints unbound; world spawns stay unsupported"
                )
            }
        }
        let mut level_poll = nfs_world::level_poll::LevelPoll::default();
        match level_poll.bind(&records, roles.gameplay) {
            Ok(true) => {}
            Ok(false) => tracing::info!("level root absent; level poll stays unsupported"),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "level poll endpoint unbound; poll stays unsupported"
                )
            }
        }
        let sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS)?;
        if sections.len() > MAX_PENDING - self.pending.len() {
            return Err(replication::Error::Bound);
        }
        self.players = players;
        self.launchers = launchers;
        self.participants = participants;
        self.spawn_points = spawn_points;
        self.level_poll = level_poll;
        self.pending.extend(sections);
        self.population = population;
        self.roles = Some(roles.clone());
        Ok(())
    }
}

impl Listener for PlayerListener {
    fn frame(&mut self, queued: Queued<'_>) -> bool {
        let parsed = match self.stats.parse(queued) {
            Ok(parsed) => parsed,
            Err(_) => return self.stats.invalid(),
        };
        let files = match self.stats.file_transition(&parsed) {
            Ok(files) => files,
            Err(_) => return self.stats.invalid(),
        };
        let requests: Vec<_> = parsed
            .messages
            .iter()
            .flat_map(|m| &m.groups)
            .flat_map(|g| &g.messages)
            .filter_map(|m| match m {
                frame::Message::CreatePlayer { name, flag, slot } => Some(Request {
                    name: name.clone(),
                    flag: *flag,
                    slot: *slot,
                }),
                _ => None,
            })
            .collect();
        let rpcs = parsed
            .messages
            .iter()
            .flat_map(|m| &m.groups)
            .flat_map(|g| &g.messages)
            .filter_map(|m| match m {
                frame::Message::Opaque { index: 47, body } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();
        let logic_events = parsed
            .messages
            .iter()
            .flat_map(|m| &m.groups)
            .flat_map(|g| &g.messages)
            .filter_map(|m| match m {
                frame::Message::Opaque { index: 82, body } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();
        if requests.is_empty() && rpcs.is_empty() && logic_events.is_empty() {
            return self.stats.record(&parsed, files);
        }
        let mut players = self.players.clone();
        let mut participants = self.participants.clone();
        let mut records = Vec::new();
        let mut population = self.population.clone();
        let mut garage_presence = self.garage_presence.clone();
        let mut spawn_points = self.spawn_points.clone();
        let mut level_poll = self.level_poll.clone();
        let mut world_cars = self.world_cars.clone();
        let mut sequences = self.sequences.clone();
        let mut logic_ghosts = self.logic_ghosts.clone();
        let mut unsupported_logic = 0;
        let mut glass = self.glass.clone();
        let mut customization = self.customization.clone();
        let mut measurements = Vec::new();
        let mut responses = Vec::new();
        let logic_result = (|| {
            if let Some(sequences) = &mut sequences {
                Self::refresh_readiness(
                    self.roles.as_ref(),
                    sequences,
                    &players,
                    &participants,
                    population.as_ref(),
                    &logic_ghosts,
                    self.persona,
                )?;
            }
            for body in logic_events {
                let message = nfs_world::logic::Message::decode(82, body)
                    .map_err(|_| replication::Error::Shape)?;
                if sequences
                    .as_mut()
                    .map(|s| s.streaming_event(&message))
                    .transpose()?
                    .unwrap_or(false)
                {
                    continue;
                }
                if crate::customization_timer::Timers::recognizes(&message) {
                    let owner = Self::customization_owner(
                        self.roles.as_ref(),
                        &players,
                        &logic_ghosts,
                        self.persona,
                        &message,
                    )?;
                    let item = self.inventory.as_ref().and_then(|i| {
                        let id = i.slots.values()[0]?;
                        i.items.items.contains_key(&id).then_some(id)
                    });
                    if let Some(measurement) =
                        customization.receive(owner, item, self.world_ms, &message)?
                    {
                        measurements.push((owner.participant, measurement));
                    }
                } else if crate::glass::Glass::recognizes(&message) {
                    let nfs_world::logic::Message::Reached { target, .. } = &message else {
                        return Err(replication::Error::Shape);
                    };
                    let owner = population
                        .as_ref()
                        .and_then(|p| {
                            p.signal_owner(
                                &players,
                                HOST_SELECTOR as u8,
                                self.persona,
                                target.ghost,
                            )
                        })
                        .ok_or(replication::Error::UnknownObject)?;
                    responses.push(nfs_world::participants::HostRpc::Event(
                        glass.receive(owner, &message)?,
                    ));
                    // A world car's first glass report also shows that the
                    // client has the car: assign its spawn point (E751).
                    if let Some(notification) =
                        Self::world_car_ready(&message, &mut world_cars, &mut spawn_points)?
                            .flatten()
                    {
                        tracing::info!(
                            participant = notification.participant,
                            call = ?notification.call,
                            "owned world car created; spawn point assigned"
                        );
                        responses.push(nfs_world::participants::HostRpc::Participant(notification));
                    }
                } else {
                    unsupported_logic += 1;
                    tracing::debug!(?message, "unsupported client logic event");
                }
            }
            Ok::<(), replication::Error>(())
        })();
        if let Err(error) = logic_result {
            self.refused += 1;
            self.last_error = Some(error);
            return false;
        }
        for request in requests {
            match players.create(HOST_SELECTOR as u8, self.persona, request) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(error) => {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
        }
        if participants.available() {
            let player_ids = records.iter().map(|r| r.id).collect::<Vec<_>>();
            for player in player_ids {
                let joined = players
                    .join(HOST_SELECTOR as u8, self.persona, player)
                    .and_then(|record| {
                        if let Some(record) = record {
                            let replies = participants.begin(record.id)?;
                            records.push(record);
                            responses.extend(
                                replies
                                    .into_iter()
                                    .map(nfs_world::participants::HostRpc::Participant),
                            );
                        }
                        Ok(())
                    });
                if let Err(error) = joined {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
        }
        if !records.is_empty() && self.pending.len() >= MAX_PENDING {
            self.refused += 1;
            return false;
        }
        let mut launchers = self.launchers.clone();
        let mut unsupported = 0;
        let mut exit_entries = Vec::new();
        for body in rpcs {
            match Self::item_builder_begin(self.roles.as_ref(), &players, self.persona, body) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
            if let Some(sequences) = &mut sequences {
                match Self::sequence_completed(sequences, &players, self.persona, body) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        self.refused += 1;
                        self.last_error = Some(error);
                        return false;
                    }
                }
            }
            if let Some(population) = &mut population {
                match Self::vehicle_loaded(population, &players, self.persona, body) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        self.refused += 1;
                        self.last_error = Some(error);
                        return false;
                    }
                }
            }
            match level_poll.receive(body, |id| {
                players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
            }) {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(error) => {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
            match spawn_points.receive(body, |id| {
                players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
            }) {
                Ok(Some(flag)) => {
                    tracing::info!(?flag, "owned spawn request or release");
                    responses.extend(flag.map(nfs_world::participants::HostRpc::SpawnOccupied));
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
            match launchers.receive(body, |id| {
                players.owns(HOST_SELECTOR as u8, self.persona, id)
            }) {
                Ok(
                    nfs_world::launchers::Outcome::Enabled(reply)
                    | nfs_world::launchers::Outcome::Repeated(reply),
                ) => responses.push(nfs_world::participants::HostRpc::Launcher(reply)),
                Ok(nfs_world::launchers::Outcome::Unsupported) => {
                    match participants.receive(body, |id| {
                        players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
                    }) {
                        Ok(nfs_world::participants::Outcome::Advanced(replies)) => {
                            let mut chain: Vec<_> = replies
                                .into_iter()
                                .map(nfs_world::participants::HostRpc::Participant)
                                .collect();
                            let exited = participants.take_exited();
                            // Official order: presence off follows the leave of
                            // customization and state 94 (E747).
                            for &participant in &exited {
                                match Self::garage_presence_leave(
                                    self.roles.as_ref(),
                                    &players,
                                    &mut garage_presence,
                                    participant,
                                    self.persona,
                                ) {
                                    Ok(Some(rpc)) => chain.insert(3.min(chain.len()), rpc),
                                    Ok(None) => {}
                                    Err(error) => {
                                        self.refused += 1;
                                        self.last_error = Some(error);
                                        return false;
                                    }
                                }
                            }
                            let mut world_car = match exited.as_slice() {
                                [participant] => match Self::exit_world_car(
                                    &mut players,
                                    population.as_mut(),
                                    self.inventory.as_ref(),
                                    self.vehicles.as_deref(),
                                    *participant,
                                    self.persona,
                                ) {
                                    Ok(section) => section,
                                    Err(error) => {
                                        self.refused += 1;
                                        self.last_error = Some(error);
                                        return false;
                                    }
                                },
                                _ => None,
                            };
                            // The same exit frame deletes the garage-only logic and
                            // recreates the loading presentation (E757).
                            if let (Some(section), Some(logic), [participant]) =
                                (world_car.as_mut(), self.logic.as_deref(), exited.as_slice())
                                && logic.has_exit()
                            {
                                match logic.produce(
                                    crate::garage_logic::Which::Exit,
                                    &mut players,
                                    *participant,
                                    logic_ghosts.entry(*participant).or_default(),
                                ) {
                                    Ok(out) => {
                                        section.deleted.extend(out.deleted);
                                        section.records.extend(out.records);
                                        chain.extend(
                                            out.events
                                                .into_iter()
                                                .map(nfs_world::participants::HostRpc::Event),
                                        );
                                    }
                                    Err(error) => {
                                        self.refused += 1;
                                        self.last_error = Some(error);
                                        return false;
                                    }
                                }
                            }
                            for &participant in &exited {
                                tracing::info!(
                                    participant,
                                    world_car = world_car.is_some(),
                                    "owned garage exit granted; participant loads the world (startup state 77)"
                                );
                            }
                            match world_car {
                                // The official exit frame carries the car swap and
                                // the state chain together (E748).
                                Some(section) => {
                                    if let Err(error) =
                                        Self::register_world_car(&mut world_cars, &exited, &section)
                                    {
                                        self.refused += 1;
                                        self.last_error = Some(error);
                                        return false;
                                    }
                                    exit_entries.push((section, chain))
                                }
                                None => responses.extend(chain),
                            }
                        }
                        Ok(nfs_world::participants::Outcome::Repeated) => {}
                        Ok(nfs_world::participants::Outcome::Unsupported) => unsupported += 1,
                        Err(error) => {
                            self.refused += 1;
                            self.last_error = Some(error);
                            return false;
                        }
                    }
                }
                Err(error) => {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
        }
        responses.extend(Self::loaded_replies(
            &mut participants,
            self.persistent.as_deref(),
            |id| players.owns_participant(HOST_SELECTOR as u8, self.persona, id),
        ));
        let mut sections = Vec::new();
        if let (Some(progression), Some(loaded)) = (&self.progression, self.persistent.as_deref()) {
            match self.progress(
                progression,
                &mut players,
                &mut participants,
                loaded,
                &mut logic_ghosts,
            ) {
                Ok((spawned, chain)) => {
                    sections = spawned;
                    responses.extend(chain);
                }
                Err(error) => {
                    self.refused += 1;
                    self.last_error = Some(error);
                    return false;
                }
            }
        }
        match Self::start_sequences(
            &mut players,
            &participants,
            sequences.as_mut(),
            self.persona,
        ) {
            Ok(spawned) => sections.extend(spawned),
            Err(error) => {
                self.refused += 1;
                self.last_error = Some(error);
                return false;
            }
        }
        let spawned = match Self::populate(
            &mut players,
            &participants,
            population.as_mut(),
            self.inventory.as_ref(),
            self.vehicles.as_deref(),
            self.persona,
        ) {
            Ok(output) => output,
            Err(error) => {
                self.refused += 1;
                self.last_error = Some(error);
                return false;
            }
        };
        sections.extend(spawned.sections);
        match Self::garage_presence(
            self.roles.as_ref(),
            &players,
            &spawned.replies,
            &mut garage_presence,
            self.persona,
        ) {
            Ok(replies) => responses.extend(replies),
            Err(error) => {
                self.refused += 1;
                self.last_error = Some(error);
                return false;
            }
        }
        let mut logic_vehicles_fired = self.logic_vehicles_fired.clone();
        match Self::vehicle_events(
            self.logic.as_deref(),
            &mut players,
            self.persona,
            &spawned.replies,
            &mut logic_ghosts,
            &mut logic_vehicles_fired,
        ) {
            Ok(events) => {
                responses.extend(spawned.replies);
                responses.extend(events);
            }
            Err(error) => {
                self.refused += 1;
                self.last_error = Some(error);
                return false;
            }
        }
        let mut blocked = Vec::new();
        let mut entered = Vec::new();
        let mut entries = Vec::new();
        if let Some(sequences) = &sequences {
            for participant in participants.waiting_garage() {
                if !sequences.is_complete(participant) {
                    continue;
                }
                match participants.enter_customization(participant) {
                    nfs_world::participants::Outcome::Advanced(chain) => {
                        entered.push(participant);
                        let mut entry_replies: Vec<_> = chain
                            .into_iter()
                            .map(nfs_world::participants::HostRpc::Participant)
                            .collect();
                        if let Some(logic) = self.logic.as_deref() {
                            match Self::enter_logic(
                                logic,
                                &mut players,
                                sequences,
                                participant,
                                logic_ghosts.entry(participant).or_default(),
                            ) {
                                Ok((mut section, events)) => {
                                    entry_replies.extend(
                                        events
                                            .into_iter()
                                            .map(nfs_world::participants::HostRpc::Event),
                                    );
                                    if section.len() != 1
                                        || !nfs_world::session::replication_frame(
                                            &section[0],
                                            &entry_replies,
                                            0,
                                        )
                                        .is_ok_and(
                                            |wire| {
                                                wire.len()
                                                    <= nfs_world::application::OUTBOUND_FRAME_BITS
                                            },
                                        )
                                    {
                                        self.refused += 1;
                                        self.last_error = Some(replication::Error::Bound);
                                        return false;
                                    }
                                    entries.push((section.remove(0), entry_replies));
                                }
                                Err(error) => {
                                    self.refused += 1;
                                    self.last_error = Some(error);
                                    return false;
                                }
                            }
                        } else {
                            responses.extend(entry_replies);
                        }
                    }
                    nfs_world::participants::Outcome::Unsupported => blocked.push(participant),
                    nfs_world::participants::Outcome::Repeated => {}
                }
            }
        }
        for (section, replies) in &exit_entries {
            if !nfs_world::session::replication_frame(section, replies, 0)
                .is_ok_and(|wire| wire.len() <= nfs_world::application::OUTBOUND_FRAME_BITS)
            {
                self.refused += 1;
                self.last_error = Some(replication::Error::Bound);
                return false;
            }
        }
        entries.extend(exit_entries);
        if let Err(error) = Self::bind_progression_launchers(
            self.progression.as_deref(),
            &players,
            &sections,
            &mut launchers,
            self.persona,
        ) {
            self.refused += 1;
            self.last_error = Some(error);
            return false;
        }
        if spawned.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || responses.len() > nfs_world::session::MAX_RPC_QUEUE - self.pending_rpcs.len()
            || sections.len() + usize::from(!records.is_empty()) > MAX_PENDING - self.pending.len()
            || entries.len() > MAX_PENDING - self.pending_entries.len()
        {
            self.refused += 1;
            return false;
        }
        for participant in participants.in_free_roam() {
            if self.participants.stage(participant)
                != Some(nfs_world::participants::Stage::FreeRoam)
            {
                tracing::info!(
                    participant,
                    "owned world entry completed; participant entered FreeRoam state 2 (world vehicle not modeled)"
                );
            }
        }
        self.players = players;
        self.launchers = launchers;
        self.participants = participants;
        self.pending_rpcs.extend(responses);
        for participant in self.participants.waiting_garage() {
            if population
                .as_ref()
                .is_some_and(|p| p.all_loaded(participant))
                && self
                    .population
                    .as_ref()
                    .is_none_or(|p| !p.all_loaded(participant))
            {
                tracing::info!(
                    participant,
                    "owned Garage vehicles acknowledged; later transition not inferred"
                );
            }
        }
        self.population = population;
        self.garage_presence = garage_presence;
        self.spawn_points = spawn_points;
        self.level_poll = level_poll;
        self.world_cars = world_cars;
        self.glass = glass;
        self.customization = customization;
        for (participant, measurement) in measurements {
            tracing::info!(
                participant,
                ?measurement,
                "owned customization timer completed (local measurement)"
            );
        }
        for participant in &blocked {
            tracing::warn!(
                participant,
                entry = ?self.participants.entry(*participant),
                "owned garage entry inputs select an unmodeled branch; client stays in LoadingGarage90"
            );
        }
        for participant in entered {
            tracing::info!(
                participant,
                "owned waiting-for-garage sequence completed; participant entered CustomizationState87"
            );
        }
        self.sequences = sequences;
        self.logic_ghosts = logic_ghosts;
        self.logic_vehicles_fired = logic_vehicles_fired;
        self.pending_content.extend(spawned.messages);
        self.unsupported_rpcs += unsupported;
        self.unsupported_logic += unsupported_logic;
        if !records.is_empty() {
            self.pending.push_back(Section {
                float_bits: None,
                flag: false,
                deleted: Vec::new(),
                setup: Some(Setup::Packed {
                    tag: None,
                    width: None,
                    axes: [None; 3],
                }),
                records,
            });
        }
        self.pending.extend(sections);
        self.pending_entries.extend(entries);
        self.stats.record(&parsed, files)
    }
}
