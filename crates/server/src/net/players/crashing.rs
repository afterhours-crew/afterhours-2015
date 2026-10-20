// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    crash::Crashes,
    participants::{HostRpc, Lifecycle, Stage},
    replication::vehicle::movement::Sample,
    sequences::{Completed, Owner},
};
#[cfg(test)]
mod tests;

fn eligible(participants: &Lifecycle, participant: u16) -> bool {
    matches!(
        participants.stage(participant),
        Some(Stage::FreeRoam | Stage::ReturnPrompted)
    )
}

impl PlayerListener {
    pub(super) fn observe_crashes(
        crashes: &mut Crashes,
        players: &Players,
        participants: &Lifecycle,
        world_cars: &BTreeMap<u16, (u16, bool)>,
        persona: u64,
        movement: &frame::Movement<'_>,
    ) -> Result<(), replication::Error> {
        let header = movement.header.ok_or(replication::Error::Shape)?;
        for record in &movement.records {
            let Some(&(participant, ready)) = world_cars.get(&record.id) else {
                continue;
            };
            let connection = HOST_SELECTOR as u8;
            let item = players
                .vehicle_item_of(connection, persona, record.id)
                .ok_or(replication::Error::UnknownObject)?;
            if record.component != 0
                || players.owned_vehicle(connection, persona, participant, item) != Some(record.id)
            {
                return Err(replication::Error::UnknownObject);
            }
            let sample = Sample::decode(header, record.data)?;
            crashes.observe(
                players,
                Owner {
                    connection,
                    persona,
                    participant,
                },
                sample.physics.identity_words[2],
                sample.physics.controls.flags[0],
                ready && eligible(participants, participant),
            )?;
        }
        Ok(())
    }

    /// A latched crash waits for replication capacity. All lifecycle, presentation
    /// and sequence changes commit together once its output fits.
    pub(in crate::net) fn advance_crashes(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        let Some(mut crashes) = self.crashes.clone() else {
            return Ok(());
        };
        let capacity = nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len());
        let stops = crashes.poll(world_ms, capacity)?;
        let mut players = self.players.clone();
        let mut participants = self.participants.clone();
        let mut ghosts = self.logic_ghosts.clone();
        let mut entries = Vec::new();
        for owner in crashes.pending() {
            if !eligible(&participants, owner.participant) {
                crashes.discard_pending(owner.participant);
                continue;
            }
            if self.pending_entries.len() + self.session_backlog + entries.len() >= MAX_PENDING {
                break;
            }
            let presentation = Self::arrive_logic(
                self.logic.as_deref(),
                crate::garage_logic::Which::Crashed,
                &mut players,
                owner.participant,
                &mut ghosts,
            )?;
            let sequence = crashes
                .start(&mut players, owner)?
                .ok_or(replication::Error::Shape)?;
            participants.crashed(owner.participant)?;
            let mut records = presentation.records;
            records.push(sequence);
            entries.push((
                Section {
                    float_bits: None,
                    flag: false,
                    deleted: presentation.deleted,
                    setup: Some(Setup::RawEscape([0; 3])),
                    records,
                },
                presentation
                    .events
                    .into_iter()
                    .map(HostRpc::Event)
                    .collect(),
            ));
        }
        for stop in &stops {
            tracing::info!(
                sequence = stop.sequence,
                "crash sequence fallback Stop queued"
            );
        }
        for owner in crashes.pending() {
            // Still pending under queue pressure; no transition is reported yet.
            debug_assert!(!matches!(
                participants.stage(owner.participant),
                Some(Stage::Crashing)
            ));
        }
        for participant in self.participants.in_free_roam() {
            if participants.stage(participant) == Some(Stage::Crashing) {
                tracing::info!(participant, "crash recovery cinematic started");
            }
        }
        self.crashes = Some(crashes);
        self.players = players;
        self.participants = participants;
        self.logic_ghosts = ghosts;
        self.pending_entries.extend(entries);
        self.pending_rpcs
            .extend(stops.into_iter().map(HostRpc::SequenceStop));
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub(super) fn finish_crash_call(
        body: BitSpan<'_>,
        crashes: &mut Crashes,
        players: &mut Players,
        participants: &mut Lifecycle,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
    ) -> Result<Option<Option<(Section, Vec<HostRpc>)>>, replication::Error> {
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
        if !crashes.has_completion(ghost, route.selector()) || route.method_index() != 1 {
            return Ok(None);
        }
        let call = Completed::decode(body)?;
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant: call.participant,
        };
        let Some(sequence) = crashes.complete(players, owner, call)? else {
            return Ok(Some(None));
        };
        let presentation = Self::arrive_logic(
            logic,
            crate::garage_logic::Which::Recovered,
            players,
            owner.participant,
            ghosts,
        )?;
        participants.recovered(owner.participant)?;
        let mut deleted = presentation.deleted;
        deleted.push(sequence);
        Ok(Some(Some((
            Section {
                float_bits: None,
                flag: false,
                deleted,
                setup: (!presentation.records.is_empty()).then_some(Setup::RawEscape([0; 3])),
                records: presentation.records,
            },
            presentation
                .events
                .into_iter()
                .map(HostRpc::Event)
                .collect(),
        ))))
    }
}
