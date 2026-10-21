// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned race event lifecycle for this connection's participants (E994–E996), in
//! the official host's order for a solo participant:
//! - free roam: eligibility and mission notifications set, the start volume
//!   subscribed (removed again when the participant leaves free roam);
//! - start volume entered: the start-area logics and the external-set filters of
//!   this area and the linked variant area; left: deleted (the filters also during
//!   a race, the logics only outside one);
//! - PageDown (client event on the start prompt): the game mode, its first logics
//!   and the intro sequence, and the in-event flags (E997); free-roam scoring is
//!   suspended; the scene and Events logics, the launcher's game-mode call, the
//!   checkpoint enables and the intro's AI goals on the participant's Actor follow;
//! - intro played: Stop, completion, the Actor's release from its goals, the
//!   countdown sequence and state with the start time; countdown completed: the
//!   waypoints state with the car, positions and the DNF deadline;
//! - each checkpoint in route order: progress and a new DNF deadline; the last one
//!   finishes the race: results logics, the outro sequence and the durable rewards;
//! - outro played: the final states with the in-event flags cleared, free-roam
//!   scoring resumes, the cooldown logic; the client's leave then deletes the game
//!   mode and disables the checkpoints;
//! - restart, leave or a passed DNF deadline: the DNF-triggered exit, the same leave.
//!
//! Calls on a newly created object follow one host frame later; calls on objects
//! about to be deleted precede their deletion in one frame. Times are f64 seconds
//! on the host replication clock, which the client's own reports share here.
use super::*;
use crate::event_logic::{self, Finish, Owner, State as Gm, request as names};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile, Serial},
};
use nfs_world::{
    game_mode::{Arguments, Crossing, Leave},
    logic::Message,
    participants::{Endpoint, HostRpc},
    sequences::{Completed, Stop},
    world_activation,
};

mod ai;
mod restart;

/// Client logic events on entity 1 of the named object (E995).
const START: u32 = 12_645_288;
const INTRO_READY: u32 = 14_506_768;
const OUTRO_READY: u32 = 5_775_918;
const RESTART: u32 = 2_481_527;
const LEAVE: u32 = 17_642_391;
/// One host frame between a creation and the calls on its objects (E994: 100 ms).
const FOLLOW_MS: u64 = 100;
/// Finish calls follow the results creations by two frames (E994: 206 ms).
const FINISH_FOLLOW_MS: u64 = 200;
/// The countdown's start lies this far after its call (E994: 0.265 s; inferred as
/// the official host's lead, not traced to a content value).
const COUNTDOWN_LEAD_S: f64 = 0.265;
/// The cooldown logic's lifetime (E994: deleted 9,999 ms and 9,993 ms after creation).
const COOLDOWN_MS: u64 = 10_000;
/// Stop the intro without the client's ready event after this long (local policy;
/// E994's intros played for about 13 s).
const INTRO_FALLBACK_MS: u64 = 30_000;
/// Delays of the in-event flags after the call that sets or clears the first ones,
/// in request order (E997: Collectibles 0.6 s and Activities 0.9 s after Events and
/// UI in every official attempt; observed host timing, not traced to content).
const FLAG_DELAYS_MS: [u64; 5] = [0, 0, 0, 600, 900];
pub(super) const MAX_INPUTS: usize = 64;
const MAX_STEPS: usize = 64;
const MAX_ENTRIES: usize = 16;

