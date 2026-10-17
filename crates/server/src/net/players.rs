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
        section::{Section, Setup, split_timed_records as split_scenes},
    },
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod activation;
mod customization;
mod diagnostics;
mod equipment;
pub(super) use equipment::EquipmentRequest;
pub(super) use scoring::{AwardRequest, Credited};
mod launchers;
mod purchases;
mod refusal;
mod returning;
mod scoring;
mod sequences;

#[cfg(test)]
mod deployment_tests;
mod vehicles;

const MAX_PENDING: usize = 32;
const MAX_WORLD_CARS: usize = 128;
/// Delay from a world car's first glass report to its spawn assignment; the
/// assignment then rides the next level poll (official 777-970 ms).
const SPAWN_DELAY_MS: u64 = 750;
/// Delay from the first glass report to the host teleport; official hosts
/// send it about 100 ms before the assignment (E101 676 ms, E742 870 ms).
const TELEPORT_DELAY_MS: u64 = 650;
const BINDING_REFRESH_DELAY_MS: u64 = 100;
/// One host frame between the first spawn-zone report and held glass replies.
const SETTLE_DELAY_MS: u64 = 100;
/// Glass replies held per world car (three reports per car).
const MAX_HELD_GLASS: usize = 8;

pub(super) struct PlayerListener {
    stats: Accepting,
    messages: frame::receive::Receiver,
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
    /// World car ghost -> garage car deletions sent once the client reports it.
    deferred_deletions: BTreeMap<u16, Vec<u16>>,
    /// Spawn assignments held until due (world ms), sent with a level poll.
    pending_spawns: VecDeque<(u64, nfs_world::participants::Notification)>,
    /// Participant -> glass replies of its world car, held until the
    /// participant reports the spawn zone.
    held_glass: BTreeMap<u16, Vec<nfs_world::logic::Fire>>,
    /// Participants whose world car has reported the spawn zone this lifetime.
    spawn_settled: BTreeSet<u16>,
    /// Release times (world ms) of the held glass replies.
    pending_settled: VecDeque<(u64, u16)>,
    /// Host teleports of world cars, due at world ms (sent as content).
    pending_teleports: VecDeque<(u64, nfs_world::teleport::Command)>,
    /// World clock at which each participant's spawn assignment was queued.
    /// The teleport waits for it: the client adopts the assigned spawn id
    /// only when the assignment precedes its spawn-zone report, and the
    /// car reports the zone within tens of milliseconds of the teleport
    /// (E844b: a later assignment left the client on id 1 with no prompt).
    spawn_released: BTreeMap<u16, u64>,
    /// Completion handoffs awaiting a current owned binding snapshot.
    pending_binding_refreshes: VecDeque<(u64, u16)>,
    glass: crate::glass::Glass,
    customization: crate::customization_timer::Timers,
    world_ms: u64,
    vehicle_reports: nfs_world::vehicle_reports::Reports,
    trigger_volumes: nfs_world::trigger_volumes::Volumes,
    world_activation: Option<nfs_world::world_activation::Activation>,
    sequences: Option<nfs_world::sequences::Sequences>,
    /// Arrive sequences after garage exit (optional sequence content).
    arrivals: Option<nfs_world::arrival::Arrivals>,
    returns: Option<nfs_world::arrival::Arrivals>,
    inventory: Option<Inventory>,
    equipment: equipment::State,
    logic: Option<std::sync::Arc<crate::garage_logic::GarageLogic>>,
    logic_ghosts: BTreeMap<u16, BTreeMap<u16, u16>>,
    logic_vehicles_fired: BTreeSet<u16>,
    free_roam_presented: BTreeSet<u16>,
    scoring: Option<std::sync::Arc<crate::scoring::Content>>,
    scoring_state: scoring::State,
    pub(super) pending_content: VecDeque<nfs_world::content::Message>,
    pub(super) pending_rpcs: VecDeque<nfs_world::participants::HostRpc>,
    unsupported_rpcs: usize,
    unsupported_logic: usize,
    unsupported: crate::diagnostics::Unsupported<diagnostics::Route>,
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
    pub(super) fn purchase_ready(&self) -> bool {
        self.inventory.is_some()
            && self.persistent.is_some()
            && self.bank_notification(0).is_ok_and(|reply| reply.is_some())
    }

