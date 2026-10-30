// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::prologue::driving::Output;
use nfs_world::{arrival::Arrivals, participants::Notification, sequences::Sequences};

#[derive(Default)]
pub(in crate::net::players) struct GarageOutput {
    pub output: Output,
    pub removed: Option<(u16, u16)>,
    pub pending: Option<returning::PendingReturn>,
}
pub(in crate::net::players) struct OwnedGarage<'a> {
    pub inventory: &'a Inventory,
    pub content: &'a GarageContent,
}

/// Place the garage presence of an intro garage entry where the official host
/// sends it: right after the participant enters `begin_garage`, before the
/// intro mode's reactions to that state (career start 298018 ms: the walk,
/// presence, four mode calls, leave `begin_garage`, enter `loading_garage`,
/// fifteen messages in one frame). Without mode calls it goes last.
pub(in crate::net::players) fn insert_presence(
    calls: &mut Vec<HostRpc>,
    presence: nfs_world::garage::presence::Notification,
) {
    let at = calls
        .iter()
        .position(|call| matches!(call, HostRpc::GameMode(_)))
        .unwrap_or(calls.len());
    calls.insert(at, HostRpc::GaragePresence(presence));
}

/// The selection's preview slots unload again when the participant leaves
/// the intro garage: VehicleSlot m2 for the participant on each slot, in
/// serializer order (official E1026 562.221 s: garage s23 off, then s15, s16,
/// s17 m2; the selection unloaded them first at 498.920 s; client
/// OnVehicleUnloaded sets the slot Bool false, ph5 5.2).
pub(in crate::net::players) fn preview_slot_unloads(
    garage: &nfs_content::garage::starter::Garage,
    players: &Players,
    participant: u16,
) -> Result<Vec<HostRpc>, replication::Error> {
    let mut slots: Vec<usize> = garage.slots.iter().map(|s| s.component).collect();
    slots.sort_unstable();
    slots
        .into_iter()
        .map(|serializer| {
            let (scene, selector) = players
                .objects()
                .scene_endpoint(garage.scene_key, serializer)
                .ok_or(replication::Error::UnknownObject)?;
            Ok(HostRpc::GameMode(crate::event_logic::call(
                players.objects(),
                scene,
                selector,
                2,
                Some(participant),
                nfs_world::game_mode::Arguments::None,
            )?))
        })
        .collect()
}

pub(in crate::net::players) fn notify(
    content: &crate::prologue::Content,
    modes: &mut Modes,
    players: &mut Players,
    chain: Vec<Notification>,
    out: &mut Output,
) -> Result<(), replication::Error> {
    let (_, canceller) = content
        .driving
        .as_ref()
        .ok_or(replication::Error::Unsupported)?;
    for notification in chain {
        out.calls.push(HostRpc::Participant(notification));
        let (a, b, chapter) = modes
            .get_mut(&notification.participant)
            .ok_or(replication::Error::UnknownObject)?;
        for (active, mode) in [(a, content.mode(*chapter)?), (b, canceller)] {
            let value = active.scene_notification(mode, players, notification)?;
            out.records.extend(value.records);
            out.deleted.extend(value.deleted);
            out.calls.extend(value.calls);
        }
    }
    Ok(())
}

