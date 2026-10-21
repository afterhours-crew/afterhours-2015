// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! In-world restart handshake. State calls use the live startup scene's
//! serializers; the destination and fade-in layout come from installed content.
use super::*;
use nfs_world::participants::{Call, Lifecycle, Notification};

#[derive(Clone, Copy, Debug)]
enum Phase {
    Requested,
    Leaving(u64),
    Sending(u64),
    Teleporting { deadline: u64 },
    Loading { deadline: u64 },
    Arriving { sequence: u16, deadline: u64 },
    Resuming(u64),
}
#[derive(Clone, Debug)]
pub(super) struct Instance {
    car: u16,
    destination: nfs_content::events::restart::Destination,
    phase: Phase,
    bindings: Option<Bindings>,
}
impl Instance {
    pub(super) fn new(car: u16, destination: nfs_content::events::restart::Destination) -> Self {
        Self {
            car,
            destination,
            phase: Phase::Requested,
            bindings: None,
        }
    }

    /// After the teleport acknowledgement, the client's car is at the new
    /// location. Populate its surroundings while rendering/arrival completes.
    /// Free-roam presentation is a later, independent gate.
    fn has_teleported(&self) -> bool {
        matches!(
            self.phase,
            Phase::Loading { .. } | Phase::Arriving { .. } | Phase::Resuming(_)
        )
    }
}

impl State {
    pub(in crate::net::players) fn restart_arrivals(&self) -> impl Iterator<Item = u16> + '_ {
        self.restarts
            .iter()
            .filter_map(|(&p, r)| r.has_teleported().then_some(p))
    }

    pub(super) fn restart_resuming(&self) -> impl Iterator<Item = u16> + '_ {
        self.restarts
            .iter()
            .filter_map(|(&p, r)| matches!(r.phase, Phase::Resuming(_)).then_some(p))
    }
}
#[derive(Clone, Copy, Debug)]
struct Bindings {
    free: Endpoint,
    pre: Endpoint,
    leave: Endpoint,
    teleport: Endpoint,
    check: Endpoint,
    wait: Endpoint,
    ready: Endpoint,
    arrive: Endpoint,
}
impl Bindings {
    fn new(players: &Players, startup: u32) -> Result<Self, replication::Error> {
        let endpoint = |field| {
            let (scene, selector) = players
                .objects()
                .scene_endpoint(startup, field)
                .ok_or(replication::Error::UnknownObject)?;
            let serial =
                event_logic::call(players.objects(), scene, selector, 0, None, Arguments::None)?
                    .serial;
            Ok(Endpoint {
                scene,
                selector,
                serial,
            })
        };
        // E994 restart, same installed startup layout as garage exit.
        Ok(Self {
            free: endpoint(2)?,
            pre: endpoint(115)?,
            leave: endpoint(85)?,
            teleport: endpoint(77)?,
            check: endpoint(83)?,
            wait: endpoint(78)?,
            ready: endpoint(81)?,
            arrive: endpoint(79)?,
        })
    }
}
fn calls(participant: u16, chain: &[(Endpoint, Call)]) -> Vec<HostRpc> {
    chain
        .iter()
        .map(|&(endpoint, call)| {
            HostRpc::Participant(Notification {
                endpoint,
                participant,
                call,
            })
        })
        .collect()
}
fn publish(out: &mut Out, participant: u16, chain: &[(Endpoint, Call)]) {
    out.entries
        .push((packed(Vec::new(), Vec::new()), calls(participant, chain)));
}

