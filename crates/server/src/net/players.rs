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
mod challenges;
mod client_info;
mod crashing;
mod customization;
mod diagnostics;
mod equipment;
mod events;
mod garage;
mod recommendations;
pub(super) use equipment::EquipmentRequest;
pub(super) use garage::{GarageOperation, GarageRequest};
pub(super) use scoring::{AwardRequest, Credited};
mod launchers;
mod map_feed;
mod output;
mod prologue;
mod purchases;
mod refusal;
mod returning;
mod rivals;
mod scoring;
mod sequences;
mod traffic;

#[cfg(test)]
mod deployment_tests;
#[cfg(test)]
mod performance_tests;
#[cfg(test)]
mod test_wire;
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

/// A short label of a host notification for the outbound diagnostics log.
pub(super) fn describe_host_rpc(rpc: &nfs_world::participants::HostRpc) -> String {
    use nfs_world::participants::HostRpc;
    match rpc {
        HostRpc::Participant(n) => format!(
            "participant{}@{}/{}:{:?}",
            n.participant, n.endpoint.scene, n.endpoint.selector, n.call
        ),
        HostRpc::Event(f) => format!("fire{}@{}e{}", f.event, f.target.ghost, f.target.entity),
        HostRpc::Vehicle(b) => format!(
            "vehicle{}@{}/{}p{}",
            b.vehicle, b.endpoint.scene, b.endpoint.selector, b.participant
        ),
        HostRpc::SequenceStop(s) => format!("stop{}", s.sequence),
        HostRpc::WorldActivation(n) => format!(
            "activation{:?}@{}/{}p{:?}",
            n.call, n.endpoint.scene, n.endpoint.selector, n.participant
        ),
        other => format!("{other:?}").chars().take(48).collect(),
    }
}

pub(super) struct PlayerListener {
    stats: Accepting,
    messages: frame::receive::Receiver,
    client_info: replication::client_info::State,
    challenges: challenges::State,
    recommendations: recommendations::State,
    /// Latest accepted hardware report, owned only by this connection.
    performance_profile: Option<frame::PerformanceProfile>,
    persona: u64,
    roles: Option<crate::scene_roles::SceneRoles>,
    players: Players,
    launchers: nfs_world::launchers::Launchers,
    participants: nfs_world::participants::Lifecycle,
    persistent: Option<std::sync::Arc<crate::persistent::Loaded>>,
    progression: Option<std::sync::Arc<crate::progression::Content>>,
    prologue: Option<std::sync::Arc<crate::prologue::Content>>,
    scene_membership: Option<crate::prologue::membership::Session>,
    opening: crate::sequence_content::authored::Instances,
    selection: crate::sequence_content::authored::Instances,
    selection_execution: BTreeMap<u16, prologue::selection::State>,
    story_pending: VecDeque<super::items::story::Request>,
    intro_composites: prologue::finale::Composites,
    intro_modes: prologue::Modes,
    intro_movies: prologue::movies::Movies,
    vehicles: Option<std::sync::Arc<GarageContent>>,
    population: Option<Population>,
    traffic_content: Option<std::sync::Arc<nfs_content::traffic::Content>>,
    traffic_population: Option<crate::traffic_content::population::Population>,
    traffic_failed: bool,
    rival_content: Option<std::sync::Arc<crate::rival_content::Content>>,
    rival_population: Option<crate::rival_content::population::Population>,
    rivals_failed: bool,
    /// Human sync, rival-source collection, rival sync; fixed-size counters.
    optional_map_failures: [usize; 3],
    /// Waiting, arrive, crash: counted only when the fallback is committed.
    sequence_fallbacks: [usize; 3],
    scoring_failed: bool,
    ai: crate::ai_settings::AiSettings,
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
    pub(super) pending_speedwalls: VecDeque<nfs_world::speedwall::Progress>,
    /// World clock at which each participant's spawn assignment was queued.
    /// The teleport waits for it: the client adopts the assigned spawn id
    /// only when the assignment precedes its spawn-zone report, and the
    /// car reports the zone within tens of milliseconds of the teleport
    /// (E844b: a later assignment left the client on id 1 with no prompt).
    spawn_released: BTreeMap<u16, u64>,
    /// Completion handoffs awaiting a current owned binding snapshot.
    pending_binding_refreshes: VecDeque<(u64, u16)>,
    /// Garage returns whose teardown and staging wait for their world time.
    pending_returns: VecDeque<returning::PendingReturn>,
    /// World ms until which the client's calls are logged (a return began).
    return_trace_until: u64,
    /// Sections waiting in the world session's replication queue, set by the
    /// world loop before each scoring advance (E923: periodic scoring
    /// sections kept that queue at 31 of 32 entries all session and held
    /// every later section, garage-return events included, for ~2.6 s).
    pub(in crate::net) session_backlog: usize,
    glass: crate::glass::Glass,
    customization: crate::customization_timer::Timers,
    world_ms: u64,
    vehicle_reports: nfs_world::vehicle_reports::Reports,
    traffic_reports: nfs_world::vehicle_reports::Reports,
    trigger_volumes: nfs_world::trigger_volumes::Volumes,
    world_activation: Option<nfs_world::world_activation::Activation>,
    sequences: Option<nfs_world::sequences::Sequences>,
    /// Arrive sequences after garage exit (optional sequence content).
    arrivals: Option<nfs_world::arrival::Arrivals>,
    returns: Option<nfs_world::arrival::Arrivals>,
    crashes: Option<nfs_world::crash::Crashes>,
    inventory: Option<Inventory>,
    equipment: equipment::State,
    garage_commands: VecDeque<GarageRequest>,
    logic: Option<std::sync::Arc<crate::garage_logic::GarageLogic>>,
    logic_ghosts: BTreeMap<u16, BTreeMap<u16, u16>>,
    logic_vehicles_fired: BTreeSet<u16>,
    free_roam_presented: BTreeSet<u16>,
    scoring: Option<std::sync::Arc<crate::scoring::Content>>,
    scoring_state: scoring::State,
    events: Option<std::sync::Arc<crate::event_logic::catalog::Catalog>>,
    event_state: events::State,
    event_discovery: events::discovery::State,
    /// The intro participant's world volume memberships (E1194).
    world_volumes: prologue::volumes::State,
    /// Last logged waiting-sequence readiness per participant.
    sequence_readiness: BTreeMap<u16, nfs_world::sequences::Readiness>,
    /// Participants in an owned event: free-roam scoring is retired meanwhile.
    event_suspended: BTreeSet<u16>,
    /// The session's replication clock origin, set by the world loop each pass.
    pub(in crate::net) replication_origin_ms: Option<u64>,
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
            .field(
                "performance_profile_received",
                &self.performance_profile.is_some(),
            )
            .field("last_error", &self.last_error)
            .finish()
    }
}
/// Whether the session admits this entry once queued, measured as the
/// session measures it (timestamp included).
pub(super) fn entry_admitted(
    section: &Section,
    calls: &[nfs_world::participants::HostRpc],
) -> bool {
    nfs_world::session::queued_replication_bits(section, calls)
        .is_ok_and(|bits| bits <= nfs_world::session::MAX_REPLICATION_FRAME_BITS)
}
/// Take the leading calls that still fit `section` as one entry (a reply
/// group carries at most fifteen).
pub(super) fn fitting_calls(
    section: &Section,
    remaining: &mut &[nfs_world::participants::HostRpc],
) -> Vec<nfs_world::participants::HostRpc> {
    let mut chunk = Vec::new();
    while let Some((call, tail)) = remaining.split_first() {
        if chunk.len() >= 15 {
            break;
        }
        chunk.push(*call);
        if !entry_admitted(section, &chunk) {
            chunk.pop();
            break;
        }
        *remaining = tail;
    }
    chunk
}

