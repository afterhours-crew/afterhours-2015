// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
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
        let Some(mut sequences) = self.sequences.clone() else {
            self.world_ms = world_ms;
            return Ok(());
        };
        Self::refresh_readiness(
            self.roles.as_ref(),
            &mut sequences,
            &self.players,
            &self.participants,
            self.population.as_ref(),
            &self.logic_ghosts,
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
        let poll = self.level_poll.due(world_ms)?;
        let polled = poll.is_some();
        if let Some(poll) = poll {
            self.pending_rpcs
                .push_back(nfs_world::participants::HostRpc::LevelPoll(poll));
        }
        // Official spawn assignments share a frame with a level poll (E755).
        if polled || self.level_poll.endpoint().is_none() {
            for notification in Self::release_spawns(&mut self.pending_spawns, world_ms) {
                tracing::info!(
                    participant = notification.participant,
                    call = ?notification.call,
                    "owned spawn point assigned"
                );
                self.pending_rpcs
                    .push_back(nfs_world::participants::HostRpc::Participant(notification));
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
        logic_ghosts: &BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
    ) -> Result<(), replication::Error> {
        for participant in participants.waiting_garage() {
            if sequences.ghost(participant).is_none() {
                continue;
            }
            let owner = Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant,
            };
            if let Some(ghosts) = logic_ghosts.get(&participant) {
                let asset = roles.ok_or(replication::Error::Unsupported)?.streaming_gate;
                let mut candidates = ghosts.values().copied().filter(|ghost| {
                    matches!(
                    players.objects().get(*ghost).map(|o| &o.initial),
                    Some(replication::Initial::Entity { prefix, .. }) if prefix.asset == asset)
                });
                if let Some(ghost) = candidates.next() {
                    if candidates.next().is_some() {
                        return Err(replication::Error::DuplicateObject);
                    }
                    sequences.bind_streaming(players, owner, ghost, asset)?;
                }
            }
            sequences
                .garage_loaded(owner, population.is_some_and(|p| p.all_loaded(participant)))?;
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