/// Restart replaces presentation roles, including the free-roam stats role.
/// Delete the old identities before allocating replacements under the same
/// role keys. The caller commits this map atomically with Players and output.
pub(super) fn presentation(
    listener: &PlayerListener,
    which: crate::garage_logic::Which,
    players: &mut Players,
    participant: u16,
    out: &mut Out,
) -> Result<crate::garage_logic::Output, replication::Error> {
    let ghosts = out
        .logic_ghosts
        .get_or_insert_with(|| listener.logic_ghosts.clone());
    PlayerListener::arrive_logic(
        listener.logic.as_deref(),
        which,
        players,
        participant,
        ghosts,
    )
}
pub(super) fn claims(state: &State, refs: &[u16], selector: u16, method: u32) -> bool {
    let &[scene, participant] = refs else {
        return false;
    };
    let Some(instance) = state.restarts.get(&participant) else {
        return false;
    };
    let Some(b) = instance.bindings else {
        return false;
    };
    (scene == b.teleport.scene && selector == b.teleport.selector && method == 3)
        || (scene == b.ready.scene && selector == b.ready.selector && method == 0)
        || matches!(instance.phase, Phase::Arriving { sequence, .. } if scene == sequence && selector == 0 && method == 1)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn input(
    listener: &PlayerListener,
    content: &event_logic::Content,
    players: &mut Players,
    state: &mut State,
    body: BitSpan<'_>,
    now: u64,
    out: &mut Out,
) -> Result<bool, replication::Error> {
    let Some((refs, selector, method, bits)) = route(body) else {
        return Ok(false);
    };
    if !claims(state, &refs, selector, method) {
        return Ok(false);
    }
    let participant = refs[1];
    if bits != 7 || !players.owns_participant(HOST_SELECTOR as u8, listener.persona, participant) {
        return Err(replication::Error::Shape);
    }
    let instance = state
        .restarts
        .get_mut(&participant)
        .ok_or(replication::Error::UnknownObject)?;
    let b = instance.bindings.ok_or(replication::Error::Shape)?;
    match instance.phase {
        Phase::Teleporting { .. }
            if refs[0] == b.teleport.scene && selector == b.teleport.selector =>
        {
            publish(
                out,
                participant,
                &[
                    (b.teleport, Call::Leave),
                    (b.check, Call::Enter),
                    (b.check, Call::Leave),
                    (b.wait, Call::Enter),
                ],
            );
            instance.phase = Phase::Loading {
                deadline: now.saturating_add(180_000),
            };
        }
        Phase::Loading { .. } if refs[0] == b.ready.scene && selector == b.ready.selector => {
            let mut creation = content.sequence_creation(
                names::RESTART_ARRIVAL,
                players.objects(),
                participant,
                0,
            )?;
            // This authored sequence connects server Started straight to Stop.
            // Its client fade completes asynchronously and acknowledges the root.
            let initial = creation
                .body
                .initial
                .as_mut()
                .ok_or(replication::Error::Shape)?;
            let [nfs_world::replication::entity::Initial::SequenceRoot { stopping, .. }] =
                initial.as_mut_slice()
            else {
                return Err(replication::Error::Unsupported);
            };
            *stopping = true;
            let record = players
                .spawn_entities(vec![creation], content.creation_content())?
                .remove(0);
            let sequence = record.id;
            let mut presentation = presentation(
                listener,
                crate::garage_logic::Which::InWorldArrival,
                players,
                participant,
                out,
            )?;
            presentation.records.push(record);
            let mut events: Vec<_> = presentation
                .events
                .into_iter()
                .map(HostRpc::Event)
                .collect();
            events.extend(calls(
                participant,
                &[(b.wait, Call::Leave), (b.arrive, Call::Enter)],
            ));
            out.entries
                .push((packed(presentation.records, presentation.deleted), events));
            instance.phase = Phase::Arriving {
                sequence,
                deadline: now.saturating_add(30_000),
            };
        }
        Phase::Arriving { sequence, .. } if refs[0] == sequence => {
            let completed = Completed::decode(body)?;
            if completed.sequence != sequence || completed.participant != participant {
                return Err(replication::Error::Shape);
            }
            players.remove_entities(&[sequence])?;
            let mut presentation = presentation(
                listener,
                crate::garage_logic::Which::Arrived,
                players,
                participant,
                out,
            )?;
            presentation.deleted.push(sequence);
            let mut events: Vec<_> = presentation
                .events
                .into_iter()
                .map(HostRpc::Event)
                .collect();
            events.extend(calls(participant, &[(b.arrive, Call::Leave)]));
            out.entries
                .push((packed(presentation.records, presentation.deleted), events));
            instance.phase = Phase::Resuming(now.saturating_add(FOLLOW_MS));
        }
        _ => {} // Repeated or early acknowledgements cannot skip a phase.
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn advance(
    players: &mut Players,
    participants: &mut Lifecycle,
    state: &mut State,
    persona: u64,
    now: u64,
    out: &mut Out,
) -> Result<Vec<u16>, replication::Error> {
    if state.failed && !state.restarts.is_empty() {
        return Err(replication::Error::Shape);
    }
    let mut done = Vec::new();
    for (&participant, instance) in &mut state.restarts {
        if !players.owns_participant(HOST_SELECTOR as u8, persona, participant) {
            return Err(replication::Error::UnknownObject);
        }
        match instance.phase {
            Phase::Requested => {
                let scene = participants
                    .exit_bindings()
                    .ok_or(replication::Error::Unsupported)?
                    .state_77
                    .scene;
                let Some(nfs_world::replication::Initial::SubLevel { prefix, .. }) =
                    players.objects().get(scene).map(|o| &o.initial)
                else {
                    return Err(replication::Error::UnknownObject);
                };
                let b = Bindings::new(players, prefix.content_key)?;
                participants.begin_relocation(participant)?;
                publish(
                    out,
                    participant,
                    &[
                        (b.free, Call::Leave),
                        (b.pre, Call::Enter),
                        (b.pre, Call::Leave),
                        (b.leave, Call::Enter),
                    ],
                );
                instance.bindings = Some(b);
                instance.phase = Phase::Leaving(now.saturating_add(FOLLOW_MS));
                tracing::info!(participant, "owned event restart leaving world");
            }
            Phase::Leaving(at) if now >= at => {
                let b = instance.bindings.ok_or(replication::Error::Shape)?;
                publish(
                    out,
                    participant,
                    &[(b.leave, Call::Leave), (b.teleport, Call::Enter)],
                );
                instance.phase = Phase::Sending(now.saturating_add(FOLLOW_MS));
            }
            Phase::Sending(at) if now >= at => {
                let destination = instance.destination;
                let command = nfs_world::teleport::Command {
                    endpoint: instance.bindings.ok_or(replication::Error::Shape)?.teleport,
                    participant,
                    vehicle: instance.car,
                    position: destination.position,
                    basis: destination.basis,
                    speed: 0.,
                };
                command.encode()?;
                out.teleports.push(command);
                instance.phase = Phase::Teleporting {
                    deadline: now.saturating_add(180_000),
                };
            }
            Phase::Teleporting { deadline }
            | Phase::Loading { deadline }
            | Phase::Arriving { deadline, .. }
                if now >= deadline =>
            {
                return Err(replication::Error::Shape);
            }
            Phase::Resuming(at) if now >= at => {
                let b = instance.bindings.ok_or(replication::Error::Shape)?;
                participants.finish_relocation(participant)?;
                publish(out, participant, &[(b.free, Call::Enter)]);
                done.push(participant);
                tracing::info!(
                    participant,
                    "owned event restart returned to start; free roam resumes"
                );
            }
            _ => {}
        }
    }
    for participant in &done {
        state.restarts.remove(participant);
    }
    Ok(done)
}