/// One entry as entries the session admits: the section with the leading
/// calls that fit, then call-only entries for the rest, in order.
/// Presentation adds calls to entries after they were sized (E1191: a
/// fifteen-call garage completion became eighteen and closed the world).
pub(super) fn admitted_entries(
    section: Section,
    calls: Vec<nfs_world::participants::HostRpc>,
) -> Result<Vec<(Section, Vec<nfs_world::participants::HostRpc>)>, replication::Error> {
    if entry_admitted(&section, &calls) {
        return Ok(vec![(section, calls)]);
    }
    if !entry_admitted(&section, &[]) {
        return Err(replication::Error::Bound);
    }
    let mut remaining = calls.as_slice();
    let chunk = fitting_calls(&section, &mut remaining);
    let mut entries = vec![(section, chunk)];
    while !remaining.is_empty() {
        let section = returning::empty_section();
        let chunk = fitting_calls(&section, &mut remaining);
        if chunk.is_empty() {
            return Err(replication::Error::Bound);
        }
        entries.push((section, chunk));
    }
    defer_deletions(&mut entries);
    Ok(entries)
}

/// An entry's calls run before its section's deletions, but a later entry's
/// calls run after them: the session refuses a call naming an object an
/// earlier entry deleted (UnknownObject). Deletions a later call names move
/// to the last entry, or to a deletion-only entry after it when the last one
/// would no longer be admitted.
pub(super) fn defer_deletions(entries: &mut Vec<(Section, Vec<nfs_world::participants::HostRpc>)>) {
    let Some(last) = entries.len().checked_sub(1) else {
        return;
    };
    let mut moved = Vec::new();
    for index in 0..last {
        let later: BTreeSet<u16> = entries[index + 1..]
            .iter()
            .flat_map(|(_, calls)| calls.iter().flat_map(|call| call.objects()))
            .collect();
        let (keep, defer): (Vec<u16>, Vec<u16>) = entries[index]
            .0
            .deleted
            .iter()
            .partition(|id| !later.contains(id));
        entries[index].0.deleted = keep;
        moved.extend(defer);
    }
    if moved.is_empty() {
        return;
    }
    let (section, calls) = &mut entries[last];
    let before = section.deleted.len();
    section.deleted.extend(&moved);
    if !entry_admitted(section, calls) {
        section.deleted.truncate(before);
        let mut deletions = returning::empty_section();
        deletions.deleted = moved;
        entries.push((deletions, Vec::new()));
    }
}
impl PlayerListener {
    /// Queue entries for the world flush. The flush refuses an entry the
    /// session does not admit and closes the world (E1186, E1188), so each
    /// entry is measured here, where its producer is still known: test builds
    /// fail at the producer, release builds log it before the flush refuses it.
    #[track_caller]
    pub(super) fn queue_entries(
        &mut self,
        entries: impl IntoIterator<Item = (Section, Vec<nfs_world::participants::HostRpc>)>,
    ) {
        for (section, calls) in entries {
            if !entry_admitted(&section, &calls) {
                let producer = std::panic::Location::caller();
                let bits = nfs_world::session::queued_replication_bits(&section, &calls);
                if cfg!(debug_assertions) {
                    panic!(
                        "entry from {producer} is not admitted by the session \
                         ({bits:?} bits, {} records, {} calls)",
                        section.records.len(),
                        calls.len()
                    );
                }
                tracing::error!(
                    connection = self.unsupported.connection(),
                    %producer,
                    ?bits,
                    limit = nfs_world::session::MAX_REPLICATION_FRAME_BITS,
                    records = section.records.len(),
                    calls = calls.len(),
                    "owned entry exceeds the session frame; the flush will refuse it and close the world"
                );
            }
            self.pending_entries.push_back((section, calls));
        }
    }

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
        let scene_membership = self.stage_scene_membership(&mut players, &mut participants)?;
        sections.extend(prologue::movies::sections(
            scene_membership.records,
            scene_membership.deleted,
        )?);
        if spawned.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || replies.len() > nfs_world::session::MAX_RPC_QUEUE - self.pending_rpcs.len()
            || sections.len() > MAX_PENDING - self.pending.len()
        {
            return Err(replication::Error::Bound);
        }
        self.scene_membership = scene_membership.session;
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
                let intro = self
                    .prologue
                    .as_ref()
                    .is_some_and(|content| content.active(&restored));
                replies.extend(if intro {
                    participants.enter_intro_with_actor(binding)?
                } else {
                    participants.enter_garage_with_actor(binding)?
                });
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
                    // The prologue gets only the spawn logic its events address
                    // (official career start fade control 480 and 482; E1183,
                    // E1184), never the garage presentation.
                    let out = if intro {
                        logic.produce_intro_spawn(players, participant, ghosts)?
                    } else {
                        let scores = logic
                            .requires_reputation()
                            .then(|| {
                                crate::garage_logic::ReputationScores::load(loaded)
                                    .map_err(|_| replication::Error::Shape)
                            })
                            .transpose()?;
                        logic.produce_with_reputation(
                            crate::garage_logic::Which::Spawn,
                            players,
                            participant,
                            ghosts,
                            scores,
                        )?
                    };
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
            || !self.pending.is_empty()
            || !self.pending_rpcs.is_empty()
            || !self.pending_entries.is_empty()
            || !self.pending_content.is_empty()
            || (self
                .crashes
                .as_ref()
                .is_some_and(|c| c.relocating(command.participant))
                && self.session_backlog != 0)
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
        let mut command = *command;
        if let Some(crashes) = &self.crashes
            && crashes.relocating(command.participant)
        {
            if !self.event_state.inputs.is_empty() {
                return None;
            }
            if !crashes.route_is_current(
                command.participant,
                self.event_state
                    .recovery_key(command.participant, command.vehicle),
            ) {
                command.speed = 0.;
                if let Some(destination) = crashes.fallback_destination(command.participant) {
                    command.position = destination.position;
                    command.basis = destination.basis;
                }
            }
        }
        Some(command)
    }
    pub(super) fn teleport_sent(&mut self) {
        if let Some((_, command)) = self.pending_teleports.pop_front()
            && let Some(crashes) = &mut self.crashes
        {
            crashes.relocation_sent(command.participant, command.vehicle);
        }
    }
    /// Cars this connection's client simulates and reports movement for: the
    /// garage cars, then the world car (official E101 grants).
    pub(super) fn movement_objects(&self) -> BTreeSet<u16> {
        let mut objects = Self::ready_movement(
            self.players
                .owned_vehicles(HOST_SELECTOR as u8, self.persona),
            &self.world_cars,
        );
        objects.extend(self.delegated_ai(&self.players).keys());
        objects
    }
    fn delegated_ai(&self, players: &Players) -> BTreeMap<u16, u64> {
        let mut cars = self
            .traffic_population
            .as_ref()
            .map(|p| p.delegated(players, HOST_SELECTOR as u8, self.persona))
            .unwrap_or_default();
        if let Some(rivals) = &self.rival_population {
            cars.extend(rivals.delegated(players, HOST_SELECTOR as u8, self.persona));
        }
        cars.extend(self.event_state.delegated(players, self.persona));
        for (active, _, _) in self.intro_modes.values() {
            cars.extend(active.delegated(players));
        }
        cars
    }
    pub(super) fn new(persona: u64) -> Self {
        Self {
            stats: Accepting::default(),
            messages: frame::receive::Receiver::default(),
            // The PC client has one local slot per connection. Remote players
            // do not change this constructor context or share its inbound IDs.
            client_info: replication::client_info::State::new(1).expect("PC local slot context"),
            challenges: challenges::State::default(),
            recommendations: recommendations::State::default(),
            performance_profile: None,
            persona,
            roles: None,
            players: Players::default(),
            launchers: nfs_world::launchers::Launchers::default(),
            participants: nfs_world::participants::Lifecycle::default(),
            persistent: None,
            progression: None,
            prologue: None,
            scene_membership: None,
            opening: crate::sequence_content::authored::Instances::default(),
            selection: crate::sequence_content::authored::Instances::default(),
            selection_execution: BTreeMap::new(),
            story_pending: VecDeque::new(),
            intro_composites: BTreeMap::new(),
            intro_modes: BTreeMap::new(),
            intro_movies: BTreeMap::new(),
            vehicles: None,
            population: None,
            traffic_content: None,
            traffic_population: None,
            traffic_failed: false,
            rival_content: None,
            rival_population: None,
            rivals_failed: false,
            optional_map_failures: [0; 3],
            sequence_fallbacks: [0; 3],
            scoring_failed: false,
            ai: crate::ai_settings::AiSettings::default(),
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
            pending_speedwalls: VecDeque::new(),
            spawn_released: BTreeMap::new(),
            pending_binding_refreshes: VecDeque::new(),
            pending_returns: VecDeque::new(),
            return_trace_until: 0,
            session_backlog: 0,
            arrivals: None,
            returns: None,
            crashes: None,
            glass: crate::glass::Glass::default(),
            customization: crate::customization_timer::Timers::default(),
            world_ms: 0,
            vehicle_reports: nfs_world::vehicle_reports::Reports::default(),
            traffic_reports: nfs_world::vehicle_reports::Reports::default(),
            trigger_volumes: nfs_world::trigger_volumes::Volumes::default(),
            world_activation: None,
            sequences: None,
            inventory: None,
            equipment: equipment::State::default(),
            garage_commands: VecDeque::new(),
            logic: None,
            logic_ghosts: BTreeMap::new(),
            logic_vehicles_fired: BTreeSet::new(),
            free_roam_presented: BTreeSet::new(),
            scoring: None,
            scoring_state: scoring::State {
                trace: std::env::var_os("NFS_SCORING_TRACE").is_some(),
                ..scoring::State::default()
            },
            events: None,
            event_state: events::State::default(),
            event_discovery: events::discovery::State::default(),
            world_volumes: prologue::volumes::State::default(),
            sequence_readiness: BTreeMap::new(),
            event_suspended: BTreeSet::new(),
            replication_origin_ms: None,
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
    pub(super) fn with_traffic(
        mut self,
        content: Option<std::sync::Arc<nfs_content::traffic::Content>>,
    ) -> Self {
        self.traffic_content = content;
        self
    }
    pub(super) fn with_ai_settings(mut self, ai: crate::ai_settings::AiSettings) -> Self {
        self.ai = ai;
        self
    }
    /// New player-chosen amounts while the session runs. Populations read
    /// them on their next tick: more traffic fills in at the spawn pacing,
    /// less lets cars retire as they leave the retain range, and a lower
    /// rival count retires rivals only where no car is shown.
    pub(super) fn set_ai_settings(&mut self, ai: crate::ai_settings::AiSettings) {
        self.ai = ai;
        if let Some(traffic) = &mut self.traffic_population {
            traffic.set_traffic_percent(ai.traffic_percent());
        }
        if let Some(rivals) = &mut self.rival_population {
            rivals.set_limit(ai.rival_limit());
        }
    }
    pub(super) fn with_arrivals(mut self, arrivals: Option<nfs_world::arrival::Arrivals>) -> Self {
        self.arrivals = arrivals;
        self
    }
    pub(super) fn with_rivals(
        mut self,
        content: Option<std::sync::Arc<crate::rival_content::Content>>,
    ) -> Self {
        self.rival_content = content;
        self
    }
    pub(super) fn with_returns(mut self, returns: Option<nfs_world::arrival::Arrivals>) -> Self {
        self.returns = returns;
        self
    }
    pub(super) fn with_crashes(mut self, crashes: Option<nfs_world::crash::Crashes>) -> Self {
        self.crashes = crashes;
        self
    }
    pub(super) fn with_prologue(
        mut self,
        content: Option<std::sync::Arc<crate::prologue::Content>>,
    ) -> Self {
        self.prologue = content;
        self
    }
    fn stage_scene_membership(
        &self,
        players: &mut Players,
        participants: &mut nfs_world::participants::Lifecycle,
    ) -> Result<crate::prologue::membership::Staged, replication::Error> {
        crate::prologue::membership::stage(
            self.prologue
                .as_ref()
                .and_then(|content| content.membership.as_ref()),
            self.scene_membership.as_ref(),
            players,
            (HOST_SELECTOR as u8, self.persona),
            participants,
        )
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
        let logic = match (
            &self.logic,
            self.prologue
                .as_ref()
                .and_then(|content| content.membership.as_ref()),
        ) {
            (Some(logic), Some(content)) => Some(std::sync::Arc::new(
                logic.without_assets(&content.assets().collect())?,
            )),
            _ => self.logic.clone(),
        };
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
        let traffic_population = self
            .traffic_content
            .as_ref()
            .map(|content| {
                crate::traffic_content::population::Population::new(bindings.clone(), content)
                    .map(|p| p.with_traffic_percent(self.ai.traffic_percent()))
            })
            .transpose()?;
        let rival_population = self
            .rival_content
            .as_ref()
            .map(|content| {
                if population.is_none() {
                    return Err(replication::Error::Shape);
                }
                crate::rival_content::population::Population::new(&bindings, content.clone())
                    .map(|p| p.with_limit(self.ai.rival_limit()))
            })
            .transpose()?;
        let rpc = Rpc {
            selector: 0,
            serial: Serial::new(0).ok_or(replication::Error::Shape)?,
        };
        let profiles = content
            .scene_profiles
            .as_ref()
            .ok_or(replication::Error::Unsupported)?;
        let scenes = genesis::tree(rpc, &bindings, profiles, &roles.traffic)?;
        let mut players = self.players.clone();
        let mut records = players.create_scene_tree(scenes, profiles)?;
        records.extend(players.enable_player_manager()?);
        let mut launchers = self.launchers.clone();
        if let Some(catalog) = &content.launchers {
            launchers.bind(&records, catalog)?;
        }
        let mut participants = self.participants.clone();
        participants.bind(&records, roles.gameplay, roles.startup)?;
        if self.prologue.is_some() && !participants.bind_intro(&records, roles.startup)? {
            return Err(replication::Error::UnknownObject);
        }
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
        let mut sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS)?;
        let scene_membership = self.stage_scene_membership(&mut players, &mut participants)?;
        sections.extend(prologue::movies::sections(
            scene_membership.records,
            scene_membership.deleted,
        )?);
        if sections.len() > MAX_PENDING - self.pending.len() {
            return Err(replication::Error::Bound);
        }
        self.logic = logic;
        self.scene_membership = scene_membership.session;
        self.players = players;
        self.launchers = launchers;
        self.participants = participants;
        self.spawn_points = spawn_points;
        self.level_poll = level_poll;
        self.trigger_volumes = trigger_volumes;
        self.world_activation = world_activation;
        self.pending.extend(sections);
        self.population = population;
        self.traffic_population = traffic_population;
        self.rival_population = rival_population;
        self.roles = Some(roles.clone());
        Ok(())
    }
}

