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
}
pub(in crate::net::players) struct OwnedGarage<'a> {
    pub inventory: &'a Inventory,
    pub content: &'a GarageContent,
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
        let Some((active, _, chapter @ (Chapter::FirstDrive | Chapter::ReturnDrive))) =
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
            if !active.destination_reached() {
                return Ok(Some(out));
            }
            match participants.begin_intro_return(participant) {
                Outcome::Repeated => return Ok(Some(out)),
                Outcome::Unsupported => return Err(replication::Error::Shape),
                Outcome::Advanced(chain) => {
                    let returns = returns.ok_or(replication::Error::Unsupported)?;
                    let owner = Owner {
                        connection: HOST_SELECTOR as u8,
                        persona,
                        participant,
                    };
                    returns.reset(players, owner)?;
                    // Official order (career start 290017-290127 ms): the
                    // return-request event, then the return presentation
                    // replaces the drive's (cinematic logic in, HUD out).
                    if let Some(logic) = logic {
                        let ghosts = logic_ghosts.entry(participant).or_default();
                        for which in [Which::ReturnRequested, Which::Returning] {
                            let batch = logic.produce_present(
                                which,
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
                    }
                    out.output
                        .records
                        .extend(returns.start_rollout(players, owner)?);
                    notify(content, modes, players, chain, &mut out.output)?;
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
        out.output.records.extend(sequences.start(players, owner)?);
        Ok(Some(out))
    }
}
