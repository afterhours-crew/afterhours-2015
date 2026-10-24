// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::sequence_content::authored::Instances;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    participants::{HostRpc, Lifecycle, Outcome, Stage},
    sequences::{Completed, Owner},
    teleport::Command,
};
type Ready = (Vec<replication::Record>, Vec<HostRpc>, Command);
type Finished = (Section, Vec<HostRpc>);
#[cfg(test)]
mod tests;

impl PlayerListener {
    pub(super) fn advance_prologue(&mut self) -> Result<(), replication::Error> {
        let Some(content) = self.prologue.as_deref() else {
            return Ok(());
        };
        let Some(vehicles) = self.vehicles.as_deref() else {
            return Err(replication::Error::Unsupported);
        };
        if self.session_backlog >= MAX_PENDING {
            return Ok(());
        }
        let mut players = self.players.clone();
        let mut population = self
            .population
            .clone()
            .ok_or(replication::Error::Unsupported)?;
        let mut opening = self.opening.clone();
        let mut cars = self.world_cars.clone();
        let mut records = Vec::new();
        let mut messages = Vec::new();
        let owners = players.player_participants().collect::<Vec<_>>();
        for participant in owners {
            let owner = Owner {
                connection: HOST_SELECTOR as u8,
                persona: self.persona,
                participant,
            };
            match self.participants.stage(participant) {
                Some(Stage::IntroSpawning)
                    if players
                        .temporary_vehicle(owner.connection, owner.persona, participant)
                        .is_none() =>
                {
                    if cars.len() >= MAX_WORLD_CARS {
                        return Err(replication::Error::Bound);
                    }
                    let spawned = population.spawn_temporary_world(
                        &mut players,
                        crate::vehicle_content::population::Owner {
                            connection: owner.connection,
                            persona: owner.persona,
                            participant,
                        },
                        content.temporary_vehicle(vehicles)?,
                        &vehicles.vehicles,
                    )?;
                    let ghost = players
                        .temporary_vehicle(owner.connection, owner.persona, participant)
                        .ok_or(replication::Error::UnknownObject)?;
                    cars.insert(ghost, (participant, false));
                    records.extend(spawned.records);
                    messages.extend(spawned.messages);
                }
                Some(Stage::IntroOpening) => {
                    if let Some(record) = opening.start(&content.opening, &mut players, owner)? {
                        records.push(record);
                    }
                }
                _ => {}
            }
            // One owner per tick keeps a busy multi-player session within the
            // same bounded content/replication batch as a single-player start.
            if !records.is_empty() || !messages.is_empty() {
                break;
            }
        }
        if records.is_empty() && messages.is_empty() {
            return Ok(());
        }
        // A staged car is constructed with an absolute zero origin. Reserve
        // space for that origin rather than inheriting a prior movement frame.
        let mut sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS - 96)?;
        for section in &mut sections {
            section.setup = Some(Setup::RawEscape([0; 3]));
        }
        if sections.len() > MAX_PENDING.saturating_sub(self.pending.len())
            || messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
        {
            return Ok(());
        }
        self.players = players;
        self.population = Some(population);
        self.opening = opening;
        self.world_cars = cars;
        self.pending.extend(sections);
        self.pending_content.extend(messages);
        Ok(())
    }
    pub(super) fn intro_car_ready(
        content: Option<&crate::prologue::Content>,
        players: &mut Players,
        participants: &mut Lifecycle,
        persona: u64,
        participant: u16,
        vehicle: u16,
    ) -> Result<Option<Ready>, replication::Error> {
        if participants.stage(participant) != Some(Stage::IntroSpawning) {
            return Ok(None);
        }
        let content = content.ok_or(replication::Error::Unsupported)?;
        if players.temporary_vehicle(HOST_SELECTOR as u8, persona, participant) != Some(vehicle) {
            return Err(replication::Error::UnknownObject);
        }
        let mut records = players.bind_player_vehicle(
            HOST_SELECTOR as u8,
            persona,
            participant,
            Some(vehicle),
        )?;
        records.extend(players.actor_vehicle_ready(HOST_SELECTOR as u8, persona, participant)?);
        let command = Command {
            endpoint: participants
                .intro_bindings()
                .ok_or(replication::Error::UnknownObject)?
                .teleport,
            participant,
            vehicle,
            position: content.start.teleport.position,
            basis: content.start.teleport.basis,
            speed: 0.,
        };
        command.encode()?;
        let Outcome::Advanced(calls) = participants.intro_vehicle_ready(participant) else {
            return Err(replication::Error::Shape);
        };
        Ok(Some((
            records,
            calls.into_iter().map(HostRpc::Participant).collect(),
            command,
        )))
    }
    pub(super) fn opening_signal(
        content: &crate::prologue::Content,
        opening: &mut Instances,
        players: &Players,
        persona: u64,
        message: &nfs_world::logic::Message,
        responses: &mut Vec<HostRpc>,
    ) -> Result<bool, replication::Error> {
        let nfs_world::logic::Message::Reached { target, .. } = message else {
            return Ok(false);
        };
        let Some(owner) = opening.owner(target.ghost) else {
            return Ok(false);
        };
        if owner.connection != HOST_SELECTOR as u8 || owner.persona != persona {
            return Err(replication::Error::UnknownObject);
        }
        if let Some(stop) = opening.signal(&content.opening, players, owner, message)? {
            responses.push(HostRpc::SequenceStop(stop));
        }
        Ok(true)
    }
    pub(super) fn opening_completed(
        opening: &mut Instances,
        players: &mut Players,
        participants: &mut Lifecycle,
        persona: u64,
        body: BitSpan<'_>,
    ) -> Result<Option<Option<Finished>>, replication::Error> {
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
        if !opening.has_endpoint(ghost, route.selector()) || route.method_index() != 1 {
            return Ok(None);
        }
        let call = Completed::decode(body)?;
        if (0..route.arguments().len()).any(|bit| route.arguments().read_u32(bit, 1) != Ok(0)) {
            return Err(replication::Error::Shape);
        }
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant: call.participant,
        };
        let Some(deleted) = opening.complete(players, owner, call)? else {
            return Ok(Some(None));
        };
        let Outcome::Advanced(calls) = participants.intro_opening_complete(owner.participant)
        else {
            return Err(replication::Error::Shape);
        };
        Ok(Some(Some((
            Section {
                float_bits: None,
                flag: false,
                deleted: vec![deleted],
                setup: None,
                records: vec![],
            },
            calls.into_iter().map(HostRpc::Participant).collect(),
        ))))
    }
}