impl Listener for PlayerListener {
    fn frame(&mut self, queued: Queued<'_>) -> bool {
        let mut parsed = match self.stats.parse(queued) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.diagnose_decode(error);
                return self.stats.invalid();
            }
        };
        let client_info = match self.prepare_client_info(queued.data, &mut parsed) {
            Ok(next) => next,
            Err(()) => return self.stats.invalid(),
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
        self.diagnose_unconsumed(&decoded);
        let performance_profile = decoded.iter().rev().find_map(|message| match message {
            frame::Message::PerformanceProfile(value) => Some(value.clone()),
            _ => None,
        });
        let challenges = match self.prepare_challenges(&decoded) {
            Ok(next) => next,
            Err(error) => return self.refuse(error),
        };
        let recommendations = match self.prepare_recommendations(&decoded) {
            Ok(next) => next,
            Err(error) => return self.refuse(error),
        };
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
            self.client_info = client_info;
            if let Some(profile) = performance_profile {
                self.performance_profile = Some(profile);
            }
            if let Some(next) = challenges {
                self.challenges = next;
            }
            if let Some(next) = recommendations {
                self.recommendations = next;
            }
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
        let mut traffic_reports = self.traffic_reports.clone();
        let mut scoring_reported = BTreeSet::new();
        let mut scoring_reports = Vec::new();
        // Fuel, clock anchors and transition snapshots commit with this frame.
        let mut nitrous = self.scoring_state.nitrous.clone();
        let mut nitrous_reported = BTreeSet::new();
        let mut trigger_volumes = self.trigger_volumes.clone();
        let mut world_activation = self.world_activation.clone();
        let mut crashes = self.crashes.clone();
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
                    for report in &reports {
                        if let Some(fuel) = nitrous.get_mut(&report.id)
                            && let Ok(physics) = players.vehicle_physics(
                                HOST_SELECTOR as u8,
                                self.persona,
                                report.id,
                            )
                        {
                            let [high, low, _] = physics.identity_words;
                            fuel.observe_clock(
                                f64::from_bits((u64::from(high) << 32) | u64::from(low)),
                                self.world_ms,
                            );
                        }
                    }
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
            if let Some(crashes) = &mut crashes
                && let Err(error) = Self::observe_crashes(
                    crashes,
                    &players,
                    &participants,
                    &world_cars,
                    self.persona,
                    movement,
                    (&self.event_state, self.events.as_deref()),
                )
            {
                return self.refuse(error);
            }
            if self.traffic_population.is_some()
                || self.rival_population.is_some()
                || !self
                    .event_state
                    .delegated(&players, self.persona)
                    .is_empty()
                || self
                    .intro_modes
                    .values()
                    .any(|(a, _, _)| !a.delegated(&players).is_empty())
            {
                let delegated = self.delegated_ai(&players);
                match traffic_reports.receive_ai(
                    &mut players,
                    HOST_SELECTOR as u8,
                    self.persona,
                    movement,
                    &delegated,
                    self.world_ms,
                ) {
                    Ok(reports) if !reports.is_empty() => {
                        deferred_sections.extend(reports.into_iter().map(|record| Section {
                            float_bits: None,
                            flag: false,
                            deleted: Vec::new(),
                            setup: Some(Setup::RawEscape([0; 3])),
                            records: vec![record],
                        }))
                    }
                    Ok(_) => {}
                    Err(error) => return self.refuse(error),
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
        let mut scheduled_returns = Vec::new();
        let mut return_trace = false;
        let mut sequences = self.sequences.clone();
        let mut logic_ghosts = self.logic_ghosts.clone();
        let mut opening = self.opening.clone();
        let mut selection = self.selection.clone();
        let mut selection_execution = self.selection_execution.clone();
        let mut story_pending = self.story_pending.clone();
        let mut intro_modes = self.intro_modes.clone();
        let mut intro_movies = self.intro_movies.clone();
        let mut intro_composites = self.intro_composites.clone();
        let mut unsupported_logic = 0;
        let mut event_inputs = Vec::new();
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
                    self.persona,
                )?;
            }
            for body in logic_events {
                let message = nfs_world::logic::Message::decode(82, body)
                    .map_err(|_| replication::Error::Shape)?;
                if let Some(content) = self.prologue.as_deref()
                    && prologue::finale::signal(
                        content,
                        &mut intro_composites,
                        &players,
                        self.persona,
                        &message,
                        &mut responses,
                    )?
                {
                    continue;
                }
                if let nfs_world::logic::Message::Reached { target, .. } = &message
                    && let Some(owner) = selection.owner(target.ghost)
                {
                    if owner.connection != HOST_SELECTOR as u8 || owner.persona != self.persona {
                        return Err(replication::Error::UnknownObject);
                    }
                    let state = selection_execution
                        .get_mut(&owner.participant)
                        .ok_or(replication::Error::UnknownObject)?;
                    let effects = state.signal(
                        &message,
                        &mut prologue::selection::Context {
                            content: self
                                .prologue
                                .as_deref()
                                .ok_or(replication::Error::Unsupported)?,
                            vehicles: self
                                .vehicles
                                .as_deref()
                                .ok_or(replication::Error::Unsupported)?,
                            progression: self
                                .progression
                                .as_ref()
                                .ok_or(replication::Error::Unsupported)?,
                            players: &mut players,
                            population: population
                                .as_mut()
                                .ok_or(replication::Error::Unsupported)?,
                            instances: &mut selection,
                            owner,
                            inventory: self.inventory.as_ref(),
                            seconds: self
                                .event_seconds(self.world_ms)
                                .ok_or(replication::Error::Shape)?,
                        },
                    )?;
                    if effects.jobs.len() > MAX_PENDING.saturating_sub(story_pending.len()) {
                        return Err(replication::Error::Bound);
                    }
                    story_pending.extend(effects.jobs);
                    responses.extend(effects.calls);
                    if !effects.deleted.is_empty() {
                        deferred_sections.push(Section {
                            float_bits: None,
                            flag: false,
                            setup: None,
                            records: vec![],
                            deleted: effects.deleted,
                        });
                    }
                    continue;
                }
                if let Some(content) = self.prologue.as_deref()
                    && Self::opening_signal(
                        content,
                        &mut opening,
                        &players,
                        self.persona,
                        &message,
                        &mut responses,
                    )?
                {
                    continue;
                }
                if self.event_claims_logic(&message) {
                    event_inputs.push(Self::event_input_logic(&message));
                    continue;
                }
                if let Some(content) = self.prologue.as_deref()
                    && Self::driving_signal(
                        content,
                        &mut intro_modes,
                        &players,
                        &message,
                        &mut responses,
                    )?
                {
                    continue;
                }
                let streamed = sequences
                    .as_mut()
                    .map(|s| s.streaming_event(&message))
                    .transpose()?
                    .unwrap_or(false);
                if let nfs_world::logic::Message::Reached { event, target, .. } = &message
                    && matches!(
                        *event,
                        nfs_world::sequences::STREAMING_LOADED
                            | nfs_world::sequences::STREAMING_UNLOADED
                    )
                {
                    tracing::info!(
                        ghost = target.ghost,
                        entity = target.entity,
                        loaded = *event == nfs_world::sequences::STREAMING_LOADED,
                        consumed = streamed,
                        "streaming gate event"
                    );
                }
                if streamed {
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
                    let nfs_world::logic::Message::Reached { target, player, .. } = &message else {
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
                        .or_else(|| self.rival_glass_owner(&players, target.ghost, *player))
                        .ok_or(replication::Error::UnknownObject)?;
                    let fire = glass.receive(owner, &message)?;
                    if players.temporary_vehicle(
                        HOST_SELECTOR as u8,
                        self.persona,
                        owner.participant,
                    ) == Some(target.ghost)
                        || (participants.intro_chapter(owner.participant).is_some()
                            && world_cars
                                .get(&target.ghost)
                                .is_some_and(|(p, _)| *p == owner.participant))
                    {
                        responses.push(nfs_world::participants::HostRpc::Event(fire));
                        if let Some((records, calls, command)) = Self::intro_car_ready(
                            self.prologue.as_deref(),
                            &mut players,
                            &mut participants,
                            self.persona,
                            owner.participant,
                            target.ghost,
                        )? {
                            deferred_sections.extend(Self::binding_sections(records)?);
                            responses.extend(calls);
                            scheduled_teleports.push((self.world_ms, command));
                            world_cars.insert(target.ghost, (owner.participant, true));
                            // The official host assigns the intro car its spawn
                            // point once per assignment lifetime (career start
                            // 38618 and 563021 ms); its garage-zone report
                            // during the return cinematic needs it (E1183).
                            if spawn_points.bindings().is_some()
                                && spawn_points.assigned(owner.participant).is_none()
                            {
                                let value = spawn_points.next_value(owner.participant);
                                let notification = spawn_points.assign(owner.participant, value)?;
                                scheduled_spawns
                                    .push((self.world_ms + SPAWN_DELAY_MS, notification));
                            }
                        }
                        continue;
                    }
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
        let mut garage_commands = self.garage_commands.clone();
        let delegated = self.delegated_ai(&players);
        for body in rpcs {
            if self.event_claims_rpc(body) {
                event_inputs.push(Self::event_input_rpc(body));
                continue;
            }
            if self.world_ms < self.return_trace_until
                && let Some(label) = refusal::route_label(body)
            {
                tracing::info!(
                    selector = label.selector,
                    method = label.method,
                    references = ?label.references,
                    "client call during a garage return"
                );
            }
            match nfs_world::vehicle_nitrous::receive_with_ai(
                &mut players,
                HOST_SELECTOR as u8,
                self.persona,
                body,
                |id| world_cars.get(&id).is_some_and(|(_, ready)| *ready),
                &delegated,
            ) {
                Ok(Some(report)) => {
                    tracing::info!(
                        vehicle = report.vehicle,
                        active = report.active,
                        client_seconds = report.client_seconds,
                        changed = report.record.is_some(),
                        "driver nitrous transition"
                    );
                    records.extend(report.record);
                    if let Some(fuel) = nitrous.get_mut(&report.vehicle) {
                        if fuel
                            .request(report.active, report.client_seconds, self.world_ms)
                            .is_err()
                        {
                            return self.refuse_rpc(replication::Error::Bound, body);
                        }
                        nitrous_reported.insert(report.vehicle);
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match nfs_world::vehicle_chassis::receive(
                &mut players,
                HOST_SELECTOR as u8,
                self.persona,
                body,
                |id| world_cars.get(&id).is_some_and(|(_, ready)| *ready),
                &delegated,
            ) {
                Ok(Some(nfs_world::vehicle_chassis::Report::Horn { record, .. })) => {
                    records.extend(record.map(|r| *r));
                    continue;
                }
                Ok(Some(nfs_world::vehicle_chassis::Report::DriftExit { vehicle, value })) => {
                    // No challenge gameplay hook is registered in free roam.
                    // The independent REP graphs retain their own query inputs.
                    tracing::debug!(vehicle, value, "driver drift exit without a challenge hook");
                    continue;
                }
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match nfs_world::vehicle_wheels::receive(
                &mut players,
                HOST_SELECTOR as u8,
                self.persona,
                body,
                |id| world_cars.get(&id).is_some_and(|(_, ready)| *ready),
                &delegated,
            ) {
                Ok(Some(report)) => {
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
            match self.stage_garage(&players, body, &mut garage_commands) {
                Ok(true) => continue,
                Ok(false) => {}
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
            if let Some(content) = self.prologue.as_deref() {
                match prologue::finale::input(
                    &mut intro_composites,
                    &mut players,
                    &mut participants,
                    self.persona,
                    body,
                ) {
                    Ok(Some(out)) => {
                        deferred_sections.extend(match prologue::movies::sections(vec![], out.0) {
                            Ok(s) => s,
                            Err(e) => return self.refuse_rpc(e, body),
                        });
                        responses.extend(out.1);
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => return self.refuse_rpc(e, body),
                }
                match prologue::movies::input(
                    content,
                    &mut intro_movies,
                    &mut players,
                    &mut participants,
                    prologue::movies::Input {
                        persona: self.persona,
                        gameplay: self.roles.as_ref().map(|r| r.gameplay),
                        seconds: self.world_ms as f64 / 1000.,
                    },
                    body,
                ) {
                    Ok(Some(out)) => {
                        deferred_sections.extend(
                            match prologue::movies::sections(out.records, out.deleted) {
                                Ok(sections) => sections,
                                Err(error) => return self.refuse_rpc(error, body),
                            },
                        );
                        responses.extend(out.calls);
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => return self.refuse_rpc(error, body),
                }
            }
            match participants.receive_intro(body, |id| {
                players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
            }) {
                Ok(Some(nfs_world::participants::Outcome::Advanced(calls))) => {
                    responses.extend(
                        calls
                            .into_iter()
                            .map(nfs_world::participants::HostRpc::Participant),
                    );
                    continue;
                }
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match prologue::selection::completed(&mut selection, &mut players, self.persona, body) {
                Ok(Some(Some(ghost))) => {
                    if let Some(content) =
                        self.prologue.as_deref().filter(|c| c.first_race.is_some())
                    {
                        let transition = (|| {
                            let owner = selection
                                .owner(ghost)
                                .ok_or(replication::Error::UnknownObject)?;
                            opening.retire_completed(&players, owner)?;
                            let nfs_world::participants::Outcome::Advanced(chain) =
                                participants.intro_selection_complete(owner.participant)
                            else {
                                return Err(replication::Error::Shape);
                            };
                            let mut out = crate::prologue::driving::Output::default();
                            prologue::garage::notify(
                                content,
                                &mut intro_modes,
                                &mut players,
                                chain,
                                &mut out,
                            )?;
                            deferred_sections.extend(Self::binding_sections(out.records)?);
                            if !out.deleted.is_empty() {
                                deferred_sections.push(Section {
                                    float_bits: None,
                                    flag: false,
                                    setup: None,
                                    records: vec![],
                                    deleted: out.deleted,
                                });
                            }
                            responses.extend(out.calls);
                            responses.extend(Self::garage_presence_leave(
                                self.roles.as_ref(),
                                &players,
                                &mut garage_presence,
                                owner.participant,
                                self.persona,
                            )?);
                            Ok::<_, replication::Error>(())
                        })();
                        if let Err(error) = transition {
                            return self.refuse_rpc(error, body);
                        }
                    }
                    deferred_sections.push(Section {
                        float_bits: None,
                        flag: false,
                        setup: None,
                        records: vec![],
                        deleted: vec![ghost],
                    });
                    continue;
                }
                Ok(Some(None)) => continue,
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            match Self::opening_completed(
                &mut opening,
                &mut players,
                &mut participants,
                self.logic.as_deref(),
                &mut logic_ghosts,
                self.persona,
                body,
            ) {
                Ok(Some(Some((section, calls)))) => {
                    deferred_sections.push(section);
                    responses.extend(calls);
                    continue;
                }
                Ok(Some(None)) => continue,
                Ok(None) => {}
                Err(error) => return self.refuse_rpc(error, body),
            }
            if let Some(content) = self.prologue.as_deref() {
                match Self::driving_input(
                    content,
                    self.vehicles.as_deref(),
                    self.inventory.as_ref(),
                    &mut intro_modes,
                    &mut players,
                    self.event_seconds(self.world_ms),
                    body,
                ) {
                    Ok(Some(out)) => {
                        if !out.departures.is_empty() {
                            match prologue::retirement(out) {
                                Ok(retired) => exit_entries.extend(retired),
                                Err(error) => return self.refuse(error),
                            }
                            continue;
                        }
                        // Records need a setup (E1180: a completion without one
                        // closed the world). Deletions travel with the first
                        // records, as in the official completion frame.
                        match Self::driving_sections(out.records, out.deleted) {
                            Ok(sections) => deferred_sections.extend(sections),
                            Err(error) => return self.refuse_rpc(error, body),
                        }
                        responses.extend(out.calls);
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => return self.refuse_rpc(error, body),
                }
            }
            if let Some(content) = self.prologue.as_deref() {
                match Self::intro_garage_input(
                    content,
                    &mut intro_modes,
                    &mut players,
                    &mut participants,
                    returns.as_mut(),
                    population.as_mut(),
                    sequences.as_mut(),
                    self.inventory.as_ref().zip(self.vehicles.as_deref()).map(
                        |(inventory, content)| prologue::garage::OwnedGarage { inventory, content },
                    ),
                    self.logic.as_deref(),
                    &mut logic_ghosts,
                    self.persona,
                    body,
                ) {
                    Ok(Some(mut out)) => {
                        if let Some((participant, removed)) = out.removed {
                            if world_cars.remove(&removed).is_none() {
                                return self.refuse_rpc(replication::Error::UnknownObject, body);
                            }
                            // The intro keeps its spawn assignment through the
                            // return cinematic; the garage entry clears it
                            // (career start 298117 and 1127014 ms).
                            if spawn_points.assigned(participant).is_some() {
                                match spawn_points.clear(participant) {
                                    Ok(Some((notification, flag))) => {
                                        out.output.calls.push(
                                            nfs_world::participants::HostRpc::Participant(
                                                notification,
                                            ),
                                        );
                                        out.output.calls.extend(
                                            flag.map(
                                                nfs_world::participants::HostRpc::SpawnOccupied,
                                            ),
                                        );
                                    }
                                    Ok(None) => {}
                                    Err(error) => return self.refuse_rpc(error, body),
                                }
                            }
                            if let Some(crashes) = &mut crashes {
                                crashes.discard_pending(participant);
                                if let Err(error) = crashes.retire(participant) {
                                    return self.refuse_rpc(error, body);
                                }
                            }
                            completed_returns.push((participant, removed));
                            let presence = (|| {
                                let endpoint =
                                    Self::presence_endpoint(self.roles.as_ref(), &players)?;
                                garage_presence.set(endpoint, participant, true, |id| {
                                    players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
                                })
                            })();
                            match presence {
                                Ok(Some(value)) => {
                                    prologue::garage::insert_presence(&mut out.output.calls, value)
                                }
                                Ok(None) => {}
                                Err(error) => return self.refuse_rpc(error, body),
                            }
                        }
                        if !out.output.records.is_empty()
                            || !out.output.deleted.is_empty()
                            || !out.output.calls.is_empty()
                        {
                            // Records split to frame size with the deletions on
                            // the first section (E1186: one packed entry with
                            // installed content refused every resent completion).
                            match Self::garage_entries(
                                out.output.records,
                                out.output.deleted,
                                out.output.calls,
                            ) {
                                Ok(entries) => exit_entries.extend(entries),
                                Err(error) => return self.refuse_rpc(error, body),
                            }
                        }
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => return self.refuse_rpc(error, body),
                }
            }
            if let Some(crashes) = &mut crashes {
                match Self::finish_crash_call(
                    body,
                    crashes,
                    &mut players,
                    &mut participants,
                    self.logic.as_deref(),
                    &mut logic_ghosts,
                    self.persona,
                    self.vehicles.as_deref().map(|c| &c.layout),
                    &self.event_state,
                ) {
                    Ok(Some(Some((section, events, teleport)))) => {
                        // Recovery events name the replacement driving objects.
                        // Entry calls run before their section, so publish those
                        // creations first and put the events in a following entry.
                        exit_entries.push((section, Vec::new()));
                        if !events.is_empty() {
                            exit_entries.push((returning::empty_section(), events));
                        }
                        if let Some(command) = teleport {
                            scheduled_teleports.push((self.world_ms, command));
                        }
                        continue;
                    }
                    Ok(Some(None)) => continue,
                    Ok(None) => {}
                    Err(error) => return self.refuse_rpc(error, body),
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
                        if let Some(crashes) = &mut crashes {
                            crashes.discard_pending(participant);
                            if let Err(error) = crashes.retire(participant) {
                                return self.refuse_rpc(error, body);
                            }
                        }
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
                    // The intro's first zone report comes from its return
                    // cinematic, not a garage-exit spawn: the official host
                    // only publishes occupancy there (E1183, 290318 ms).
                    if !report.release
                        && report.current
                        && participants.intro_chapter(report.participant).is_none()
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
                                let Some(returns) = returns.as_mut() else {
                                    return self.refuse_rpc(replication::Error::Unsupported, body);
                                };
                                if population.is_none()
                                    || self.inventory.is_none()
                                    || self.vehicles.is_none()
                                {
                                    return self.refuse_rpc(replication::Error::Unsupported, body);
                                }
                                // Garage slot refreshes follow the staged car
                                // (E921: official slot calls 1.8 s after the
                                // request); the other replies ride the teardown.
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
                                    self.logic.as_deref(),
                                    &logic_ghosts,
                                    self.persona,
                                    *participant,
                                ) {
                                    Ok(entries) => exit_entries.extend(entries),
                                    Err(error) => {
                                        return self.refuse_rpc(error, body);
                                    }
                                }
                                scheduled_returns.push(returning::PendingReturn {
                                    participant: *participant,
                                    requested_ms: self.world_ms,
                                    torn_down: false,
                                    chain,
                                    slots,
                                });
                                return_trace = true;
                                tracing::info!(
                                    participant,
                                    teardown_ms = returning::RETURN_TEARDOWN_DELAY_MS,
                                    staging_ms = returning::RETURN_STAGING_DELAY_MS,
                                    "garage return requested; presentation event queued, teardown and staging scheduled"
                                );
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
                                        || !entry_admitted(&section[0], &entry_replies)
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
            if !entry_admitted(section, replies) {
                return self.refuse(replication::Error::Bound);
            }
        }
        entries.extend(exit_entries);
        if let Some(loading) = self
            .prologue
            .as_ref()
            .and_then(|content| content.loading.as_ref())
        {
            match loading.sync(
                &mut players,
                &participants,
                HOST_SELECTOR as u8,
                self.persona,
            ) {
                Ok(batch) => records.extend(batch),
                Err(error) => return self.refuse(error),
            }
        }
        let map_cars = world_cars
            .iter()
            .map(|(&car, &(participant, _))| (participant, car))
            .collect();
        if let Some(location) = self
            .roles
            .as_ref()
            .and_then(|roles| roles.free_roam.as_ref())
            .map(|catalog| catalog.location)
        {
            let mut batch =
                self.sync_optional_maps(&mut players, &intro_modes, &map_cars, location);
            // Keep movement and map records together when they fit, without
            // letting optional map failures poison a valid movement frame.
            if let Some(section) = &mut movement_section {
                let original_len = section.records.len();
                section.records.append(&mut batch);
                if !entry_admitted(section, &[]) {
                    batch = section.records.split_off(original_len);
                }
            }
            records.extend(batch);
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
        // These snapshots all originate in the same accepted client frame.
        // Packing them here prevents one frame per delegated car from filling
        // the listener queue before the session can coalesce them. The helper
        // preserves non-motion records as ordering barriers.
        if let (Some(content), Some(progression), Some(bindings)) = (
            self.prologue.as_deref(),
            self.progression.as_ref(),
            participants.return_bindings(),
        ) {
            for call in responses
                .iter()
                .chain(entries.iter().flat_map(|(_, calls)| calls))
            {
                if let nfs_world::participants::HostRpc::Participant(n) = call
                    && n.endpoint == bindings.preparing[0]
                    && n.call == nfs_world::participants::Call::Enter
                    && participants.intro_chapter(n.participant)
                        == Some(nfs_world::participants::intro::Chapter::FirstDrive)
                {
                    if story_pending.len() >= MAX_PENDING {
                        return self.refuse(replication::Error::Bound);
                    }
                    let Some(player_index) =
                        players.runtime_index(HOST_SELECTOR as u8, self.persona, n.participant)
                    else {
                        return self.refuse(replication::Error::UnknownObject);
                    };
                    story_pending.push_back(super::items::story::Request {
                        participant: n.participant,
                        player_index: u32::from(player_index),
                        progression: progression.clone(),
                        action: super::items::story::Action::Complete {
                            objective: content.start.objective,
                        },
                    });
                }
            }
        }
        // Presentation events accompany the intro state notifications, so add
        // them before the queue bounds are checked.
        {
            let ctx = prologue::presentation::Context {
                logic: self.logic.as_deref(),
                ghosts: &logic_ghosts,
                players: &players,
                participants: &participants,
                content: self.prologue.as_deref(),
            };
            responses = match prologue::presentation::present(&ctx, responses) {
                Ok(calls) => calls,
                Err(error) => return self.refuse(error),
            };
            for (_, calls) in entries.iter_mut() {
                *calls = match prologue::presentation::present(&ctx, std::mem::take(calls)) {
                    Ok(calls) => calls,
                    Err(error) => return self.refuse(error),
                };
            }
            // The added events can take an entry past a reply group.
            let mut admitted = Vec::with_capacity(entries.len());
            for (section, calls) in entries {
                match admitted_entries(section, calls) {
                    Ok(split) => admitted.extend(split),
                    Err(error) => return self.refuse(error),
                }
            }
            entries = admitted;
        }
        let scene_membership = match self.stage_scene_membership(&mut players, &mut participants) {
            Ok(value) => value,
            Err(error) => return self.refuse(error),
        };
        match prologue::movies::sections(scene_membership.records, scene_membership.deleted) {
            Ok(batch) => sections.extend(batch),
            Err(error) => return self.refuse(error),
        }
        deferred_sections = output::pack_motion(deferred_sections);
        if spawned.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || scheduled_returns.len() > MAX_PENDING.saturating_sub(self.pending_returns.len())
            || scheduled_teleports.len() > MAX_PENDING.saturating_sub(self.pending_teleports.len())
            || responses.len() > nfs_world::session::MAX_RPC_QUEUE - self.pending_rpcs.len()
            || sections.len() + deferred_sections.len() + usize::from(!records.is_empty())
                > MAX_PENDING - self.pending.len()
            || entries.len() > MAX_PENDING - self.pending_entries.len()
            || scheduled_binding_refreshes.len()
                > MAX_PENDING.saturating_sub(self.pending_binding_refreshes.len())
        {
            return self.refuse(replication::Error::Bound);
        }
        // The world host closes the session on a section or call it cannot
        // send (E1180), so check them here, before anything is committed.
        if let Some(error) = sections
            .iter()
            .chain(&deferred_sections)
            .chain(entries.iter().map(|(section, _)| section))
            .find_map(|section| section.encode().err())
            .or_else(|| {
                responses
                    .iter()
                    .chain(entries.iter().flat_map(|(_, calls)| calls))
                    .find_map(|call| call.encode().err())
            })
        {
            return self.refuse(error);
        }
        for participant in participants.in_free_roam() {
            if self.participants.stage(participant)
                == Some(nfs_world::participants::Stage::Crashing)
            {
                tracing::info!(
                    participant,
                    "crash recovery completed; driving context restored"
                );
            } else if !matches!(
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
        self.scene_membership = scene_membership.session;
        self.participants = participants;
        self.opening = opening;
        self.selection = selection;
        self.selection_execution = selection_execution;
        self.story_pending = story_pending;
        self.intro_modes = intro_modes;
        self.intro_movies = intro_movies;
        self.intro_composites = intro_composites;
        self.pending_rpcs.extend(responses);
        for input in event_inputs {
            if self.event_state.inputs.len() >= events::MAX_INPUTS {
                tracing::warn!("owned event input queue full; input dropped");
                break;
            }
            self.event_state.inputs.push_back(input);
        }
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
        self.traffic_reports = traffic_reports;
        self.world_activation = world_activation;
        self.deferred_deletions = deferred_deletions;
        self.pending_spawns.extend(scheduled_spawns);
        self.held_glass = held_glass;
        self.spawn_settled = spawn_settled;
        self.pending_settled.extend(scheduled_settled);
        self.arrivals = arrivals;
        self.returns = returns;
        self.crashes = crashes;
        if let Some(crashes) = &self.crashes {
            self.pending_rpcs.retain(|rpc| !matches!(
                rpc,
                nfs_world::participants::HostRpc::SequenceStop(stop) if crashes.completed(stop.sequence)
            ));
        }
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
            self.pending_returns
                .retain(|r| r.participant != participant);
            tracing::info!(
                participant,
                world_car = removed,
                "garage return completed; garage loading begins"
            );
        }
        self.pending_teleports.extend(scheduled_teleports);
        self.pending_binding_refreshes
            .extend(scheduled_binding_refreshes);
        self.pending_returns.extend(scheduled_returns);
        if return_trace {
            self.return_trace_until = self.world_ms.saturating_add(returning::RETURN_TRACE_MS);
        }
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
        self.garage_commands = garage_commands;
        self.logic_ghosts = logic_ghosts;
        self.logic_vehicles_fired = logic_vehicles_fired;
        self.pending_content.extend(spawned.messages);
        self.unsupported_rpcs += unsupported;
        self.unsupported_logic += unsupported_logic;
        self.scoring_state.reported.extend(scoring_reported);
        self.scoring_state.reports.extend(scoring_reports);
        self.scoring_state.nitrous = nitrous;
        self.scoring_state.reported.extend(nitrous_reported);
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
        self.queue_entries(entries);
        self.messages = messages;
        self.client_info = client_info;
        if let Some(profile) = performance_profile {
            self.performance_profile = Some(profile);
        }
        if let Some(next) = challenges {
            self.challenges = next;
        }
        if let Some(next) = recommendations {
            self.recommendations = next;
        }
        self.stats.record(&parsed, files)
    }
}