impl PlayerListener {
    /// Retire the driving presentation only after its owned client confirmation.
    /// The caller commits the complete cinematic, actor and notification batch.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::net::players) fn confirmed_intro_return(
        content: &crate::prologue::Content,
        modes: &mut Modes,
        players: &mut Players,
        returns: &mut Arrivals,
        logic: Option<&crate::garage_logic::GarageLogic>,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
        chain: Vec<Notification>,
    ) -> Result<Output, replication::Error> {
        let parent = modes
            .get(&participant)
            .ok_or(replication::Error::UnknownObject)?
            .0
            .parent();
        let mut out = Output::default();
        if let Some(logic) = logic {
            let batch = logic.produce_present(
                crate::garage_logic::Which::Returning,
                players,
                participant,
                logic_ghosts.entry(participant).or_default(),
                crate::garage_logic::Part::All,
            )?;
            out.records.extend(batch.records);
            out.deleted.extend(batch.deleted);
            out.calls
                .extend(batch.events.into_iter().map(HostRpc::Event));
        }
        // Each intro return stops the driving chapter's sounds.
        for &(bus, ordinal) in &content.return_events {
            let fire = nfs_world::logic::Fire {
                event: ordinal,
                player: None,
                target: nfs_world::logic::EntityRef {
                    ghost: parent,
                    entity: bus,
                },
            };
            fire.encode().map_err(|_| replication::Error::Shape)?;
            out.calls.push(HostRpc::Event(fire));
        }
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant,
        };
        // Scripted control must precede assignment of the cinematic goal.
        out.records
            .extend(players.actor_arriving(owner.connection, persona, participant)?);
        out.records.extend(returns.start_rollout(players, owner)?);
        notify(content, modes, players, chain, &mut out)?;
        Ok(out)
    }

    /// These models are all candidates from the enclosing client-frame transaction.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::net::players) fn intro_garage_input(
        content: &crate::prologue::Content,
        modes: &mut Modes,
        players: &mut Players,
        participants: &mut Lifecycle,
        returns: Option<&mut Arrivals>,
        population: Option<&mut Population>,
        sequences: Option<&mut Sequences>,
        owned: Option<OwnedGarage<'_>>,
        logic: Option<&crate::garage_logic::GarageLogic>,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        world_ms: u64,
        body: BitSpan<'_>,
    ) -> Result<Option<GarageOutput>, replication::Error> {
        use crate::garage_logic::{Part, Which};
        if modes.is_empty() {
            return Ok(None);
        }
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
        let request = participants.return_bindings().is_some_and(|b| {
            b.request.scene == ghost
                && b.request.selector == route.selector()
                && route.method_index() == 0
        });
        let returning = returns.as_ref().is_some_and(|r| {
            r.acknowledges(ghost, route.selector(), route.method_index())
                || (r.has_completion(ghost, route.selector()) && route.method_index() == 1)
        });
        if !request && !returning {
            return Ok(None);
        }
        let &[_, participant] = envelope.references() else {
            return Err(replication::Error::Shape);
        };
        // Once the intro is finished the ordinary garage path owns the
        // participant's requests; the retired modes stay recorded (E1257: the
        // free-roam request was claimed here and refused for good).
        if participants.intro_chapter(participant).is_none() {
            return Ok(None);
        }
        let Some((_, _, chapter @ (Chapter::FirstDrive | Chapter::ReturnDrive))) =
            modes.get(&participant)
        else {
            return Ok(None);
        };
        let final_return = *chapter == Chapter::ReturnDrive;
        if !players.owns_participant(HOST_SELECTOR as u8, persona, participant) {
            return Err(replication::Error::UnknownObject);
        }
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(replication::Error::Shape);
        }
        let mut out = GarageOutput::default();
        if request {
            // The client sends the request when its car enters the
            // SpawnPoints RaceBoxTrigger (trace 11.12), independently of the
            // route's destination checkpoint; the installed request chain
            // (layer9 LocalParticipantEvent → state sequence selector and
            // the state machine channel) has no objective gate. E1248: the
            // request preceded the crossing in both drives, and the drive
            // waited for a second request (first drive) or stayed stuck.
            match participants.begin_intro_return(participant) {
                Outcome::Repeated => return Ok(Some(out)),
                // Not an intro return in this stage: the ordinary path decides
                // (and logs what it cannot use); a well-formed request is
                // never refused for the stage it arrives in.
                Outcome::Unsupported => return Ok(None),
                Outcome::Advanced(chain) => {
                    let returns = returns.ok_or(replication::Error::Unsupported)?;
                    let owner = Owner {
                        connection: HOST_SELECTOR as u8,
                        persona,
                        participant,
                    };
                    returns.reset(players, owner)?;
                    // A return that begins before the destination crossing
                    // releases the drive's route first, as the crossing does.
                    let drive = &mut modes
                        .get_mut(&participant)
                        .ok_or(replication::Error::UnknownObject)?
                        .0;
                    drive.cancel_messages();
                    let released = drive.release_owner_routes(players)?;
                    out.output.records.extend(released.records);
                    out.output.deleted.extend(released.deleted);
                    // Official order (career start 290017-290127 ms): the
                    // return-request event, then the return presentation
                    // replaces the drive's (cinematic logic in, HUD out).
                    if let Some(logic) = logic {
                        let ghosts = logic_ghosts.entry(participant).or_default();
                        let batch = logic.produce_present(
                            Which::ReturnRequested,
                            players,
                            participant,
                            ghosts,
                            Part::All,
                        )?;
                        out.output.records.extend(batch.records);
                        out.output.deleted.extend(batch.deleted);
                        out.output
                            .calls
                            .extend(batch.events.into_iter().map(HostRpc::Event));
                    }
                    let confirmation = logic
                        .map(|logic| {
                            logic.return_confirmation(
                                players,
                                logic_ghosts.get(&participant).unwrap_or(&BTreeMap::new()),
                            )
                        })
                        .transpose()?
                        .flatten();
                    if confirmation.is_some() {
                        out.pending = Some(returning::PendingReturn {
                            participant,
                            intro: true,
                            requested_ms: world_ms,
                            torn_down: false,
                            confirmation,
                            confirmed: false,
                            chain: chain.into_iter().map(HostRpc::Participant).collect(),
                            slots: Vec::new(),
                        });
                    } else {
                        let staged = Self::confirmed_intro_return(
                            content,
                            modes,
                            players,
                            returns,
                            logic,
                            logic_ghosts,
                            persona,
                            participant,
                            chain,
                        )?;
                        out.output.records.extend(staged.records);
                        out.output.deleted.extend(staged.deleted);
                        out.output.calls.extend(staged.calls);
                    }
                    return Ok(Some(out));
                }
            }
        }
        if participants.stage(participant) != Some(Stage::IntroEnteringGarage) {
            return Ok(None);
        }
        let returns = returns.ok_or(replication::Error::Unsupported)?;
        if returns.ghost(participant) != Some(ghost) {
            return Err(replication::Error::UnknownObject);
        }
        if returns.acknowledges(ghost, route.selector(), route.method_index()) {
            return Ok(Some(out));
        }
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant,
        };
        if returns
            .complete(players, owner, Completed::decode(body)?)?
            .is_none()
        {
            return Ok(Some(out));
        }
        let (records, deleted) = returns.finish_rollout(players, owner)?;
        out.output.records.extend(records);
        out.output.deleted.extend(deleted);
        out.output.records.extend(players.bind_player_vehicle(
            owner.connection,
            persona,
            participant,
            None,
        )?);
        let population = population.ok_or(replication::Error::Unsupported)?;
        let vehicle_owner = crate::vehicle_content::population::Owner {
            connection: owner.connection,
            persona,
            participant,
        };
        let removed = if final_return {
            let owned = owned.ok_or(replication::Error::Unsupported)?;
            population.finish_return(
                players,
                vehicle_owner,
                owned.inventory,
                &owned.content.layout,
            )?
        } else {
            population.remove_temporary_world(players, vehicle_owner)?
        };
        out.output.deleted.push(removed);
        out.removed = Some((participant, removed));
        // The cinematic logic goes with the return (career start 298018 ms).
        if let Some(logic) = logic {
            let ghosts = logic_ghosts.entry(participant).or_default();
            let batch = logic.produce_present(
                Which::Returned,
                players,
                participant,
                ghosts,
                Part::Deletions,
            )?;
            out.output.deleted.extend(batch.deleted);
        }
        let Outcome::Advanced(chain) = participants.returned_to_garage(participant) else {
            return Err(replication::Error::Shape);
        };
        notify(content, modes, players, chain, &mut out.output)?;
        let sequences = sequences.ok_or(replication::Error::Unsupported)?;
        sequences.reset(players, owner)?;
        // Both official intro garage waits stopped 8 s after the waiting
        // sequence (E1026 306.019 and 1134.827 s; E1249 trace 3.14).
        out.output.records.extend(sequences.start_with_delay(
            players,
            owner,
            nfs_world::sequences::INTRO_FIRST_READY_MS,
        )?);
        Ok(Some(out))
    }
}
