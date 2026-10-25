// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    arrival::Arrivals,
    participants::Lifecycle,
    sequences::{Completed, Owner, Sequences},
};

impl PlayerListener {
    pub(in crate::net) fn advance_sequences(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        if world_ms < self.world_ms {
            return Err(replication::Error::Shape);
        }
        self.advance_binding_refreshes(world_ms)
            .inspect_err(|error| {
                tracing::warn!(?error, world_ms, "binding refresh update rejected");
            })?;
        self.advance_prologue().inspect_err(|error| {
            tracing::warn!(?error, world_ms, "prologue update rejected");
        })?;
        self.advance_crashes(world_ms).inspect_err(|error| {
            tracing::warn!(?error, world_ms, "crash recovery update rejected");
        })?;
        self.advance_world_activation().inspect_err(|error| {
            tracing::warn!(?error, world_ms, "world activation update rejected");
        })?;
        self.advance_returns(world_ms);
        if let Some(returns) = &mut self.returns {
            let capacity =
                nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len());
            for stop in returns.poll_capacity(world_ms, capacity)? {
                tracing::info!(
                    sequence = stop.sequence,
                    "garage entry cinematic finished; waiting for client completion"
                );
                self.pending_rpcs
                    .push_back(nfs_world::participants::HostRpc::SequenceStop(stop));
            }
        }
        if let Some(arrivals) = self.arrivals.as_mut() {
            for stop in arrivals.poll(world_ms)? {
                tracing::info!(
                    sequence = stop.sequence,
                    world_ms,
                    "owned arrive sequence not reported; Stop queued after the fallback"
                );
                self.pending_rpcs
                    .push_back(nfs_world::participants::HostRpc::SequenceStop(stop));
            }
        }
        let Some(mut sequences) = self.sequences.clone() else {
            self.world_ms = world_ms;
            return Ok(());
        };
        // Readiness observed this tick starts its delay on this tick's clock.
        sequences.poll(world_ms, 0)?;
        Self::refresh_readiness(
            self.roles.as_ref(),
            &mut sequences,
            &self.players,
            &self.participants,
            self.population.as_ref(),
            self.persona,
        )?;
        let capacity = nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len());
        let stops = sequences.poll(world_ms, capacity)?;
        for stop in &stops {
            tracing::info!(
                sequence = stop.sequence,
                world_ms,
                "owned waiting sequence Stop queued"
            );
        }
        self.pending_rpcs.extend(
            stops
                .into_iter()
                .map(nfs_world::participants::HostRpc::SequenceStop),
        );
        self.sequences = Some(sequences);
        self.world_ms = world_ms;
        Ok(())
    }
    /// Queue the level root poll when due, once a participant has begun
    /// (its scenes then exist on the client). Official cadence ~1.1 s.
    pub(in crate::net) fn advance_level_poll(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        if !self.participants.has_participants()
            || self.pending_rpcs.len() >= nfs_world::session::MAX_RPC_QUEUE
        {
            return Ok(());
        }
        let mut level_poll = self.level_poll.clone();
        let poll = level_poll.due(world_ms)?;
        let polled = poll.is_some();
        let mut replies = Vec::new();
        if let Some(poll) = poll {
            replies.push(nfs_world::participants::HostRpc::LevelPoll(poll));
        }
        // Official spawn assignments share a frame with a level poll (E755).
        if polled || level_poll.endpoint().is_none() {
            for &(_, notification) in self
                .pending_spawns
                .iter()
                .take_while(|(due, _)| *due <= world_ms)
            {
                // The spawn presentation follows the client's zone report, not
                // the assignment; see the spawn report handling.
                if replies.len() + 1
                    > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
                {
                    // Keep the due poll and assignment together for the retry.
                    return Ok(());
                }
                tracing::info!(
                    participant = notification.participant,
                    call = ?notification.call,
                    "owned spawn point assigned"
                );
                replies.push(nfs_world::participants::HostRpc::Participant(notification));
                self.spawn_released
                    .insert(notification.participant, world_ms);
            }
        }
        self.level_poll = level_poll;
        if polled || self.level_poll.endpoint().is_none() {
            let _ = Self::release_spawns(&mut self.pending_spawns, world_ms);
        }
        self.pending_rpcs.extend(replies);
        Ok(())
    }
    /// Release the glass replies held for a world car one host frame after
    /// its participant's first spawn-zone report. Queue space is
    /// checked; a full queue retries on the next tick.
    pub(in crate::net) fn advance_settled(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        while let Some(&(due, participant)) = self.pending_settled.front() {
            if due > world_ms {
                break;
            }
            let held = self.held_glass.get(&participant).map_or(0, Vec::len);
            let events = Self::presentation_events(
                self.logic.as_deref(),
                crate::garage_logic::Which::WorldSettled,
                &self.players,
                &self.logic_ghosts,
                self.persona,
                participant,
            )?;
            if self.pending_rpcs.len() + held + events.len() > nfs_world::session::MAX_RPC_QUEUE {
                return Ok(());
            }
            self.pending_settled.pop_front();
            self.pending_rpcs.extend(events);
            if let Some(fires) = self.held_glass.remove(&participant) {
                tracing::info!(
                    participant,
                    replies = fires.len(),
                    "owned world car glass replies released"
                );
                self.pending_rpcs.extend(
                    fires
                        .into_iter()
                        .map(nfs_world::participants::HostRpc::Event),
                );
            }
        }
        Ok(())
    }
    pub(super) fn refresh_readiness(
        roles: Option<&crate::scene_roles::SceneRoles>,
        sequences: &mut Sequences,
        players: &Players,
        participants: &Lifecycle,
        population: Option<&Population>,
        persona: u64,
    ) -> Result<(), replication::Error> {
        let Some(roles) = roles else {
            return Ok(());
        };
        for participant in players.player_participants() {
            let owner = Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant,
            };
            if !players.owns_participant(owner.connection, owner.persona, participant) {
                continue;
            }
            let asset = roles.streaming_gate;
            let mut candidates = players.objects().iter().filter_map(|(ghost, object)| {
                matches!(&object.initial,
                    replication::Initial::Entity { prefix, fields } if prefix.asset == asset
                    && matches!(fields.first(), Some(replication::entity::Initial::Root {reference,..}) if *reference == participant)
                ).then_some(ghost)
            });
            if let Some(ghost) = candidates.next() {
                if candidates.next().is_some() {
                    return Err(replication::Error::DuplicateObject);
                }
                sequences.bind_streaming(players, owner, ghost, asset)?;
            }
            if sequences.ghost(participant).is_some() {
                let loaded = population.is_some_and(|p| match participants.stage(participant) {
                    Some(nfs_world::participants::Stage::IntroLoadingGarage) => {
                        if participants.intro_chapter(participant)
                            == Some(nfs_world::participants::intro::Chapter::ReturnDrive)
                        {
                            p.all_loaded(participant)
                        } else {
                            p.previews_loaded(participant)
                        }
                    }
                    Some(nfs_world::participants::Stage::LoadingGarage) => {
                        p.all_loaded(participant)
                    }
                    _ => false,
                });
                sequences.garage_loaded(owner, loaded)?;
            }
        }
        Ok(())
    }
    pub(super) fn start_sequences(
        players: &mut Players,
        participants: &Lifecycle,
        sequences: Option<&mut Sequences>,
        persona: u64,
    ) -> Result<Vec<Section>, replication::Error> {
        let Some(sequences) = sequences else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();
        for participant in participants.waiting_garage() {
            if let Some(record) = sequences.start(
                players,
                Owner {
                    connection: HOST_SELECTOR as u8,
                    persona,
                    participant,
                },
            )? {
                records.push(record);
            }
        }
        if records.is_empty() {
            return Ok(Vec::new());
        }
        split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS)
    }
    /// Start the arrive sequence of participants that just entered the arrive
    /// state; it goes out with their state calls and the arrive garage logic
    /// batch (official order). Without configured arrive content they go
    /// straight on to free roam, and both arrive batches apply at once.
    pub(super) fn start_arrivals(
        players: &mut Players,
        participants: &mut Lifecycle,
        arrivals: Option<&mut Arrivals>,
        logic: Option<&crate::garage_logic::GarageLogic>,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        mut chain: Vec<nfs_world::participants::HostRpc>,
    ) -> Result<(Option<Section>, Vec<nfs_world::participants::HostRpc>), replication::Error> {
        use crate::garage_logic::Which;
        let arriving = participants.take_arriving();
        if arriving.is_empty() {
            return Ok((None, chain));
        }
        let mut records = Vec::new();
        let mut deleted = Vec::new();
        let mut apply = |players: &mut Players,
                         participant: u16,
                         which: Which,
                         chain: &mut Vec<nfs_world::participants::HostRpc>|
         -> Result<Vec<replication::Record>, replication::Error> {
            let out = Self::arrive_logic(logic, which, players, participant, logic_ghosts)?;
            deleted.extend(out.deleted);
            chain.extend(
                out.events
                    .into_iter()
                    .map(nfs_world::participants::HostRpc::Event),
            );
            Ok(out.records)
        };
        match arrivals {
            None => {
                for participant in arriving {
                    records.extend(players.actor_arriving(
                        HOST_SELECTOR as u8,
                        persona,
                        participant,
                    )?);
                    if let nfs_world::participants::Outcome::Advanced(calls) =
                        participants.arrived(participant)
                    {
                        if participants.prompted(participant) {
                            tracing::info!(
                                participant,
                                "garage prompt carried into free roam; the return waits for the confirming request"
                            );
                        }
                        chain.extend(
                            calls
                                .into_iter()
                                .map(nfs_world::participants::HostRpc::Participant),
                        );
                    }
                    for which in [Which::Arrive, Which::Arrived] {
                        records.extend(apply(players, participant, which, &mut chain)?);
                    }
                }
            }
            Some(arrivals) => {
                for participant in arriving {
                    let started = arrivals.start_rollout(
                        players,
                        Owner {
                            connection: HOST_SELECTOR as u8,
                            persona,
                            participant,
                        },
                    )?;
                    let logic = if started.is_empty() {
                        Vec::new()
                    } else {
                        apply(players, participant, Which::Arrive, &mut chain)?
                    };
                    records.extend(started);
                    records.extend(logic);
                    records.extend(players.actor_arriving(
                        HOST_SELECTOR as u8,
                        persona,
                        participant,
                    )?);
                }
            }
        }
        Ok((arrival_section(records, deleted)?, chain))
    }
    /// The garage logic batch for an arrive point (empty when the deployment
    /// describes none).
    pub(super) fn arrive_logic(
        logic: Option<&crate::garage_logic::GarageLogic>,
        which: crate::garage_logic::Which,
        players: &mut Players,
        participant: u16,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
    ) -> Result<crate::garage_logic::Output, replication::Error> {
        Self::arrive_logic_part(
            logic,
            which,
            players,
            participant,
            logic_ghosts,
            crate::garage_logic::Part::All,
        )
    }
    /// One part of an arrive point's batch (the garage return is staged).
    pub(super) fn arrive_logic_part(
        logic: Option<&crate::garage_logic::GarageLogic>,
        which: crate::garage_logic::Which,
        players: &mut Players,
        participant: u16,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        part: crate::garage_logic::Part,
    ) -> Result<crate::garage_logic::Output, replication::Error> {
        match logic.filter(|l| l.has(which)) {
            None => Ok(crate::garage_logic::Output::default()),
            Some(logic) => logic.produce_part(
                which,
                players,
                participant,
                logic_ghosts.entry(participant).or_default(),
                part,
            ),
        }
    }
    /// Client calls on an arrive sequence: `Some(None)` for its
    /// acknowledgements and completion repeats, `Some(Some((sequence,
    /// participant, chain)))` when it completes (delete the sequence, leave
    /// the arrive state for free roam), `None` for anything else.
    #[allow(clippy::type_complexity)]
    pub(super) fn arrival_call(
        arrivals: &mut Arrivals,
        players: &Players,
        participants: &mut Lifecycle,
        persona: u64,
        body: BitSpan<'_>,
    ) -> Result<Option<Option<(u16, u16, Vec<nfs_world::participants::HostRpc>)>>, replication::Error>
    {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| replication::Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| replication::Error::Shape)?;
        let Some(&ghost) = envelope.references().first() else {
            return Ok(None);
        };
        if arrivals.acknowledges(ghost, route.selector(), route.method_index()) {
            return Ok(Some(None));
        }
        if !arrivals.has_completion(ghost, route.selector()) || route.method_index() != 1 {
            return Ok(None);
        }
        let call = Completed::decode(body)?;
        let Some(sequence) = arrivals.complete(
            players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant: call.participant,
            },
            call,
        )?
        else {
            return Ok(Some(None));
        };
        let nfs_world::participants::Outcome::Advanced(calls) =
            participants.arrived(call.participant)
        else {
            return Err(replication::Error::Unsupported);
        };
        if participants.prompted(call.participant) {
            tracing::info!(
                participant = call.participant,
                "garage prompt carried into free roam; the return waits for the confirming request"
            );
        }
        Ok(Some(Some((
            sequence,
            call.participant,
            calls
                .into_iter()
                .map(nfs_world::participants::HostRpc::Participant)
                .collect(),
        ))))
    }
    pub(super) fn sequence_completed(
        sequences: &mut Sequences,
        players: &Players,
        persona: u64,
        body: BitSpan<'_>,
    ) -> Result<bool, replication::Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| replication::Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| replication::Error::Shape)?;
        let Some(&ghost) = envelope.references().first() else {
            return Ok(false);
        };
        if !sequences.has_endpoint(ghost, route.selector()) || route.method_index() != 1 {
            return Ok(false);
        }
        let call = Completed::decode(body)?;
        sequences.complete(
            players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant: call.participant,
            },
            call,
        )?;
        Ok(true)
    }
}

/// One replication section for the arrive frame: the sequence creations plus
/// the deletions of the arrive garage logic batch, or `None` when empty.
pub(super) fn arrival_section(
    records: Vec<replication::Record>,
    deleted: Vec<u16>,
) -> Result<Option<Section>, replication::Error> {
    let mut sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS)?;
    if sections.len() > 1 {
        return Err(replication::Error::Bound);
    }
    Ok(match (sections.pop(), deleted.is_empty()) {
        (section, true) => section,
        (Some(mut section), false) => {
            section.deleted.extend(deleted);
            Some(section)
        }
        (None, false) => Some(Section {
            float_bits: None,
            flag: false,
            deleted,
            setup: None,
            records: Vec::new(),
        }),
    })
}