#[derive(Clone, Debug)]
pub(in crate::net) enum Input {
    Logic(Message),
    Rpc(Vec<u8>, usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Intro,
    Countdown,
    Racing,
    Outro,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Exit {
    Completed,
    Restart,
    Abandon,
    TimedOut,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Logic {
    Sound,
    Intro,
    Telemetry,
    Restart,
    Leave,
    ResultTracking,
    OngoingScore,
    AddSet(usize),
    DrivingStyle,
    Awards,
    Outro,
}
impl Logic {
    fn name(self) -> (&'static str, usize) {
        match self {
            Self::Sound => (names::SOUND, 0),
            Self::Intro => (names::INTRO, 0),
            Self::Telemetry => (names::TELEMETRY, 0),
            Self::Restart => (names::RESTART, 0),
            Self::Leave => (names::LEAVE, 0),
            Self::ResultTracking => (names::RESULT_TRACKING, 0),
            Self::OngoingScore => (names::ONGOING_SCORE, 0),
            Self::AddSet(n) => (names::ADD_SET, n),
            Self::DrivingStyle => (names::DRIVING_STYLE, 0),
            Self::Awards => (names::AWARDS, 0),
            Self::Outro => (names::OUTRO, 0),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sequence {
    Intro,
    Countdown,
    Outro,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// Scene, Events and game-mode logics after the game mode; the launcher's
    /// game-mode call and the checkpoint enables.
    Started,
    /// Countdown follow-up: scoring logics and the countdown sequence's Stop.
    Counting,
    /// The race begins: driving-style logic and the waypoints state.
    Racing,
    /// Finish calls after the results creations.
    Finished,
    /// Final-state calls after the outro.
    Ending,
    /// Exit calls after a restart or leave choice.
    Exiting,
    /// The intro's Stop when the client never reported it played.
    IntroFallback,
    /// A delayed in-event flag call (method 0 sets, 1 clears).
    Flag { method: u32, index: usize },
}

#[derive(Clone, Debug)]
struct Race {
    game_mode: u16,
    car: u16,
    phase: Phase,
    logics: BTreeMap<Logic, u16>,
    /// Event-scene and Events-scene logics created with the game mode.
    scene: Vec<u16>,
    sequence: Option<(Sequence, u16, bool)>,
    start: Option<f64>,
    next: usize,
    lap: u8,
    progress: Option<nfs_world::race_progress::Race>,
    deadline: Option<f64>,
    finish: Option<Finish>,
    exit: Option<Exit>,
    awaiting_leave: bool,
    /// The intro's AI goals on the participant's Actor, while it holds them.
    goals: Vec<u16>,
    /// Whether the in-event flags are set.
    flags: bool,
    ai: Option<ai::Grid>,
}
#[derive(Clone, Debug, Default)]
struct Entry {
    inside: bool,
    area: Vec<u16>,
    filters: Vec<u16>,
    race: Option<Race>,
}

#[derive(Clone, Debug, Default)]
pub(in crate::net) struct State {
    active: BTreeSet<u16>,
    eligible: BTreeSet<u16>,
    entries: BTreeMap<u16, Entry>,
    pub(in crate::net) inputs: VecDeque<Input>,
    steps: VecDeque<(u64, u16, Step)>,
    cooldowns: VecDeque<(u64, u16)>,
    /// Completions of this session, the event half of the credit batch IDs.
    completed: u64,
    ai_cursor: crate::event_ai::Cursor,
    failed: bool,
    restarts: BTreeMap<u16, restart::Instance>,
    /// Next authored location in this session's event restart grid.
    restart_cursor: usize,
}

/// One frame's event output.
#[derive(Default)]
struct Out {
    records: Vec<replication::Record>,
    deleted: Vec<u16>,
    rpcs: Vec<HostRpc>,
    entries: Vec<(Section, Vec<HostRpc>)>,
    messages: Vec<nfs_world::content::Message>,
    registrations: Option<crate::vehicle_content::registrations::Registry>,
    rivals: Option<crate::rival_content::population::Population>,
    teleports: Vec<nfs_world::teleport::Command>,
    // Clone only when a presentation lifecycle changes; commit with the world
    // and output queues so a rejected batch cannot lose or duplicate roles.
    logic_ghosts: Option<BTreeMap<u16, BTreeMap<u16, u16>>>,
}

fn route(body: BitSpan<'_>) -> Option<(Vec<u16>, u16, u32, usize)> {
    let e = Envelope::decode(
        body,
        Limits {
            max_input_bits: 4096,
            max_references: 32,
            max_payload_bytes: 256,
        },
    )
    .ok()?;
    let r = e.route(RouteProfile::ClientSend).ok()?;
    Some((
        e.references().to_vec(),
        r.selector(),
        r.method_index(),
        r.arguments().len(),
    ))
}
fn owned(body: BitSpan<'_>) -> (Vec<u8>, usize) {
    let mut w = nfs_world::bits::BitWriter::new();
    w.put_span(body);
    let bits = w.len();
    (w.into_bytes(), bits)
}
/// Set (method 0) or clear (method 1) the in-event flags: the first in this frame,
/// the others after their official delays. A clear drops pending sets first.
fn event_flags(
    content: &event_logic::Content,
    players: &Players,
    state: &mut State,
    participant: u16,
    method: u32,
    now_ms: u64,
    out: &mut Out,
) -> Result<(), replication::Error> {
    if method == 1 {
        state
            .steps
            .retain(|(_, p, s)| *p != participant || !matches!(s, Step::Flag { method: 0, .. }));
    }
    for index in 0..content.event().game_mode_notifications.len() {
        match FLAG_DELAYS_MS.get(index).copied().unwrap_or(0) {
            0 => out.rpcs.push(HostRpc::GameMode(content.flag_call(
                players.objects(),
                index,
                method,
            )?)),
            delay => {
                state
                    .steps
                    .push_back((now_ms + delay, participant, Step::Flag { method, index }))
            }
        }
    }
    Ok(())
}
/// Release the Actor from the intro's goals and delete them in this frame (E997:
/// with the intro's deletion). A refused release is logged; the goals then stay.
fn release_goals(
    players: &mut Players,
    persona: u64,
    participant: u16,
    goals: Vec<u16>,
    out: &mut Out,
) {
    if goals.is_empty() {
        return;
    }
    match players.release_controlled_goals(HOST_SELECTOR as u8, persona, participant, &goals) {
        Ok(cleared) => {
            tracing::info!(participant, ?goals, "owned event intro goals released");
            out.records.push(cleared);
            out.deleted.extend(goals);
        }
        Err(error) => {
            tracing::warn!(
                participant,
                ?error,
                ?goals,
                "owned event intro goals not released"
            )
        }
    }
}
fn packed(records: Vec<replication::Record>, deleted: Vec<u16>) -> Section {
    Section {
        float_bits: None,
        flag: false,
        deleted,
        setup: (!records.is_empty()).then_some(Setup::Packed {
            tag: None,
            width: None,
            axes: [None; 3],
        }),
        records,
    }
}

impl PlayerListener {
    pub(in crate::net) fn with_events(
        mut self,
        events: Option<std::sync::Arc<crate::event_logic::Content>>,
    ) -> Self {
        self.events = events;
        self
    }
    /// Schedule transactional teardown after a fault. Keep command ownership and
    /// delegation until its release can be published. A second fault in teardown
    /// requires closing the session instead of abandoning owned objects.
    pub(in crate::net) fn disable_events(&mut self) -> bool {
        if self.event_state.failed {
            return false;
        }
        self.event_state.failed = true;
        self.event_state.inputs.clear();
        self.event_state.steps.clear();
        true
    }
    /// The participant's race driving-style logic, from the race start until
    /// the results replace it; scoring runs the event participant on it.
    pub(super) fn event_driving_style(&self, participant: u16) -> Option<u16> {
        self.event_state
            .entries
            .get(&participant)?
            .race
            .as_ref()?
            .logics
            .get(&Logic::DrivingStyle)
            .copied()
    }
    fn event_seconds(&self, now_ms: u64) -> Option<f64> {
        let origin = self.replication_origin_ms?;
        Some(now_ms.saturating_sub(origin) as f64 / 1000.0)
    }

    /// Whether a client logic event belongs to an owned event object.
    pub(super) fn event_claims_logic(&self, message: &Message) -> bool {
        let Message::Reached { event, target, .. } = message else {
            return false;
        };
        if self.events.is_none() || target.entity != 1 {
            return false;
        }
        self.event_state.entries.values().any(|entry| {
            (*event == START && entry.area.contains(&target.ghost))
                || entry.race.as_ref().is_some_and(|race| {
                    matches!(*event, INTRO_READY | OUTRO_READY)
                        && race
                            .sequence
                            .is_some_and(|(_, ghost, _)| ghost == target.ghost)
                        || matches!(*event, RESTART | LEAVE)
                            && race.logics.values().any(|g| *g == target.ghost)
                })
        })
    }
    /// Whether a client call belongs to an owned event: a start-volume or checkpoint
    /// crossing, an event sequence's completion or the game mode's leave.
    pub(super) fn event_claims_rpc(&self, body: BitSpan<'_>) -> bool {
        let Some(content) = self.events.as_deref() else {
            return false;
        };
        let Some((refs, selector, method, _)) = route(body) else {
            return false;
        };
        let Some(&first) = refs.first() else {
            return false;
        };
        if restart::claims(&self.event_state, &refs, selector, method) {
            return true;
        }
        let event = content.event();
        let world = self.players.objects();
        let scene = world.scene(event.scene_content_key);
        if Some(first) == scene && matches!(method, 2 | 3) {
            let start = world
                .scene_endpoint(event.scene_content_key, event.start_volume)
                .map(|(_, s)| s);
            if start == Some(selector) {
                return true;
            }
            return event
                .game_mode
                .checkpoints
                .iter()
                .any(|&(k, s)| world.scene_endpoint(k, s).map(|(_, sel)| sel) == Some(selector))
                && self.event_state.entries.values().any(|e| e.race.is_some());
        }
        self.event_state.entries.values().any(|entry| {
            entry.race.as_ref().is_some_and(|race| {
                (selector == 0
                    && method == 1
                    && race.sequence.is_some_and(|(_, ghost, _)| ghost == first))
                    || (selector == 0 && method == 0 && first == race.game_mode)
            })
        })
    }
    pub(super) fn event_input_logic(message: &Message) -> Input {
        Input::Logic(message.clone())
    }
    pub(super) fn event_input_rpc(body: BitSpan<'_>) -> Input {
        let (bytes, bits) = owned(body);
        Input::Rpc(bytes, bits)
    }

    /// Advance owned events: free-roam membership, queued client inputs, due host
    /// steps, cooldowns and DNF deadlines. Runs between frames on the committed world.
    pub(in crate::net) fn advance_events(&mut self, now_ms: u64) -> Result<(), replication::Error> {
        let Some(content) = self.events.clone() else {
            return Ok(());
        };
        if !self.pending_entries.is_empty() || self.pending.len() >= MAX_PENDING {
            return Ok(());
        }
        let Some(now) = self.event_seconds(now_ms) else {
            return Ok(());
        };
        let mut state = self.event_state.clone();
        let mut players = self.players.clone();
        let mut suspended = self.event_suspended.clone();
        let mut participants = self.participants.clone();
        let mut out = Out::default();
        let mut completions = Vec::new();
        // This event path implements normal-mode humans (request::sprint_race_007); AI
        // racers and roaming rivals are outside the authored invitation set.
        let eligible_humans = players.player_participants().count();
        if let Some(record) = content.invitation_update(&mut players, eligible_humans)? {
            out.records.push(record);
        }
        // Free-roam membership.
        let active: BTreeSet<u16> = self
            .participants
            .in_free_roam()
            .into_iter()
            .filter(|p| !state.failed && self.free_roam_presented.contains(p))
            // Official restart restores event subscriptions before the final
            // FreeRoam entry. Do not wait for that entry's presentation marker.
            .chain(state.restart_resuming().filter(|_| !state.failed))
            .collect();
        if state.failed {
            let participants: Vec<_> = state.entries.keys().copied().collect();
            for participant in participants {
                self.event_part(
                    &content,
                    &mut players,
                    &mut state,
                    &mut suspended,
                    participant,
                    &mut out,
                )?;
            }
            let gone: Vec<_> = state
                .cooldowns
                .drain(..)
                .map(|(_, ghost)| ghost)
                .filter(|&ghost| players.objects().get(ghost).is_some())
                .collect();
            players.remove_entities(&gone)?;
            out.deleted.extend(gone);
            state.inputs.clear();
        } else {
            let joined: Vec<u16> = active.difference(&state.active).copied().collect();
            let left: Vec<u16> = state.active.difference(&active).copied().collect();
            for participant in joined {
                if state.entries.len() >= MAX_ENTRIES {
                    return Err(replication::Error::Bound);
                }
                self.event_join(&content, &players, &mut state, participant, &mut out)?;
            }
            for participant in left {
                self.event_part(
                    &content,
                    &mut players,
                    &mut state,
                    &mut suspended,
                    participant,
                    &mut out,
                )?;
            }
            // Client inputs.
            let inputs = std::mem::take(&mut state.inputs);
            for input in inputs {
                if let Input::Rpc(bytes, bits) = &input
                    && restart::input(
                        self,
                        &content,
                        &mut players,
                        &mut state,
                        BitSpan::new(bytes, 0, *bits).map_err(|_| replication::Error::Shape)?,
                        now_ms,
                        &mut out,
                    )?
                {
                    continue;
                }
                self.event_input(
                    &content,
                    &mut players,
                    &mut state,
                    &mut suspended,
                    input,
                    now_ms,
                    now,
                    &mut out,
                    &mut completions,
                )?;
            }
            // Due steps, in queue order; steps fall due out of queue order (a fallback
            // waits longer than the frames that follow it).
            let (due, later): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut state.steps)
                .into_iter()
                .partition(|(due, _, _)| *due <= now_ms);
            state.steps = later;
            for (_, participant, step) in due {
                self.event_step(
                    &content,
                    &mut players,
                    &mut state,
                    participant,
                    step,
                    now_ms,
                    now,
                    &mut out,
                )?;
            }
            while let Some(&(due, ghost)) = state.cooldowns.front() {
                if due > now_ms {
                    break;
                }
                state.cooldowns.pop_front();
                if players.objects().get(ghost).is_some() {
                    players.remove_entities(&[ghost])?;
                    out.deleted.push(ghost);
                }
            }
            // A passed DNF deadline leaves the race as a leave choice does.
            let late: Vec<u16> = state
                .entries
                .iter()
                .filter(|(_, e)| {
                    e.race.as_ref().is_some_and(|r| {
                        r.phase == Phase::Racing
                            && r.exit.is_none()
                            && r.deadline.is_some_and(|d| now >= d)
                    })
                })
                .map(|(p, _)| *p)
                .collect();
            for participant in late {
                tracing::info!(
                    participant,
                    "owned event DNF deadline passed; leaving the race"
                );
                self.event_exit(
                    &content,
                    &mut players,
                    &mut state,
                    &mut suspended,
                    participant,
                    Exit::TimedOut,
                    now_ms,
                    &mut out,
                )?;
            }
            self.event_ai_tick(&content, &mut players, &mut state, now_ms, &mut out)?;
        }
        for participant in restart::advance(
            &mut players,
            &mut participants,
            &mut state,
            self.persona,
            now_ms,
            &mut out,
        )? {
            suspended.remove(&participant);
        }
        let mut sections = if out.registrations.is_some() {
            split_scenes(
                out.records,
                nfs_world::application::OUTBOUND_FRAME_BITS - 104,
            )?
            .into_iter()
            .map(|mut section| {
                section.setup = Some(Setup::RawEscape([0; 3]));
                section
            })
            .collect::<Vec<_>>()
        } else if out.records.is_empty() {
            Vec::new()
        } else {
            split_scenes(
                out.records,
                nfs_world::application::OUTBOUND_FRAME_BITS - 104,
            )?
        };
        if !out.deleted.is_empty() {
            sections.push(packed(Vec::new(), out.deleted));
        }
        if state.steps.len() > MAX_STEPS {
            return Err(replication::Error::Bound);
        }
        if out.rpcs.len()
            > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
            || out.entries.len() > MAX_PENDING.saturating_sub(self.pending_entries.len())
            || sections.len() > MAX_PENDING.saturating_sub(self.pending.len())
            || out.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || out.teleports.len() > MAX_PENDING.saturating_sub(self.pending_teleports.len())
        {
            // Retry the same inputs and allocations after the queues drain.
            return Ok(());
        }
        let mut population = self.population.clone();
        if let Some(registrations) = out.registrations {
            let population = population.as_mut().ok_or(replication::Error::Shape)?;
            let before = population.registrations().clone();
            population.update_registrations(&before, registrations)?;
        }
        state.active = active;
        self.players = players;
        if let Some(ghosts) = out.logic_ghosts {
            self.logic_ghosts = ghosts;
        }
        self.participants = participants;
        self.event_state = state;
        if self.event_state.failed {
            self.events = None;
        }
        self.event_suspended = suspended;
        self.population = population;
        if let Some(rivals) = out.rivals {
            self.rival_population = Some(rivals);
        }
        self.pending_content.extend(out.messages);
        self.pending.extend(sections);
        self.pending_rpcs.extend(out.rpcs);
        self.pending_entries.extend(out.entries);
        self.pending_teleports
            .extend(out.teleports.into_iter().map(|c| (now_ms, c)));
        for request in completions {
            self.scoring_state.pending_awards.push_back(request);
        }
        Ok(())
    }

    fn event_join(
        &self,
        content: &event_logic::Content,
        players: &Players,
        state: &mut State,
        participant: u16,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let world = players.objects();
        let event = content.event();
        if state.eligible.insert(participant) {
            // Eligibility (E994: set at the participant's start, before free roam).
            for &serializer in &event.area_notifications {
                out.rpcs.push(HostRpc::GameMode(
                    content.scene_call(world, serializer, 0, None)?,
                ));
            }
        }
        let (scene, selector) = world
            .scene_endpoint(event.scene_content_key, event.start_volume)
            .ok_or(replication::Error::UnknownObject)?;
        let serial = event_logic::call(world, scene, selector, 0, None, Arguments::None)?.serial;
        out.rpcs
            .push(HostRpc::WorldActivation(world_activation::Notification {
                endpoint: Endpoint {
                    scene,
                    selector,
                    serial,
                },
                participant: Some(participant),
                call: world_activation::Call::Subscribe {
                    token: 0,
                    inside: false,
                },
            }));
        for &serializer in &event.scene_notifications {
            out.rpcs.push(HostRpc::GameMode(
                content.scene_call(world, serializer, 0, None)?,
            ));
        }
        state.entries.entry(participant).or_default();
        tracing::info!(participant, "owned event start volume subscribed");
        Ok(())
    }

    fn event_part(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        suspended: &mut BTreeSet<u16>,
        participant: u16,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let event = content.event();
        if players.owns_participant(HOST_SELECTOR as u8, self.persona, participant) {
            let world = players.objects();
            let (scene, selector) = world
                .scene_endpoint(event.scene_content_key, event.start_volume)
                .ok_or(replication::Error::UnknownObject)?;
            let serial =
                event_logic::call(world, scene, selector, 1, None, Arguments::None)?.serial;
            out.rpcs
                .push(HostRpc::WorldActivation(world_activation::Notification {
                    endpoint: Endpoint {
                        scene,
                        selector,
                        serial,
                    },
                    participant: Some(participant),
                    call: world_activation::Call::Unsubscribe,
                }));
            for &serializer in &event.scene_notifications {
                out.rpcs.push(HostRpc::GameMode(
                    content.scene_call(world, serializer, 1, None)?,
                ));
            }
        }
        if let Some(entry) = state.entries.get_mut(&participant) {
            let mut gone: Vec<u16> = entry
                .area
                .drain(..)
                .chain(entry.filters.drain(..))
                .collect();
            if let Some(mut race) = entry.race.take() {
                self.event_ai_stop(players, &mut race, out)?;
                gone.extend(race.logics.values());
                gone.extend(race.scene);
                gone.extend(race.sequence.map(|(_, g, _)| g));
                gone.push(race.game_mode);
                suspended.remove(&participant);
                if players.owns_participant(HOST_SELECTOR as u8, self.persona, participant) {
                    release_goals(players, self.persona, participant, race.goals, out);
                    // Leaving free roam mid-race clears the flags at once (local
                    // policy; no official departure was observed).
                    if race.flags {
                        for index in 0..event.game_mode_notifications.len() {
                            out.rpcs.push(HostRpc::GameMode(content.flag_call(
                                players.objects(),
                                index,
                                1,
                            )?));
                        }
                    }
                }
            }
            gone.retain(|g| players.objects().get(*g).is_some());
            players.remove_entities(&gone)?;
            out.deleted.extend(gone);
        }
        // A late report sent before the client applied the unsubscription finds
        // no entry and changes nothing (E838's pattern for world volumes).
        state.entries.remove(&participant);
        state.steps.retain(|(_, p, _)| *p != participant);
        tracing::info!(participant, "owned event start volume released");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn event_input(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        suspended: &mut BTreeSet<u16>,
        input: Input,
        now_ms: u64,
        now: f64,
        out: &mut Out,
        completions: &mut Vec<AwardRequest>,
    ) -> Result<(), replication::Error> {
        match input {
            Input::Logic(Message::Reached { event, target, .. }) => {
                let Some((&participant, entry)) = state.entries.iter().find(|(_, e)| {
                    e.area.contains(&target.ghost)
                        || e.race.as_ref().is_some_and(|r| {
                            r.sequence.is_some_and(|(_, g, _)| g == target.ghost)
                                || r.logics.values().any(|g| *g == target.ghost)
                        })
                }) else {
                    return Ok(());
                };
                let showing = entry.area.get(1) == Some(&target.ghost);
                let idle = entry.race.is_none();
                let choice = entry.race.as_ref().map(|r| {
                    (
                        r.logics.get(&Logic::Restart).copied(),
                        r.logics.get(&Logic::Leave).copied(),
                        r.exit.is_some(),
                    )
                });
                match event {
                    START if showing && idle => self.event_start(
                        content,
                        players,
                        state,
                        suspended,
                        participant,
                        now_ms,
                        now,
                        out,
                    ),
                    INTRO_READY | OUTRO_READY => {
                        let entry = state
                            .entries
                            .get_mut(&participant)
                            .ok_or(replication::Error::Shape)?;
                        let Some(race) = entry.race.as_mut() else {
                            return Ok(());
                        };
                        if let Some((which, ghost, stopping)) = race.sequence.as_mut()
                            && *ghost == target.ghost
                            && !*stopping
                            && matches!(
                                (*which, event),
                                (Sequence::Intro, INTRO_READY) | (Sequence::Outro, OUTRO_READY)
                            )
                        {
                            *stopping = true;
                            out.rpcs.push(HostRpc::SequenceStop(Stop {
                                sequence: *ghost,
                                serial: Serial::new(1).ok_or(replication::Error::Shape)?,
                            }));
                            tracing::info!(
                                participant,
                                sequence = *ghost,
                                "owned event sequence played; stopping it"
                            );
                        }
                        Ok(())
                    }
                    RESTART | LEAVE => {
                        let Some((restart, leave, exiting)) = choice else {
                            return Ok(());
                        };
                        let (exit, wanted) = if event == RESTART {
                            (Exit::Restart, restart)
                        } else {
                            (Exit::Abandon, leave)
                        };
                        if wanted != Some(target.ghost) || exiting {
                            return Ok(());
                        }
                        self.event_exit(
                            content,
                            players,
                            state,
                            suspended,
                            participant,
                            exit,
                            now_ms,
                            out,
                        )
                    }
                    _ => Ok(()),
                }
            }
            Input::Logic(_) => Ok(()),
            Input::Rpc(bytes, bits) => {
                let body = BitSpan::new(&bytes, 0, bits).map_err(|_| replication::Error::Shape)?;
                self.event_rpc(
                    content,
                    players,
                    state,
                    suspended,
                    body,
                    now_ms,
                    now,
                    out,
                    completions,
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn event_rpc(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        suspended: &mut BTreeSet<u16>,
        body: BitSpan<'_>,
        now_ms: u64,
        now: f64,
        out: &mut Out,
        completions: &mut Vec<AwardRequest>,
    ) -> Result<(), replication::Error> {
        let event = content.event();
        // A malformed or foreign client call is logged and ignored: it never
        // advances the event and never ends it.
        let ignore = |what: &str| {
            tracing::info!(what, "owned event ignored a client call");
            Ok(())
        };
        let Some((refs, selector, method, bits)) = route(body) else {
            return ignore("undecodable envelope");
        };
        let world = players.objects();
        let scene = world.scene(event.scene_content_key);
        let start_selector = world
            .scene_endpoint(event.scene_content_key, event.start_volume)
            .map(|(_, s)| s);
        // Start-volume crossing: [scene, participant], f64 time, token, padding.
        if refs.first().copied() == scene
            && Some(selector) == start_selector
            && matches!(method, 2 | 3)
        {
            let &[_, participant] = refs.as_slice() else {
                return ignore("start crossing references");
            };
            if bits != 103
                || !players.owns_participant(HOST_SELECTOR as u8, self.persona, participant)
            {
                return ignore("start crossing shape or owner");
            }
            return self.event_area(content, players, state, participant, method == 2, out);
        }
        // Checkpoint crossing: [scene, car], f64 time.
        if refs.first().copied() == scene && matches!(method, 2 | 3) {
            let Ok(Some(crossing)) = Crossing::decode(body) else {
                return ignore("checkpoint crossing shape");
            };
            if !crossing.inside {
                return Ok(());
            }
            let Some((&participant, _)) = state.entries.iter().find(|(_, e)| {
                e.race.as_ref().is_some_and(|r| {
                    matches!(r.phase, Phase::Racing | Phase::Outro)
                        && !matches!(r.exit, Some(Exit::Restart | Exit::Abandon | Exit::TimedOut))
                        && (r.car == crossing.vehicle
                            || r.ai.as_ref().is_some_and(|grid| {
                                grid.racers.members.iter().any(|m| {
                                    m.ids.vehicle == crossing.vehicle
                                        && players.ai_ids(
                                            HOST_SELECTOR as u8,
                                            self.persona,
                                            m.selection.car_key,
                                        ) == Some(m.ids)
                                })
                            }))
                })
            }) else {
                return Ok(());
            };
            return self.event_checkpoint(
                content,
                players,
                state,
                suspended,
                participant,
                crossing,
                now_ms,
                now,
                out,
                completions,
            );
        }
        let Some(&first) = refs.first() else {
            return Ok(());
        };
        let Some((participant, game_mode, awaiting, sequence)) =
            state.entries.iter().find_map(|(&p, e)| {
                e.race
                    .as_ref()
                    .filter(|r| {
                        r.game_mode == first || r.sequence.is_some_and(|(_, g, _)| g == first)
                    })
                    .map(|r| (p, r.game_mode, r.awaiting_leave, r.sequence))
            })
        else {
            return Ok(());
        };
        if first == game_mode {
            let Ok(Some(leave)) = Leave::decode(body) else {
                return ignore("game-mode call");
            };
            if leave.participant != participant || !awaiting {
                return Ok(());
            }
            return self.event_left(content, players, state, participant, out);
        }
        let Ok(completed) = Completed::decode(body) else {
            return ignore("sequence call");
        };
        if completed.participant != participant {
            return ignore("sequence completion by another participant");
        }
        let Some((which, ghost, stopping)) = sequence else {
            return Ok(());
        };
        if ghost != completed.sequence || !(stopping || which == Sequence::Countdown) {
            return Ok(());
        }
        match which {
            Sequence::Intro => self.event_intro_done(
                content,
                players,
                state,
                participant,
                ghost,
                now_ms,
                now,
                out,
            ),
            Sequence::Countdown => {
                self.event_countdown_done(players, state, participant, ghost, now_ms, out)
            }
            Sequence::Outro => self.event_outro_done(
                content,
                players,
                state,
                suspended,
                participant,
                ghost,
                now_ms,
                out,
            ),
        }
    }

    fn event_area(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        inside: bool,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let Some(entry) = state.entries.get_mut(&participant) else {
            return Ok(());
        };
        if entry.inside == inside {
            return Ok(());
        }
        entry.inside = inside;
        let racing = entry.race.is_some();
        if inside {
            let mut creations = Vec::new();
            if !racing {
                for name in [
                    names::START_AREA,
                    names::SHOW_START,
                    names::SHOW_PARTICIPANTS,
                ] {
                    let layout = content.logic(Owner::Scene, name, 0)?;
                    creations.push(content.area_logic(layout, players.objects(), participant)?);
                }
            }
            let mut filters = vec![content.logic(Owner::Scene, names::EXTERNAL_FILTER, 0)?];
            filters.extend(content.linked());
            for layout in filters {
                creations.push(content.area_logic(layout, players.objects(), participant)?);
            }
            let records = players.spawn_entities(creations, content.creation_content())?;
            let split = if racing { 0 } else { 3 };
            entry.area = records[..split].iter().map(|r| r.id).collect();
            entry.filters = records[split..].iter().map(|r| r.id).collect();
            tracing::info!(participant, area = ?entry.area, filters = ?entry.filters, "owned event start area entered");
            out.records.extend(records);
        } else {
            let mut gone = std::mem::take(&mut entry.filters);
            if !racing {
                gone.append(&mut entry.area);
            }
            players.remove_entities(&gone)?;
            tracing::info!(participant, deleted = ?gone, "owned event start area left");
            out.deleted.extend(gone);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn event_start(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        suspended: &mut BTreeSet<u16>,
        participant: u16,
        now_ms: u64,
        now: f64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let car = self
            .world_cars
            .iter()
            .find(|(_, (p, ready))| *p == participant && *ready)
            .map(|(car, _)| *car)
            .ok_or(replication::Error::Unsupported)?;
        let ai = self.event_grid(content, players, state, out)?;
        let gm = content.game_mode(players.objects(), participant, now)?;
        let gm_record = players
            .spawn_entities(vec![gm], content.creation_content())?
            .remove(0);
        let game_mode = gm_record.id;
        let mut creations = Vec::new();
        for logic in [Logic::Sound, Logic::Intro] {
            let (name, occurrence) = logic.name();
            let layout = content.logic(Owner::GameMode, name, occurrence)?;
            creations.push(content.game_mode_logic(
                layout,
                players.objects(),
                participant,
                game_mode,
                None,
            )?);
        }
        let mut records = vec![gm_record];
        records.extend(players.spawn_entities(creations, content.creation_content())?);
        let intro = content.sequence_creation(
            names::INTRO_SEQUENCE,
            players.objects(),
            participant,
            game_mode,
        )?;
        let intro = players
            .spawn_entities(vec![intro], content.creation_content())?
            .remove(0);
        let entry = state
            .entries
            .get_mut(&participant)
            .ok_or(replication::Error::Shape)?;
        let gone = std::mem::take(&mut entry.area);
        players.remove_entities(&gone)?;
        let mut logics = BTreeMap::new();
        logics.insert(Logic::Sound, records[1].id);
        logics.insert(Logic::Intro, records[2].id);
        entry.race = Some(Race {
            game_mode,
            car,
            phase: Phase::Intro,
            logics,
            scene: Vec::new(),
            sequence: Some((Sequence::Intro, intro.id, false)),
            start: None,
            next: 0,
            lap: 1,
            progress: None,
            deadline: None,
            finish: None,
            exit: None,
            awaiting_leave: false,
            goals: Vec::new(),
            flags: true,
            ai,
        });
        records.push(intro);
        event_flags(content, players, state, participant, 0, now_ms, out)?;
        suspended.insert(participant);
        state
            .steps
            .push_back((now_ms + FOLLOW_MS, participant, Step::Started));
        state
            .steps
            .push_back((now_ms + INTRO_FALLBACK_MS, participant, Step::IntroFallback));
        tracing::info!(participant, game_mode, car, deleted = ?gone, "owned event started; intro sequence created");
        out.records.extend(records);
        out.deleted.extend(gone);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn event_step(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        step: Step,
        now_ms: u64,
        now: f64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let event = content.event();
        if let Step::Flag { method, index } = step {
            out.rpcs.push(HostRpc::GameMode(content.flag_call(
                players.objects(),
                index,
                method,
            )?));
            return Ok(());
        }
        let Some(race) = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
        else {
            return Ok(());
        };
        let game_mode = race.game_mode;
        if players.objects().get(game_mode).is_none() {
            return Ok(());
        }
        let world_call = |players: &Players,
                          state: Gm,
                          method: u32,
                          reference: Option<u16>,
                          arguments: Arguments| {
            content.state_call(
                players.objects(),
                game_mode,
                state,
                method,
                reference,
                arguments,
            )
        };
        match step {
            Step::Started => {
                let mut creations = Vec::new();
                for name in [names::STARTING, names::LOGIC_TELEMETRY] {
                    let layout = content.logic(Owner::Scene, name, 0)?;
                    creations.push(content.area_logic(layout, players.objects(), participant)?);
                }
                creations.push(content.scene_logic_creation(
                    names::IN_GAME_MODE,
                    players.objects(),
                    participant,
                )?);
                let scene = players.spawn_entities(creations, content.creation_content())?;
                let mut gm_creations = Vec::new();
                for logic in [Logic::Telemetry, Logic::Restart, Logic::Leave] {
                    let (name, occurrence) = logic.name();
                    let layout = content.logic(Owner::GameMode, name, occurrence)?;
                    gm_creations.push(content.game_mode_logic(
                        layout,
                        players.objects(),
                        participant,
                        game_mode,
                        None,
                    )?);
                }
                let gm_logics = players.spawn_entities(gm_creations, content.creation_content())?;
                for (logic, record) in [Logic::Telemetry, Logic::Restart, Logic::Leave]
                    .into_iter()
                    .zip(&gm_logics)
                {
                    race.logics.insert(logic, record.id);
                }
                race.scene = scene.iter().map(|r| r.id).collect();
                out.records.extend(scene);
                out.records.extend(gm_logics);
                // The launcher learns its game mode; the route's checkpoints enable.
                out.rpcs.push(HostRpc::GameMode(event_logic::call(
                    players.objects(),
                    players
                        .objects()
                        .scene(event.scene_content_key)
                        .ok_or(replication::Error::UnknownObject)?,
                    players
                        .objects()
                        .scene_endpoint(event.scene_content_key, event.launcher)
                        .ok_or(replication::Error::UnknownObject)?
                        .1,
                    4,
                    Some(game_mode),
                    Arguments::None,
                )?));
                for &(_, serializer) in &event.game_mode.checkpoints {
                    out.rpcs.push(HostRpc::GameMode(content.scene_call(
                        players.objects(),
                        serializer,
                        0,
                        None,
                    )?));
                }
                // The intro drives the car to its waypoint under AI control (E997).
                if race.phase == Phase::Intro
                    && let Some(goals) = content.intro_goals()?
                {
                    match players.start_controlled_goals(
                        HOST_SELECTOR as u8,
                        self.persona,
                        participant,
                        goals,
                    ) {
                        Ok(records) => {
                            race.goals =
                                records[..records.len() - 1].iter().map(|r| r.id).collect();
                            tracing::info!(participant, goals = ?race.goals, "owned event intro goals assigned; the Actor drives to the waypoint");
                            out.records.extend(records);
                        }
                        Err(error) => {
                            tracing::warn!(
                                participant,
                                ?error,
                                "owned event intro goals not assigned"
                            )
                        }
                    }
                }
                self.event_ai_intro(content, players, race, out)?;
            }
            Step::Counting => {
                let mut creations = Vec::new();
                for logic in [Logic::AddSet(0), Logic::OngoingScore, Logic::ResultTracking] {
                    let (name, occurrence) = logic.name();
                    let layout = content.logic(Owner::GameMode, name, occurrence)?;
                    creations.push(content.game_mode_logic(
                        layout,
                        players.objects(),
                        participant,
                        game_mode,
                        None,
                    )?);
                }
                let records = players.spawn_entities(creations, content.creation_content())?;
                for (logic, record) in
                    [Logic::AddSet(0), Logic::OngoingScore, Logic::ResultTracking]
                        .into_iter()
                        .zip(&records)
                {
                    race.logics.insert(logic, record.id);
                }
                out.records.extend(records);
                // The countdown sequence is stopped at once (E994: 98 ms after it).
                if let Some((Sequence::Countdown, ghost, stopping)) = race.sequence.as_mut() {
                    *stopping = true;
                    out.rpcs.push(HostRpc::SequenceStop(Stop {
                        sequence: *ghost,
                        serial: Serial::new(1).ok_or(replication::Error::Shape)?,
                    }));
                }
            }
            Step::Racing => {
                let layout = content.logic(Owner::GameMode, names::DRIVING_STYLE, 0)?;
                let creation = content.game_mode_logic(
                    layout,
                    players.objects(),
                    participant,
                    game_mode,
                    None,
                )?;
                let record = players
                    .spawn_entities(vec![creation], content.creation_content())?
                    .remove(0);
                race.logics.insert(Logic::DrivingStyle, record.id);
                out.records.push(record);
                let deadline = now + f64::from(event.dnf_timeout);
                race.deadline = Some(deadline);
                race.phase = Phase::Racing;
                race.progress =
                    Some(nfs_world::race_progress::Race::new(
                        event.game_mode.checkpoints.len(),
                        event.game_mode.laps,
                        &std::iter::once(participant)
                            .chain(race.ai.iter().flat_map(|grid| {
                                grid.racers.members.iter().map(|m| m.ids.participant)
                            }))
                            .collect::<Vec<_>>(),
                        race.start.ok_or(replication::Error::Shape)?,
                    )?);
                self.event_ai_start(content, players, participant, race, now_ms, out)?;
                let p = Some(participant);
                let mut calls = vec![
                    (Gm::Countdown, 1, p, Arguments::None),
                    (Gm::Waypoints, 0, p, Arguments::None),
                ];
                match self.event_car_id(race.car) {
                    Some(car) => calls.push((Gm::Waypoints, 3, p, Arguments::Word(car))),
                    None => tracing::warn!(
                        participant,
                        car = race.car,
                        "owned event car without a blueprint; car ID not sent"
                    ),
                }
                calls.extend([
                    (
                        Gm::Positions,
                        0,
                        p,
                        Arguments::EventPosition {
                            position: 0,
                            active: true,
                        },
                    ),
                    (Gm::LocalPosition, 0, None, Arguments::Word(0)),
                    (
                        Gm::Positions,
                        0,
                        p,
                        Arguments::EventPosition {
                            position: 0,
                            active: true,
                        },
                    ),
                    (Gm::Dnf, 0, p, Arguments::time(deadline)),
                ]);
                if race.ai.is_none() {
                    calls.extend([
                        (Gm::LocalPosition, 0, None, Arguments::Word(1)),
                        (
                            Gm::Positions,
                            0,
                            p,
                            Arguments::EventPosition {
                                position: 1,
                                active: true,
                            },
                        ),
                    ]);
                }
                for (state, method, reference, arguments) in calls {
                    out.rpcs.push(HostRpc::GameMode(world_call(
                        players, state, method, reference, arguments,
                    )?));
                }
                self.event_ai_racing_calls(content, players, race, deadline, out)?;
                tracing::info!(participant, game_mode, deadline, "owned event race running");
            }
            Step::Finished => {
                let outro = content.spawner(names::OUTRO_SEQUENCE)?;
                let p = Some(participant);
                out.rpcs.push(HostRpc::GameMode(content.field_call(
                    players.objects(),
                    game_mode,
                    outro,
                    0,
                    p,
                )?));
                for (state, method) in [(Gm::Waypoints, 1), (Gm::Finish, 0), (Gm::Finish, 1)] {
                    out.rpcs.push(HostRpc::GameMode(world_call(
                        players,
                        state,
                        method,
                        p,
                        Arguments::None,
                    )?));
                }
                let position = race.finish.map_or(1, |f| f.position) as u32;
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::Positions,
                    0,
                    p,
                    Arguments::EventPosition {
                        position,
                        active: false,
                    },
                )?));
            }
            Step::Ending => {
                let outro = content.spawner(names::OUTRO_SEQUENCE)?;
                let p = Some(participant);
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::Root,
                    3,
                    p,
                    Arguments::None,
                )?));
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::Final,
                    0,
                    p,
                    Arguments::None,
                )?));
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::ResultReporting,
                    0,
                    p,
                    Arguments::None,
                )?));
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::ResultReporting,
                    1,
                    p,
                    Arguments::None,
                )?));
                out.rpcs.push(HostRpc::GameMode(content.field_call(
                    players.objects(),
                    game_mode,
                    outro,
                    1,
                    p,
                )?));
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::Root,
                    1,
                    None,
                    Arguments::None,
                )?));
                race.awaiting_leave = true;
            }
            Step::Flag { .. } => {}
            Step::IntroFallback => {
                if let Some((Sequence::Intro, ghost, stopping)) = race.sequence.as_mut()
                    && !*stopping
                {
                    *stopping = true;
                    out.rpcs.push(HostRpc::SequenceStop(Stop {
                        sequence: *ghost,
                        serial: Serial::new(1).ok_or(replication::Error::Shape)?,
                    }));
                    tracing::info!(
                        participant,
                        sequence = *ghost,
                        "owned event intro not reported; Stop after the fallback"
                    );
                }
            }
            Step::Exiting => {
                let p = Some(participant);
                let exit = race.exit.ok_or(replication::Error::Shape)?;
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    if exit == Exit::TimedOut {
                        Gm::Dnf
                    } else {
                        Gm::DnfTriggered
                    },
                    u32::from(exit == Exit::TimedOut),
                    p,
                    Arguments::None,
                )?));
                match race.phase {
                    Phase::Racing => out.rpcs.push(HostRpc::GameMode(world_call(
                        players,
                        Gm::Waypoints,
                        1,
                        p,
                        Arguments::None,
                    )?)),
                    Phase::Countdown => out.rpcs.push(HostRpc::GameMode(world_call(
                        players,
                        Gm::Countdown,
                        1,
                        p,
                        Arguments::None,
                    )?)),
                    Phase::Intro => {
                        let intro = content.spawner(names::INTRO_SEQUENCE)?;
                        out.rpcs.push(HostRpc::GameMode(content.field_call(
                            players.objects(),
                            game_mode,
                            intro,
                            1,
                            p,
                        )?));
                    }
                    Phase::Outro => {}
                }
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::Positions,
                    0,
                    p,
                    Arguments::EventPosition {
                        position: 1,
                        active: false,
                    },
                )?));
                for (state, method) in [(Gm::DnfState, 0), (Gm::DnfState, 1), (Gm::Final, 0)] {
                    out.rpcs.push(HostRpc::GameMode(world_call(
                        players,
                        state,
                        method,
                        p,
                        Arguments::None,
                    )?));
                }
                if matches!(exit, Exit::Abandon | Exit::TimedOut) {
                    out.rpcs.push(HostRpc::GameMode(world_call(
                        players,
                        Gm::Root,
                        3,
                        p,
                        Arguments::None,
                    )?));
                }
                out.rpcs.push(HostRpc::GameMode(world_call(
                    players,
                    Gm::Root,
                    1,
                    None,
                    Arguments::None,
                )?));
                race.awaiting_leave = true;
            }
        }
        // The in-event flags clear in the frame of the root's release (E997).
        if matches!(step, Step::Ending | Step::Exiting) && race.flags {
            race.flags = false;
            event_flags(content, players, state, participant, 1, now_ms, out)?;
        }
        Ok(())
    }

    /// The participant's car ID: the hash of its equipped vehicle's blueprint path.
    fn event_car_id(&self, car: u16) -> Option<u32> {
        let item = self
            .players
            .vehicle_item_of(HOST_SELECTOR as u8, self.persona, car)?;
        let guid = self.inventory.as_ref()?.items.items.get(&item)?.definition;
        let definition = self.vehicles.as_ref()?.vehicles.definition(&guid)?;
        Some(event_logic::car_id(&definition.blueprint))
    }

    #[allow(clippy::too_many_arguments)]
    fn event_intro_done(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        intro: u16,
        now_ms: u64,
        now: f64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let race = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            .ok_or(replication::Error::Shape)?;
        let game_mode = race.game_mode;
        let mut gone = vec![intro];
        gone.extend(race.logics.remove(&Logic::Intro));
        players.remove_entities(&gone)?;
        let goals = std::mem::take(&mut race.goals);
        release_goals(players, self.persona, participant, goals, out);
        let countdown = content.sequence_creation(
            names::COUNTDOWN_SEQUENCE,
            players.objects(),
            participant,
            game_mode,
        )?;
        let record = players
            .spawn_entities(vec![countdown], content.creation_content())?
            .remove(0);
        race.sequence = Some((Sequence::Countdown, record.id, false));
        race.phase = Phase::Countdown;
        let start = now + COUNTDOWN_LEAD_S;
        race.start = Some(start);
        self.event_ai_countdown(content, players, race, out)?;
        let intro_state = content.spawner(names::INTRO_SEQUENCE)?;
        let world = players.objects();
        let p = Some(participant);
        out.rpcs.push(HostRpc::GameMode(content.state_call(
            world,
            game_mode,
            Gm::Countdown,
            0,
            p,
            Arguments::None,
        )?));
        out.rpcs.push(HostRpc::GameMode(content.field_call(
            world,
            game_mode,
            intro_state,
            1,
            p,
        )?));
        out.rpcs.push(HostRpc::GameMode(content.state_call(
            world,
            game_mode,
            Gm::Countdown,
            2,
            None,
            Arguments::time(start),
        )?));
        state
            .steps
            .push_back((now_ms + FOLLOW_MS, participant, Step::Counting));
        tracing::info!(
            participant,
            start,
            countdown = record.id,
            "owned event intro completed; countdown created"
        );
        out.records.push(record);
        // The calls name only the game mode and participant; the deletions ride
        // a later section than the calls on the deleted objects (none here).
        out.deleted.extend(gone);
        Ok(())
    }

    fn event_countdown_done(
        &self,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        countdown: u16,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let race = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            .ok_or(replication::Error::Shape)?;
        race.sequence = None;
        players.remove_entities(&[countdown])?;
        out.deleted.push(countdown);
        state
            .steps
            .push_back((now_ms + FOLLOW_MS, participant, Step::Racing));
        tracing::info!(participant, "owned event countdown completed");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn event_checkpoint(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        _suspended: &mut BTreeSet<u16>,
        participant: u16,
        crossing: Crossing,
        now_ms: u64,
        now: f64,
        out: &mut Out,
        completions: &mut Vec<AwardRequest>,
    ) -> Result<(), replication::Error> {
        let event = content.event();
        let route = &event.game_mode.checkpoints;
        let race = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            .ok_or(replication::Error::Shape)?;
        let racer = if crossing.vehicle == race.car {
            participant
        } else {
            race.ai
                .as_ref()
                .and_then(|grid| {
                    grid.racers
                        .members
                        .iter()
                        .find(|m| m.ids.vehicle == crossing.vehicle)
                })
                .map(|m| m.ids.participant)
                .ok_or(replication::Error::UnknownObject)?
        };
        let next = race
            .progress
            .as_ref()
            .and_then(|progress| progress.participant(racer))
            .ok_or(replication::Error::UnknownObject)?
            .next;
        let expected = route
            .get(next)
            .and_then(|&(k, s)| players.objects().scene_endpoint(k, s))
            .map(|(_, s)| s);
        if expected != Some(crossing.selector) {
            return Ok(());
        }
        let Some(progress) = race
            .progress
            .as_mut()
            .ok_or(replication::Error::Shape)?
            .cross(racer, next, crossing.seconds)?
        else {
            return Ok(());
        };
        if let Some(commands) = race.ai.as_mut().and_then(|grid| grid.commands.as_mut()) {
            ai::append(out, commands.crossed(players, racer, progress)?);
        }
        let seconds = progress.seconds;
        let passed = progress.passed as u32;
        let next = progress.next;
        let lap = progress.lap;
        let deadline = now + f64::from(event.dnf_timeout);
        let world = players.objects();
        let game_mode = race.game_mode;
        let p = Some(racer);
        out.rpcs.push(HostRpc::GameMode(content.state_call(
            world,
            game_mode,
            Gm::Waypoints,
            2,
            p,
            Arguments::Progress {
                passed,
                next: next as u32,
                seconds: seconds.to_bits(),
                lap: u32::from(lap),
            },
        )?));
        out.rpcs.push(HostRpc::GameMode(content.state_call(
            world,
            game_mode,
            Gm::Dnf,
            0,
            p,
            Arguments::time(deadline),
        )?));
        if racer == participant {
            race.next = next;
            race.lap = lap;
            race.deadline = Some(deadline);
        } else if let Some(grid) = &mut race.ai {
            grid.deadlines.insert(racer, deadline);
        }
        tracing::info!(
            participant,
            passed,
            next,
            lap,
            seconds,
            "owned event checkpoint passed"
        );
        let Some(position) = progress.finish else {
            return Ok(());
        };
        if racer != participant {
            return self.event_ai_finish(content, players, race, racer, position, now_ms, out);
        }
        // Finish: the results logics and the outro sequence replace the race logics.
        let finish = Finish {
            race_ms: progress.milliseconds()?,
            position: i32::from(position),
            // The progress roster retains finished and withdrawn members.
            participants: race
                .progress
                .as_ref()
                .ok_or(replication::Error::Shape)?
                .participants()
                .len(),
        };
        race.finish = Some(finish);
        race.phase = Phase::Outro;
        race.deadline = None;
        // The category totals the race's driving-style session wrote.
        let properties = self.event_scoring_properties(participant);
        let snapshots: Vec<_> = race
            .progress
            .as_ref()
            .ok_or(replication::Error::Shape)?
            .participants()
            .iter()
            .map(|p| (u64::from(p.id), self.event_scoring_properties(p.id)))
            .collect();
        let awards = content.evaluate_awards(participant, &snapshots)?;
        tracing::info!(
            participant,
            ?properties,
            ?awards,
            "owned event participant properties"
        );
        let mut creations = Vec::new();
        for logic in [Logic::AddSet(1), Logic::Outro, Logic::Awards] {
            let (name, occurrence) = logic.name();
            let layout = content.logic(Owner::GameMode, name, occurrence)?;
            creations.push(content.game_mode_logic(
                layout,
                players.objects(),
                participant,
                game_mode,
                Some(event_logic::Results {
                    finish,
                    properties: &properties,
                    awards: awards.as_ref(),
                }),
            )?);
        }
        let mut records = players.spawn_entities(creations, content.creation_content())?;
        for (logic, record) in [Logic::AddSet(1), Logic::Outro, Logic::Awards]
            .into_iter()
            .zip(&records)
        {
            race.logics.insert(logic, record.id);
        }
        let outro = content.sequence_creation(
            names::OUTRO_SEQUENCE,
            players.objects(),
            participant,
            game_mode,
        )?;
        let outro = players
            .spawn_entities(vec![outro], content.creation_content())?
            .remove(0);
        race.sequence = Some((Sequence::Outro, outro.id, false));
        records.push(outro);
        let mut gone = Vec::new();
        for logic in [
            Logic::Restart,
            Logic::Leave,
            Logic::AddSet(0),
            Logic::OngoingScore,
            Logic::ResultTracking,
            Logic::DrivingStyle,
        ] {
            gone.extend(race.logics.remove(&logic));
        }
        players.remove_entities(&gone)?;
        state
            .steps
            .push_back((now_ms + FINISH_FOLLOW_MS, participant, Step::Finished));
        state.completed = state
            .completed
            .checked_add(1)
            .filter(|n| *n < 1 << 19)
            .ok_or(replication::Error::Bound)?;
        match self.event_completion(
            content,
            participant,
            state.completed,
            finish,
            awards.as_ref().map_or(0, |a| a.rep),
        )? {
            Some(request) => completions.push(request),
            None => tracing::warn!(
                participant,
                "owned event completion without reputation thresholds; rewards dropped"
            ),
        }
        tracing::info!(
            participant,
            race_ms = finish.race_ms,
            "owned event finished; results and rewards queued"
        );
        out.records.extend(records);
        out.deleted.extend(gone);
        Ok(())
    }

    /// The completion's durable rewards as one credit. Its batch ID takes the
    /// session's credit base with bit 19 set and the session's completion ordinal,
    /// apart from the driving credits' sequence.
    fn event_completion(
        &self,
        content: &event_logic::Content,
        participant: u16,
        ordinal: u64,
        finish: Finish,
        award_rep: i32,
    ) -> Result<Option<AwardRequest>, replication::Error> {
        let rewards = content.rewards_for_finish(finish)?;
        let rep = rewards
            .rep
            .checked_add(award_rep)
            .filter(|_| award_rep >= 0)
            .ok_or(replication::Error::Bound)?;
        let Some(thresholds) = self
            .logic
            .as_deref()
            .and_then(|l| l.reputation_thresholds())
            .cloned()
        else {
            return Ok(None);
        };
        let batch_id = self.scoring_state.batch_base | (1 << 19) | ordinal;
        Ok(Some(AwardRequest {
            participant,
            credit: nfs_services::reputation::Scores {
                total: rep,
                build: 0,
                crew: 0,
                outlaw: 0,
                speed: 0,
                style: 0,
            },
            batch_id,
            thresholds,
            player_index: self
                .players
                .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
                .map(u32::from),
            event: Some(nfs_services::event_rewards::Completion {
                event_id: content.record_id(),
                kind: content.category().to_owned(),
                position: u32::try_from(finish.position - 1)
                    .map_err(|_| replication::Error::Bound)?,
                cash: u32::try_from(rewards.cash).map_err(|_| replication::Error::Bound)?,
                rep: nfs_services::reputation::Scores {
                    total: rep,
                    build: 0,
                    crew: 0,
                    outlaw: 0,
                    speed: 0,
                    style: 0,
                },
                batch_id,
            }),
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn event_outro_done(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        suspended: &mut BTreeSet<u16>,
        participant: u16,
        outro: u16,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let race = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            .ok_or(replication::Error::Shape)?;
        race.sequence = None;
        race.exit = Some(Exit::Completed);
        players.remove_entities(&[outro])?;
        out.deleted.push(outro);
        let cooldown =
            content.scene_logic_creation(names::COOLDOWN, players.objects(), participant)?;
        let record = players
            .spawn_entities(vec![cooldown], content.creation_content())?
            .remove(0);
        state.cooldowns.push_back((now_ms + COOLDOWN_MS, record.id));
        out.records.push(record);
        suspended.remove(&participant);
        state
            .steps
            .push_back((now_ms + FINISH_FOLLOW_MS, participant, Step::Ending));
        tracing::info!(
            participant,
            "owned event outro completed; free roam resumes"
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn event_exit(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        suspended: &mut BTreeSet<u16>,
        participant: u16,
        exit: Exit,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let race = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            .ok_or(replication::Error::Shape)?;
        race.exit = Some(exit);
        race.deadline = None;
        let first_record = out.records.len();
        let first_deleted = out.deleted.len();
        let presentation = if exit == Exit::Restart {
            // Queue the authored relocation presentation before releasing race
            // HUD state. The relocation owns it until the arrival sequence.
            Some(restart::presentation(
                self,
                crate::garage_logic::Which::InWorldTeleport,
                players,
                participant,
                out,
            )?)
        } else {
            None
        };
        self.event_ai_stop(players, race, out)?;
        // The participant's event logics go at once; the game mode stays until the
        // client leaves it (E994: Events, sound, telemetry, restart, leave and
        // racing logics deleted at the choice).
        let mut gone: Vec<u16> = race.logics.values().copied().collect();
        race.logics.clear();
        if let Some(&in_game_mode) = race.scene.get(2) {
            gone.push(in_game_mode);
            race.scene.truncate(2);
        }
        if let Some((_, ghost, _)) = race.sequence.take() {
            gone.push(ghost);
        }
        gone.retain(|g| players.objects().get(*g).is_some());
        players.remove_entities(&gone)?;
        out.deleted.extend(gone);
        let goals = std::mem::take(&mut race.goals);
        release_goals(players, self.persona, participant, goals, out);
        let cooldown =
            content.scene_logic_creation(names::COOLDOWN, players.objects(), participant)?;
        let record = players
            .spawn_entities(vec![cooldown], content.creation_content())?
            .remove(0);
        state.cooldowns.push_back((now_ms + COOLDOWN_MS, record.id));
        out.records.push(record);
        // Restart must not create a transient free-roam scoring session while
        // the old game mode is still leaving. Release it when relocation ends.
        if exit != Exit::Restart {
            suspended.remove(&participant);
        }
        // Pending steps go, except delayed flag calls (they keep their order).
        state
            .steps
            .retain(|(_, p, s)| *p != participant || matches!(s, Step::Flag { .. }));
        state
            .steps
            .push_back((now_ms + FOLLOW_MS, participant, Step::Exiting));
        tracing::info!(participant, ?exit, "owned event exit chosen");
        if let Some(mut presentation) = presentation {
            // Race HUD deletion and LoadingScreen activation belong to one
            // publication. Neither may escape first under queue pressure.
            presentation
                .records
                .extend(out.records.drain(first_record..));
            presentation
                .deleted
                .extend(out.deleted.drain(first_deleted..));
            out.entries.push((
                packed(presentation.records, presentation.deleted),
                presentation
                    .events
                    .into_iter()
                    .map(HostRpc::Event)
                    .collect(),
            ));
        }
        Ok(())
    }

    /// The client left the game mode: its final calls, the checkpoint disables and
    /// the deletion of the game mode with every remaining event object, in one frame.
    fn event_left(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let event = content.event();
        let entry = state
            .entries
            .get_mut(&participant)
            .ok_or(replication::Error::Shape)?;
        let mut race = entry.race.take().ok_or(replication::Error::Shape)?;
        if race.exit == Some(Exit::Restart) {
            if self.world_cars.get(&race.car) != Some(&(participant, true)) {
                return Err(replication::Error::UnknownObject);
            }
            if state.restarts.len() >= MAX_ENTRIES || state.restarts.contains_key(&participant) {
                return Err(replication::Error::Bound);
            }
            let destinations = content
                .event()
                .restart
                .as_ref()
                .ok_or(replication::Error::Unsupported)?;
            let destination = *destinations
                .get(state.restart_cursor)
                .ok_or(replication::Error::Shape)?;
            state.restart_cursor = (state.restart_cursor + 1) % destinations.len();
            state
                .restarts
                .insert(participant, restart::Instance::new(race.car, destination));
        }
        self.event_ai_stop(players, &mut race, out)?;
        release_goals(
            players,
            self.persona,
            participant,
            std::mem::take(&mut race.goals),
            out,
        );
        let world = players.objects();
        let game_mode = race.game_mode;
        let p = Some(participant);
        let call = |state: Gm, method: u32, reference: Option<u16>| {
            content.state_call(world, game_mode, state, method, reference, Arguments::None)
        };
        let mut calls = Vec::new();
        match race.exit {
            Some(Exit::Completed) | None => {
                calls.push(call(Gm::Root, 4, p)?);
                calls.push(call(Gm::Main, 0, p)?);
                calls.push(call(Gm::Final, 1, p)?);
            }
            Some(Exit::Restart | Exit::Abandon | Exit::TimedOut) => {
                calls.push(call(Gm::Root, 7, None)?);
                calls.push(call(Gm::Final, 1, p)?);
                calls.push(call(Gm::Main, 0, p)?);
                calls.push(call(Gm::Root, 4, p)?);
            }
        }
        for &(_, serializer) in &event.game_mode.checkpoints {
            calls.push(content.scene_call(world, serializer, 1, None)?);
        }
        let mut gone: Vec<u16> = race.logics.values().copied().collect();
        gone.extend(race.scene);
        gone.extend(race.sequence.map(|(_, g, _)| g));
        gone.push(game_mode);
        gone.retain(|g| players.objects().get(*g).is_some());
        players.remove_entities(&gone)?;
        // Calls on the deleted objects precede the deletion in the same frame,
        // fifteen notifications per frame at most.
        let mut calls: Vec<HostRpc> = calls.into_iter().map(HostRpc::GameMode).collect();
        let tail = calls.split_off(
            calls
                .len()
                .saturating_sub(nfs_world::session::MAX_EAGER_NOTIFICATIONS),
        );
        out.rpcs.extend(calls);
        out.entries.push((packed(Vec::new(), gone.clone()), tail));
        tracing::info!(participant, game_mode, deleted = ?gone, exit = ?race.exit, "owned event game mode left");
        Ok(())
    }
}

#[cfg(test)]
mod tests;
