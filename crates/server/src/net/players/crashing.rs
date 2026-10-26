// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    crash::{Crashes, ResetBindings},
    participants::{Call, HostRpc, Lifecycle, Notification, Stage},
    replication::vehicle::movement::Sample,
    sequences::{Completed, Owner},
};
#[cfg(test)]
mod tests;

fn eligible(participants: &Lifecycle, events: &events::State, participant: u16) -> bool {
    participants.intro_chapter(participant).is_none()
        && matches!(
            participants.stage(participant),
            Some(Stage::FreeRoam | Stage::ReturnPrompted)
        )
        && events.crash_allowed(participant)
}

impl PlayerListener {
    pub(super) fn observe_crashes(
        crashes: &mut Crashes,
        players: &Players,
        participants: &Lifecycle,
        world_cars: &BTreeMap<u16, (u16, bool)>,
        persona: u64,
        movement: &frame::Movement<'_>,
        event: (
            &events::State,
            Option<&crate::event_logic::catalog::Catalog>,
        ),
    ) -> Result<(), replication::Error> {
        let header = movement.header.ok_or(replication::Error::Shape)?;
        for record in &movement.records {
            let Some(&(participant, ready)) = world_cars.get(&record.id) else {
                continue;
            };
            let connection = HOST_SELECTOR as u8;
            // The intro's temporary car has no durable inventory identity.
            // Its scripted driving also owns presentation/recovery, even when
            // the participant shares the FreeRoam movement state.
            if players.temporary_vehicle(connection, persona, participant) == Some(record.id)
                && !eligible(participants, event.0, participant)
            {
                continue;
            }
            if record.component != 0
                || !players.owns_participant_vehicle(connection, persona, participant, record.id)
            {
                return Err(replication::Error::UnknownObject);
            }
            let sample = Sample::decode(header, record.data)?;
            let route = event
                .0
                .recovery_key(participant, record.id)
                .zip(
                    event
                        .1
                        .and_then(|catalog| catalog.get(event.0.event_id(participant)?)),
                )
                .and_then(|(key, content)| {
                    Some((
                        key,
                        content.event().game_mode.recovery?,
                        content.navigation()?,
                    ))
                })
                .map(|(key, config, navigation)| {
                    let motion = sample.physics.decode([0.; 3])?;
                    let geometry = if ready
                        && eligible(participants, event.0, participant)
                        && !sample.physics.controls.flags[0]
                        && sample.physics.controls.values[0] != 1
                    {
                        navigation.recovery(
                            motion.position,
                            usize::from(key.next),
                            config.sharp_corner_degrees,
                        )?
                    } else {
                        None
                    };
                    Ok::<_, replication::Error>(nfs_world::crash::route::Sample {
                        key,
                        config,
                        geometry,
                    })
                })
                .transpose()?;
            crashes.observe_vehicle(
                players,
                Owner {
                    connection,
                    persona,
                    participant,
                },
                record.id,
                &sample,
                ready && eligible(participants, event.0, participant),
                route,
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
            if !eligible(&participants, &self.event_state, owner.participant) {
                crashes.discard_pending(owner.participant);
                continue;
            }
            // Event inputs from the same accepted frame can finish the race or
            // begin a presentation. Resolve those before admitting a new crash;
            // the next tick rechecks the committed event state.
            if !self.event_state.inputs.is_empty() {
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
            let mut events: Vec<_> = presentation
                .events
                .into_iter()
                .map(HostRpc::Event)
                .collect();
            if let Some(exit) = participants.exit_bindings() {
                let bindings = ResetBindings::from_scene(&players, exit.state_2.scene)?;
                events.push(HostRpc::Participant(Notification {
                    endpoint: bindings.cinematic,
                    participant: owner.participant,
                    call: Call::Enter,
                }));
            }
            entries.push((
                Section {
                    float_bits: None,
                    flag: false,
                    deleted: presentation.deleted,
                    setup: Some(Setup::RawEscape([0; 3])),
                    records,
                },
                events,
            ));
        }
        for stop in &stops {
            tracing::warn!(
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
        self.sequence_fallbacks[2] = self.sequence_fallbacks[2].saturating_add(stops.len());
        self.players = players;
        self.participants = participants;
        self.logic_ghosts = ghosts;
        self.queue_entries(entries);
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
        layout: Option<&crate::vehicle_content::layout::Layout>,
        event_state: &events::State,
    ) -> Result<
        Option<Option<(Section, Vec<HostRpc>, Option<nfs_world::teleport::Command>)>>,
        replication::Error,
    > {
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
        let bindings = participants
            .exit_bindings()
            .map(|exit| ResetBindings::from_scene(players, exit.state_2.scene))
            .transpose()?;
        if let Some(bindings) = bindings
            && ghost == bindings.teleport.scene
            && route.selector() == bindings.teleport.selector
            && route.method_index() == 3
        {
            let [_, participant] = envelope.references() else {
                return Err(replication::Error::Shape);
            };
            // This method has no arguments. The seven remaining payload bits
            // only align it to a byte; native writers do not have to clear them.
            if envelope.words() != [0, 0]
                || !envelope.remaining().is_empty()
                || route.arguments().len() != 7
            {
                return Err(replication::Error::Shape);
            }
            if !players.owns_participant(HOST_SELECTOR as u8, persona, *participant) {
                return Err(replication::Error::UnknownObject);
            }
            let owner = Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant: *participant,
            };
            if !crashes.relocating(*participant) {
                return Ok(Some(None));
            }
            if !crashes.finish_relocation(owner)? {
                return Ok(Some(None));
            }
            let arrived = Self::arrive_logic(
                logic,
                crate::garage_logic::Which::ResetArrived,
                players,
                owner.participant,
                ghosts,
            )?;
            let (mut section, events) =
                Self::restore_crash_driving(players, participants, logic, ghosts, owner, None)?;
            section.deleted.extend(arrived.deleted);
            section.records.extend(arrived.records);
            section.setup = (!section.records.is_empty()).then_some(Setup::RawEscape([0; 3]));
            let mut events: Vec<_> = arrived
                .events
                .into_iter()
                .map(HostRpc::Event)
                .chain(events)
                .collect();
            events.extend(
                bindings
                    .finish(*participant)
                    .into_iter()
                    .map(HostRpc::Participant),
            );
            return Ok(Some(Some((section, events, None))));
        }
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
        let event_recovery = crashes
            .vehicle(owner.participant)
            .and_then(|vehicle| event_state.recovery_key(owner.participant, vehicle))
            .and_then(|key| crashes.event_relocation(owner.participant, key));
        let relocation = event_recovery
            .map(|(vehicle, recovery)| (vehicle, Some(recovery.destination), recovery.speed))
            .or_else(|| {
                crashes
                    .relocation(owner.participant)
                    .map(|(vehicle, destination)| (vehicle, destination, 0.))
            });
        if let Some((vehicle, destination, speed)) = relocation {
            let bindings = bindings.ok_or(replication::Error::UnknownObject)?;
            let destination = destination
                .or_else(|| {
                    layout?
                        .exit_teleport()
                        .map(|d| nfs_world::crash::Destination {
                            position: d.locator(),
                            basis: d.basis(),
                        })
                })
                .ok_or(replication::Error::Shape)?;
            let command = nfs_world::teleport::Command {
                endpoint: bindings.teleport,
                participant: owner.participant,
                vehicle,
                position: destination.position,
                basis: destination.basis,
                speed,
            };
            command.encode()?;
            tracing::info!(participant = owner.participant, vehicle, speed, position = ?destination.position,
                forward = ?destination.basis[2], event = event_recovery.is_some(), "crash recovery teleport queued");
            crashes.begin_relocation(owner)?;
            let presentation = Self::arrive_logic(
                logic,
                crate::garage_logic::Which::Resetting,
                players,
                owner.participant,
                ghosts,
            )?;
            let mut deleted = presentation.deleted;
            deleted.push(sequence);
            let section = Section {
                float_bits: None,
                flag: false,
                deleted,
                setup: (!presentation.records.is_empty()).then_some(Setup::RawEscape([0; 3])),
                records: presentation.records,
            };
            let events = presentation
                .events
                .into_iter()
                .map(HostRpc::Event)
                .chain(
                    bindings
                        .begin(owner.participant)
                        .into_iter()
                        .map(HostRpc::Participant),
                )
                .collect();
            return Ok(Some(Some((section, events, Some(command)))));
        }
        let (section, mut events) = Self::restore_crash_driving(
            players,
            participants,
            logic,
            ghosts,
            owner,
            Some(sequence),
        )?;
        if let Some(bindings) = bindings {
            events.push(HostRpc::Participant(Notification {
                endpoint: bindings.cinematic,
                participant: owner.participant,
                call: Call::Leave,
            }));
        }
        Ok(Some(Some((section, events, None))))
    }

    fn restore_crash_driving(
        players: &mut Players,
        participants: &mut Lifecycle,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        owner: Owner,
        sequence: Option<u16>,
    ) -> Result<(Section, Vec<HostRpc>), replication::Error> {
        let presentation = Self::arrive_logic(
            logic,
            crate::garage_logic::Which::Recovered,
            players,
            owner.participant,
            ghosts,
        )?;
        participants.recovered(owner.participant)?;
        let mut deleted = presentation.deleted;
        deleted.extend(sequence);
        Ok((
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
        ))
    }
}