    fn bank_notification(
        &self,
        amount: u32,
    ) -> Result<Option<nfs_world::participants::HostRpc>, replication::Error> {
        let Some(bank) = self.roles.as_ref().and_then(|roles| roles.bank) else {
            return Ok(None);
        };
        let scene = self
            .players
            .objects()
            .scene(bank.scene)
            .ok_or(replication::Error::UnknownObject)?;
        let Some(replication::Initial::SubLevel { fields, .. }) =
            self.players.objects().get(scene).map(|o| &o.initial)
        else {
            return Err(replication::Error::TypeMismatch);
        };
        let rpc = fields
            .get(bank.serializer)
            .and_then(replication::sublevel::Initial::rpc)
            .ok_or(replication::Error::TypeMismatch)?;
        Ok(Some(nfs_world::participants::HostRpc::Bank(
            nfs_world::bank::Balance {
                endpoint: nfs_world::participants::Endpoint {
                    scene,
                    selector: rpc.selector,
                    serial: rpc.serial,
                },
                amount,
            },
        )))
    }

    /// A purchase adds a leaf or promotes a purchasable leaf in place, without
    /// equipping it or replacing a live car. ItemBuilder owns that transition.
    /// Other persistent game tables and previously owned records stay identical.
    pub(super) fn purchased_inventory(
        &mut self,
        garage: [Option<u64>; 5],
        persistent: Option<std::sync::Arc<crate::persistent::Loaded>>,
        items: std::sync::Arc<nfs_world::items::Collection>,
    ) -> Result<(), replication::Error> {
        let old = self.inventory.as_ref().ok_or(replication::Error::Shape)?;
        let before = self.persistent.as_ref().ok_or(replication::Error::Shape)?;
        let after = persistent.as_ref().ok_or(replication::Error::Shape)?;
        let slots = nfs_world::garage::Slots::new(garage)?;
        if slots != old.slots
            || after.garage() != garage
            || after.generation() < before.generation()
            || after.tables() != before.tables()
            || after.stores() != before.stores()
            || !purchases::valid_refresh(&old.items, &items)
        {
            return Err(replication::Error::Shape);
        }
        let reply = self
            .bank_notification(after.balance().0)?
            .ok_or(replication::Error::Unsupported)?;
        if self.pending_rpcs.len() >= nfs_world::session::MAX_RPC_QUEUE {
            return Err(replication::Error::Bound);
        }
        let inventory = Inventory { slots, items };
        if let Some(population) = &mut self.population {
            population.refresh_inventory(
                &self.players,
                HOST_SELECTOR as u8,
                self.persona,
                old,
                &inventory,
            )?;
        }
        self.inventory = Some(inventory);
        self.persistent = persistent;
        self.pending_rpcs.push_back(reply);
        Ok(())
    }

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
                if let Some(bank) = roles.bank {
                    let scene = players
                        .objects()
                        .scene(bank.scene)
                        .ok_or(replication::Error::UnknownObject)?;
                    let Some(replication::Initial::SubLevel { fields, .. }) =
                        players.objects().get(scene).map(|o| &o.initial)
                    else {
                        return Err(replication::Error::TypeMismatch);
                    };
                    let rpc = fields
                        .get(bank.serializer)
                        .and_then(replication::sublevel::Initial::rpc)
                        .ok_or(replication::Error::TypeMismatch)?;
                    replies.push(nfs_world::participants::HostRpc::Bank(
                        nfs_world::bank::Balance {
                            endpoint: nfs_world::participants::Endpoint {
                                scene,
                                selector: rpc.selector,
                                serial: rpc.serial,
                            },
                            amount: loaded.balance().0,
                        },
                    ));
                }
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
        self.stats.files_alive(now) && self.messages.alive(now)
    }
    /// The next host teleport due by `world_ms`, oldest first, once the
    /// participant's spawn assignment has left in an earlier tick (official
    /// E742: assignment, then the client's zone report; the teleport may not
    /// overtake the assignment). A participant without a scheduled
    /// assignment (no SpawnPoints scene) teleports when due.
    pub(super) fn due_teleport(&self, world_ms: u64) -> Option<nfs_world::teleport::Command> {
        let (at, command) = self.pending_teleports.front()?;
        if *at > world_ms
            || self
                .pending_spawns
                .iter()
                .any(|(_, n)| n.participant == command.participant)
            || self
                .spawn_released
                .get(&command.participant)
                .is_some_and(|released| *released >= world_ms)
        {
            return None;
        }
        Some(*command)
    }
    pub(super) fn teleport_sent(&mut self) {
        self.pending_teleports.pop_front();
    }
    /// Cars this connection's client simulates and reports movement for: the
    /// garage cars, then the world car (official E101 grants).
    pub(super) fn movement_objects(&self) -> BTreeSet<u16> {
        Self::ready_movement(
            self.players
                .owned_vehicles(HOST_SELECTOR as u8, self.persona),
            &self.world_cars,
        )
    }
    pub(super) fn new(persona: u64) -> Self {
        Self {
            stats: Accepting::default(),
            messages: frame::receive::Receiver::default(),
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
            deferred_deletions: BTreeMap::new(),
            pending_spawns: VecDeque::new(),
            held_glass: BTreeMap::new(),
            spawn_settled: BTreeSet::new(),
            pending_settled: VecDeque::new(),
            pending_teleports: VecDeque::new(),
            spawn_released: BTreeMap::new(),
            pending_binding_refreshes: VecDeque::new(),
            arrivals: None,
            returns: None,
            glass: crate::glass::Glass::default(),
            customization: crate::customization_timer::Timers::default(),
            world_ms: 0,
            vehicle_reports: nfs_world::vehicle_reports::Reports::default(),
            trigger_volumes: nfs_world::trigger_volumes::Volumes::default(),
            world_activation: None,
            sequences: None,
            inventory: None,
            equipment: equipment::State::default(),
            logic: None,
            logic_ghosts: BTreeMap::new(),
            logic_vehicles_fired: BTreeSet::new(),
            free_roam_presented: BTreeSet::new(),
            scoring: None,
            scoring_state: scoring::State::default(),
            pending_content: VecDeque::new(),
            pending_rpcs: VecDeque::new(),
            unsupported_rpcs: 0,
            unsupported_logic: 0,
            unsupported: crate::diagnostics::Unsupported::new("world", 0),
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
    pub(super) fn with_arrivals(mut self, arrivals: Option<nfs_world::arrival::Arrivals>) -> Self {
        self.arrivals = arrivals;
        self
    }
    pub(super) fn with_returns(mut self, returns: Option<nfs_world::arrival::Arrivals>) -> Self {
        self.returns = returns;
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
        let mut records = players.create_scenes(&levels, creations, profiles)?;
        records.extend(players.enable_player_manager()?);
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
        if self.returns.is_some() {
            use crate::garage_logic::Which;
            if self.logic.as_ref().is_some_and(|logic| {
                [Which::ReturnRequested, Which::Returning, Which::Returned]
                    .into_iter()
                    .all(|phase| logic.has(phase))
            }) && self.sequences.is_some()
                && self.vehicles.is_some()
            {
                participants.bind_return(&records, roles.startup)?;
            } else {
                tracing::warn!(
                    "garage return presentation or loading content absent; return stays unsupported"
                );
            }
        }
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
        let mut trigger_volumes = self.trigger_volumes.clone();
        trigger_volumes.bind(&records)?;
        let world_activation = roles
            .free_roam
            .as_ref()
            .map(|catalog| nfs_world::world_activation::Activation::bind(&records, catalog))
            .transpose()?;
        let sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS)?;
        if sections.len() > MAX_PENDING - self.pending.len() {
            return Err(replication::Error::Bound);
        }
        self.players = players;
        self.launchers = launchers;
        self.participants = participants;
        self.spawn_points = spawn_points;
        self.level_poll = level_poll;
        self.trigger_volumes = trigger_volumes;
        self.world_activation = world_activation;
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
            Err(error) => {
                self.diagnose_decode(error);
                return self.stats.invalid();
            }
        };
        let files = match self.stats.file_transition(&parsed) {
            Ok(files) => files,
            Err(error) => {
                self.diagnose_decode(frame::Error::File(error));
                return self.stats.invalid();
            }
        };
        let (messages, received) = match &parsed.messages {
            Some(input) => match self.messages.prepare(input, self.stats.now) {
                Ok(prepared) => prepared,
                Err(error) => {
                    if refusal::logged(self.stats.errors + 1) {
                        tracing::warn!(
                            connection = self.unsupported.connection(),
                            ?error,
                            "refused MessageEX ordering or chunk assembly"
                        );
                    }
                    return self.stats.invalid();
                }
            },
            None => (self.messages.clone(), Vec::new()),
        };
        let decoded = match received
            .iter()
            .map(|m| m.decode())
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(decoded) => decoded,
            Err(error) => {
                self.diagnose_decode(error);
                return self.stats.invalid();
            }
        };
        self.diagnose_unconsumed(&parsed, &decoded);
        let requests: Vec<_> = decoded
            .iter()
            .filter_map(|m| match m {
                frame::Message::CreatePlayer { name, flag, slot } => Some(Request {
                    name: name.clone(),
                    flag: *flag,
                    slot: *slot,
                }),
                _ => None,
            })
            .collect();
        let rpcs = decoded
            .iter()
            .filter_map(|m| match m {
                frame::Message::Opaque { index: 47, body } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();
        let logic_events = decoded
            .iter()
            .filter_map(|m| match m {
                frame::Message::Opaque { index: 82, body } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();
        if requests.is_empty()
            && rpcs.is_empty()
            && logic_events.is_empty()
            && parsed.movement.is_none()
        {
            self.messages = messages;
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
        let mut deferred_deletions = self.deferred_deletions.clone();
        let mut deferred_sections = Vec::new();
        let mut movement_section = None;
        let mut vehicle_reports = self.vehicle_reports.clone();
        let mut scoring_reported = BTreeSet::new();
        let mut scoring_reports = Vec::new();
        let mut trigger_volumes = self.trigger_volumes.clone();
        let mut world_activation = self.world_activation.clone();
        if let Some(movement) = &parsed.movement {
            let cars = world_cars.keys().copied().collect();
            match vehicle_reports.receive(
                &mut players,
                HOST_SELECTOR as u8,
                self.persona,
                movement,
                &cars,
                self.world_ms,
            ) {
                Ok(reports) if !reports.is_empty() => {
                    scoring_reported.extend(reports.iter().map(|r| r.id));
                    movement_section = Some(Section {
                        float_bits: None,
                        flag: false,
                        deleted: Vec::new(),
                        setup: Some(Setup::RawEscape([0; 3])),
                        records: reports,
                    })
                }
                Ok(_) => {}
                Err(error) => {
                    return self.refuse(error);
                }
            }
        }
        let mut scheduled_spawns = Vec::new();
        let mut held_glass = self.held_glass.clone();
        let mut spawn_settled = self.spawn_settled.clone();
        let mut scheduled_settled = Vec::new();
        let mut arrivals = self.arrivals.clone();
        let mut returns = self.returns.clone();
        let mut completed_returns = Vec::new();
        let mut scheduled_teleports = Vec::new();
        let mut scheduled_binding_refreshes = Vec::new();
        let mut sequences = self.sequences.clone();
        let mut logic_ghosts = self.logic_ghosts.clone();
        let mut unsupported_logic = 0;
        let mut glass = self.glass.clone();
        let mut customization = self.customization.clone();
        customization.retain_ghosts(|id| players.objects().get(id).is_some());
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
                // The client played the arrive sequence: stop it (E101, E742).
                if let Some(stop) = arrivals
                    .as_mut()
                    .map(|a| a.arrived(&message))
                    .transpose()?
                    .flatten()
                {
                    tracing::info!(
                        sequence = stop.sequence,
                        "owned arrive sequence played; stopping it"
                    );
                    responses.push(nfs_world::participants::HostRpc::SequenceStop(stop));
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
                    let fire = glass.receive(owner, &message)?;
                    match world_cars.get(&target.ghost) {
                        // A world car's replies wait for the participant's
                        // spawn-zone report; garage cars answer at once.
                        Some((participant, _)) if !spawn_settled.contains(participant) => {
                            let held = held_glass.entry(*participant).or_default();
                            if held.len() >= MAX_HELD_GLASS {
                                return Err(replication::Error::Bound);
                            }
                            held.push(fire);
                        }
                        _ => responses.push(nfs_world::participants::HostRpc::Event(fire)),
                    }
                    // A world car's first glass report also shows that the
                    // client has the car: assign its spawn point (E751), sent
                    // with a level poll once due (`release_spawns`).
                    if let Some(notification) =
                        Self::world_car_ready(&message, &mut world_cars, &mut spawn_points)?
                            .flatten()
                    {
                        responses.extend(
                            spawn_points
                                .set_presentation_ready(notification.participant, false)?
                                .map(nfs_world::participants::HostRpc::SpawnOccupied),
                        );
                        tracing::info!(
                            participant = notification.participant,
                            call = ?notification.call,
                            "owned world car created; spawn point assignment scheduled"
                        );
                        let mut bindings = players.bind_player_vehicle(
                            HOST_SELECTOR as u8,
                            self.persona,
                            notification.participant,
                            Some(target.ghost),
                        )?;
                        bindings.extend(players.actor_vehicle_ready(
                            HOST_SELECTOR as u8,
                            self.persona,
                            notification.participant,
                        )?);
                        deferred_sections.extend(Self::binding_sections(bindings)?);
                        responses.extend(Self::presentation_events(
                            self.logic.as_deref(),
                            crate::garage_logic::Which::WorldReady,
                            &players,
                            &logic_ghosts,
                            self.persona,
                            notification.participant,
                        )?);
                        if let Some(command) = Self::exit_teleport(
                            self.participants.exit_bindings(),
                            self.vehicles.as_deref().map(|c| &c.layout),
                            notification.participant,
                            target.ghost,
                        ) {
                            scheduled_teleports.push((self.world_ms + TELEPORT_DELAY_MS, command));
                        }
                        scheduled_spawns.push((self.world_ms + SPAWN_DELAY_MS, notification));
                    }
                    if let Some(section) =
                        Self::garage_car_deletion(&message, &mut deferred_deletions)
                    {
                        tracing::info!(
                            deleted = ?section.deleted,
                            "world car reported; garage car deleted"
                        );
                        deferred_sections.push(section);
                    }
                } else {
                    unsupported_logic += 1;
                    self.diagnose_logic(&message);
                }
            }
            Ok::<(), replication::Error>(())
        })();
        if let Err(error) = logic_result {
            return self.refuse(error);
        }
        for request in requests {
            match players.create(HOST_SELECTOR as u8, self.persona, request) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(error) => {
                    return self.refuse(error);
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
                    return self.refuse(error);
                }
            }
        }
        match players.sync_player_views() {
            Ok(views) => records.extend(views),
            Err(error) => {
                return self.refuse(error);
            }
        }
        match players.sync_player_manager() {
            Ok(record) => records.extend(record),
            Err(error) => {
                return self.refuse(error);
            }
        }
        if !records.is_empty() && self.pending.len() >= MAX_PENDING {
            return self.refuse(replication::Error::Bound);
        }
        let mut launchers = self.launchers.clone();
        let mut unsupported = 0;
        let mut exit_entries = Vec::new();
        let mut equipment = self.equipment.clone();
        for body in rpcs {
            match nfs_world::vehicle_nitrous::receive(
                &mut players,
                HOST_SELECTOR as u8,
                self.persona,
                body,
                |id| world_cars.get(&id).is_some_and(|(_, ready)| *ready),
            ) {
                Ok(Some(report)) => {
                    tracing::debug!(
                        vehicle = report.vehicle,
                        active = report.active,
                        client_seconds = report.client_seconds,
                        changed = report.record.is_some(),
                        "owned nitrous transition"
                    );
                    records.extend(report.record);
                    continue;
                }
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match self.scoring_report(&players, body) {
                Ok(Some(queued)) => {
                    if scoring_reports.len() >= scoring::MAX_QUEUED_REPORTS {
                        return self.refuse_rpc(replication::Error::Bound, body);
                    }
                    scoring_reports.push(queued);
                    continue;
                }
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match self.stage_equipment(&players, body, &mut equipment) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match Self::item_builder_begin(self.roles.as_ref(), &players, self.persona, body) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    return self.refuse_rpc(error, body);
                }
            }
            if let Some(sequences) = &mut sequences {
                match Self::sequence_completed(sequences, &players, self.persona, body) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        return self.refuse_rpc(error, body);
                    }
                }
            }
            if let Some(arrivals) = &mut arrivals {
                match Self::arrival_call(arrivals, &players, &mut participants, self.persona, body)
                {
                    Ok(Some(None)) => continue,
                    Ok(Some(Some((sequence, participant, mut chain)))) => {
                        let (mut rollout_records, rollout_deleted) = match arrivals.finish_rollout(
                            &mut players,
                            nfs_world::sequences::Owner {
                                connection: HOST_SELECTOR as u8,
                                persona: self.persona,
                                participant,
                            },
                        ) {
                            Ok(output) => output,
                            Err(error) => {
                                return self.refuse_rpc(error, body);
                            }
                        };
                        // The exit presentation's last logic leaves with the
                        // sequence (E101 115,292 ms).
                        let logic = match Self::arrive_logic(
                            self.logic.as_deref(),
                            crate::garage_logic::Which::Arrived,
                            &mut players,
                            participant,
                            &mut logic_ghosts,
                        ) {
                            Ok(logic) => logic,
                            Err(error) => {
                                return self.refuse_rpc(error, body);
                            }
                        };
                        tracing::info!(
                            sequence,
                            deleted = ?logic.deleted,
                            "owned arrive sequence completed; participant enters FreeRoam"
                        );
                        chain.extend(
                            logic
                                .events
                                .into_iter()
                                .map(nfs_world::participants::HostRpc::Event),
                        );
                        rollout_records.extend(logic.records);
                        scheduled_binding_refreshes.push((
                            self.world_ms.saturating_add(BINDING_REFRESH_DELAY_MS),
                            participant,
                        ));
                        exit_entries.push((
                            Section {
                                float_bits: None,
                                flag: false,
                                deleted: rollout_deleted.into_iter().chain(logic.deleted).collect(),
                                setup: (!rollout_records.is_empty()).then_some(
                                    nfs_world::replication::section::Setup::Packed {
                                        tag: None,
                                        width: None,
                                        axes: [None; 3],
                                    },
                                ),
                                records: rollout_records,
                            },
                            chain,
                        ));
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return self.refuse_rpc(error, body);
                    }
                }
            }
            if let (
                Some(returns),
                Some(population),
                Some(inventory),
                Some(content),
                Some(sequences),
            ) = (
                returns.as_mut(),
                population.as_mut(),
                self.inventory.as_ref(),
                self.vehicles.as_deref(),
                sequences.as_mut(),
            ) {
                match Self::finish_return_call(
                    body,
                    returns,
                    &mut players,
                    &mut participants,
                    population,
                    inventory,
                    content,
                    sequences,
                    arrivals.as_mut(),
                    self.logic.as_deref(),
                    &mut logic_ghosts,
                    &mut garage_presence,
                    self.roles.as_ref(),
                    self.persona,
                ) {
                    Ok(Some(None)) => continue,
                    Ok(Some(Some((entry, removed)))) => {
                        let Some((participant, _)) = world_cars.remove(&removed) else {
                            return self.refuse_rpc(replication::Error::UnknownObject, body);
                        };
                        completed_returns.push((participant, removed));
                        exit_entries.push(entry);
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return self.refuse_rpc(error, body);
                    }
                }
            }
            if let Some(population) = &mut population {
                match Self::vehicle_loaded(population, &players, self.persona, body) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        return self.refuse_rpc(error, body);
                    }
                }
            }
            if let Some(activation) = &mut world_activation {
                match activation.receive_ready(
                    body,
                    |id| players.owns_participant(HOST_SELECTOR as u8, self.persona, id),
                    |id| {
                        matches!(
                            participants.stage(id),
                            Some(
                                nfs_world::participants::Stage::FreeRoam
                                    | nfs_world::participants::Stage::ReturnPrompted
                            )
                        )
                    },
                ) {
                    Ok(Some(nfs_world::world_activation::Report::Retired { participant })) => {
                        // Queued unsubscription not yet applied by the client (E838).
                        tracing::info!(participant, "late world report from a retired participant");
                        continue;
                    }
                    Ok(Some(report)) => {
                        tracing::debug!(?report, "owned world volume or location report");
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return self.refuse_rpc(error, body);
                    }
                }
            }
            match trigger_volumes.receive_retired(
                body,
                |id| {
                    world_cars.contains_key(&id)
                        && players
                            .owned_vehicles(HOST_SELECTOR as u8, self.persona)
                            .contains(&id)
                },
                |id| players.objects().retired(id),
            ) {
                Ok(Some(nfs_world::trigger_volumes::Outcome::Retired { vehicle })) => {
                    tracing::info!(vehicle, "late trigger volume report for a removed car");
                    continue;
                }
                Ok(Some(nfs_world::trigger_volumes::Outcome::Observed(report))) => {
                    if report.changed {
                        tracing::info!(
                            scene = report.scene,
                            selector = report.selector,
                            vehicle = report.vehicle,
                            entered = report.entered,
                            seconds = report.seconds,
                            "owned trigger volume crossing"
                        );
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    return self.refuse_rpc(error, body);
                }
            }
            match level_poll.receive(body, |id| {
                players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
            }) {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(error) => {
                    return self.refuse_rpc(error, body);
                }
            }
            match spawn_points.report(body, |id| {
                players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
            }) {
                Ok(Some(report)) => {
                    tracing::info!(
                        participant = report.participant,
                        release = report.release,
                        current = report.current,
                        flag = ?report.flag,
                        "owned spawn request or release"
                    );
                    responses.extend(
                        report
                            .flag
                            .map(nfs_world::participants::HostRpc::SpawnOccupied),
                    );
                    // First current zone report of a world-car lifetime:
                    // publish spawn presentation and schedule held replies.
                    if !report.release
                        && report.current
                        && !spawn_settled.contains(&report.participant)
                        && world_cars.values().any(|(p, _)| *p == report.participant)
                    {
                        spawn_settled.insert(report.participant);
                        match Self::presentation_events(
                            self.logic.as_deref(),
                            crate::garage_logic::Which::WorldSpawn,
                            &players,
                            &logic_ghosts,
                            self.persona,
                            report.participant,
                        ) {
                            Ok(events) => responses.extend(events),
                            Err(error) => return self.refuse_rpc(error, body),
                        }
                        scheduled_settled
                            .push((self.world_ms + SETTLE_DELAY_MS, report.participant));
                        tracing::info!(
                            participant = report.participant,
                            held = held_glass.get(&report.participant).map_or(0, Vec::len),
                            "owned world car settled at the spawn; presentation sent, glass replies due"
                        );
                    }
                    // The car left the spawn zone: the Enter Garage prompt is
                    // gone, so the next field-100 request is a first request.
                    // Later visits report the first assignment's id (E843:
                    // every second-visit call was stale), so a stale release
                    // ends the prompt too.
                    if report.release
                        && participants.leave_prompt(report.participant)
                            == nfs_world::participants::Outcome::Advanced(Vec::new())
                    {
                        tracing::info!(
                            participant = report.participant,
                            "garage prompt ended; the car left the spawn zone"
                        );
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    return self.refuse_rpc(error, body);
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
                            let prompted = participants.take_prompted();
                            if let [participant] = prompted.as_slice() {
                                // First field-100 request of this zone visit:
                                // nothing goes out, so that PageDown repeats it.
                                // Before free roam the prompt is carried into
                                // it by the arrival.
                                tracing::info!(
                                    participant,
                                    stage = ?participants.stage(*participant),
                                    "garage prompt shown; the return waits for the confirming request"
                                );
                                continue;
                            }
                            let returning = participants.take_returning();
                            if let [participant] = returning.as_slice() {
                                let (
                                    Some(returns),
                                    Some(population),
                                    Some(inventory),
                                    Some(content),
                                ) = (
                                    returns.as_mut(),
                                    population.as_mut(),
                                    self.inventory.as_ref(),
                                    self.vehicles.as_deref(),
                                )
                                else {
                                    return self.refuse_rpc(replication::Error::Unsupported, body);
                                };
                                // Send garage slot refreshes in a following
                                // frame so the return entry stays within the
                                // outbound frame bound.
                                let (slots, chain): (Vec<_>, Vec<_>) =
                                    chain.into_iter().partition(|rpc| {
                                        matches!(
                                            rpc,
                                            nfs_world::participants::HostRpc::Participant(n)
                                                if matches!(
                                                    n.call,
                                                    nfs_world::participants::Call::GarageItem(_)
                                                        | nfs_world::participants::Call::NotifyTagged(_)
                                                )
                                        )
                                    });
                                match Self::begin_return(
                                    &mut players,
                                    returns,
                                    population,
                                    inventory,
                                    content,
                                    self.logic.as_deref(),
                                    &mut logic_ghosts,
                                    self.persona,
                                    *participant,
                                    chain,
                                ) {
                                    Ok(entries) => exit_entries.extend(entries),
                                    Err(error) => {
                                        return self.refuse_rpc(error, body);
                                    }
                                }
                                if !slots.is_empty() {
                                    exit_entries.push((returning::empty_section(), slots));
                                }
                                continue;
                            }
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
                                        return self.refuse_rpc(error, body);
                                    }
                                }
                            }
                            let mut garage_deletions = Vec::new();
                            let mut world_car = match exited.as_slice() {
                                [participant] => match Self::exit_world_car(
                                    &mut players,
                                    population.as_mut(),
                                    self.inventory.as_ref(),
                                    self.vehicles.as_deref(),
                                    *participant,
                                    self.persona,
                                ) {
                                    Ok(Some((section, deleted))) => {
                                        garage_deletions = deleted;
                                        Some(section)
                                    }
                                    Ok(None) => None,
                                    Err(error) => {
                                        return self.refuse_rpc(error, body);
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
                                        return self.refuse_rpc(error, body);
                                    }
                                }
                            }
                            for &participant in &exited {
                                // Retain object-lifetime diagnostics across
                                // repeated garage/world transitions.
                                let ids: Vec<u16> =
                                    players.objects().iter().map(|(id, _)| id).collect();
                                tracing::info!(
                                    participant,
                                    world_car = world_car.is_some(),
                                    objects = ids.len(),
                                    ?ids,
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
                                        return self.refuse_rpc(error, body);
                                    }
                                    for participant in &exited {
                                        spawn_settled.remove(participant);
                                        held_glass.remove(participant);
                                    }
                                    if let Some(car) = section.records.first()
                                        && !garage_deletions.is_empty()
                                    {
                                        if deferred_deletions.len() >= MAX_WORLD_CARS {
                                            return self
                                                .refuse_rpc(replication::Error::Bound, body);
                                        }
                                        deferred_deletions
                                            .insert(car.id, std::mem::take(&mut garage_deletions));
                                    }
                                    exit_entries.push((section, chain))
                                }
                                None => {
                                    match Self::start_arrivals(
                                        &mut players,
                                        &mut participants,
                                        arrivals.as_mut(),
                                        self.logic.as_deref(),
                                        &mut logic_ghosts,
                                        self.persona,
                                        chain,
                                    ) {
                                        Ok((Some(section), chain)) => {
                                            exit_entries.push((section, chain))
                                        }
                                        Ok((None, chain)) => responses.extend(chain),
                                        Err(error) => {
                                            return self.refuse_rpc(error, body);
                                        }
                                    }
                                }
                            }
                        }
                        Ok(nfs_world::participants::Outcome::Repeated) => {}
                        Ok(nfs_world::participants::Outcome::Unsupported) => {
                            unsupported += 1;
                            self.diagnose_rpc(body, &players);
                        }
                        Err(error) => {
                            return self.refuse_rpc(error, body);
                        }
                    }
                }
                Err(error) => {
                    return self.refuse_rpc(error, body);
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
                    return self.refuse(error);
                }
            }
        }
        match Self::start_sequences(
            &mut players,
            &participants,
            sequences.as_mut(),
            self.persona,
        ) {
            Ok(spawned) if !completed_returns.is_empty() => {
                // Publish the return completion and garage loading context
                // before starting the next waiting sequence.
                exit_entries.extend(spawned.into_iter().map(|section| (section, Vec::new())));
            }
            Ok(spawned) => sections.extend(spawned),
            Err(error) => {
                return self.refuse(error);
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
                return self.refuse(error);
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
                return self.refuse(error);
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
                return self.refuse(error);
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
                                        return self.refuse(replication::Error::Bound);
                                    }
                                    entries.push((section.remove(0), entry_replies));
                                }
                                Err(error) => {
                                    return self.refuse(error);
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
                return self.refuse(replication::Error::Bound);
            }
        }
        entries.extend(exit_entries);
        let map_cars = world_cars
            .iter()
            .map(|(&car, &(participant, _))| (participant, car))
            .collect();
        if let Some(catalog) = self
            .roles
            .as_ref()
            .and_then(|roles| roles.free_roam.as_ref())
        {
            match players.sync_location_service(
                HOST_SELECTOR as u8,
                self.persona,
                &map_cars,
                [0.; 3],
                catalog.location,
            ) {
                Ok(mut batch) => {
                    // One accepted movement sample can change both chassis and
                    // minimap position. Two frames per sample can outrun the
                    // host's bounded send queue and delay lifecycle messages.
                    // Both use world origin zero; retain ordinary framing if
                    // their combined timed frame would exceed the wire limit.
                    if let Some(section) = &mut movement_section {
                        let original_len = section.records.len();
                        section.records.append(&mut batch);
                        if !nfs_world::session::replication_frame(section, &[], 0).is_ok_and(
                            |wire| wire.len() <= nfs_world::application::OUTBOUND_FRAME_BITS,
                        ) {
                            batch = section.records.split_off(original_len);
                        }
                    }
                    records.extend(batch);
                }
                Err(error) => {
                    return self.refuse(error);
                }
            }
        }
        if let Err(error) = Self::bind_progression_launchers(
            self.progression.as_deref(),
            &players,
            &sections,
            &mut launchers,
            self.persona,
        ) {
            return self.refuse(error);
        }
        if let Some(section) = movement_section {
            deferred_sections.insert(0, section);
        }
        if spawned.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || responses.len() > nfs_world::session::MAX_RPC_QUEUE - self.pending_rpcs.len()
            || sections.len() + deferred_sections.len() + usize::from(!records.is_empty())
                > MAX_PENDING - self.pending.len()
            || entries.len() > MAX_PENDING - self.pending_entries.len()
            || scheduled_binding_refreshes.len()
                > MAX_PENDING.saturating_sub(self.pending_binding_refreshes.len())
        {
            return self.refuse(replication::Error::Bound);
        }
        for participant in participants.in_free_roam() {
            if !matches!(
                self.participants.stage(participant),
                Some(
                    nfs_world::participants::Stage::FreeRoam
                        | nfs_world::participants::Stage::ReturnPrompted
                )
            ) {
                tracing::info!(
                    participant,
                    "world entry completed; participant entered free roam"
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
                    "garage vehicles loaded; waiting for garage readiness"
                );
            }
        }
        self.population = population;
        self.garage_presence = garage_presence;
        self.spawn_points = spawn_points;
        self.level_poll = level_poll;
        trigger_volumes.retain_vehicles(|id| world_cars.contains_key(&id));
        self.world_cars = world_cars;
        self.trigger_volumes = trigger_volumes;
        self.vehicle_reports = vehicle_reports;
        self.world_activation = world_activation;
        self.deferred_deletions = deferred_deletions;
        self.pending_spawns.extend(scheduled_spawns);
        self.held_glass = held_glass;
        self.spawn_settled = spawn_settled;
        self.pending_settled.extend(scheduled_settled);
        self.arrivals = arrivals;
        self.returns = returns;
        for (participant, removed) in completed_returns {
            self.pending_spawns
                .retain(|(_, n)| n.participant != participant);
            self.pending_settled.retain(|(_, p)| *p != participant);
            self.held_glass.remove(&participant);
            self.spawn_settled.remove(&participant);
            self.spawn_released.remove(&participant);
            self.pending_teleports.retain(|(_, n)| n.vehicle != removed);
            self.pending_binding_refreshes
                .retain(|(_, p)| *p != participant);
            tracing::info!(
                participant,
                world_car = removed,
                "garage return completed; garage loading begins"
            );
        }
        self.pending_teleports.extend(scheduled_teleports);
        self.pending_binding_refreshes
            .extend(scheduled_binding_refreshes);
        glass.retain_vehicles(|id| self.players.objects().get(id).is_some());
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
        self.equipment = equipment;
        self.logic_ghosts = logic_ghosts;
        self.logic_vehicles_fired = logic_vehicles_fired;
        self.pending_content.extend(spawned.messages);
        self.unsupported_rpcs += unsupported;
        self.unsupported_logic += unsupported_logic;
        self.scoring_state.reported.extend(scoring_reported);
        self.scoring_state.reports.extend(scoring_reports);
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
        self.pending.extend(deferred_sections);
        self.pending_entries.extend(entries);
        self.messages = messages;
        self.stats.record(&parsed, files)
    }
}
