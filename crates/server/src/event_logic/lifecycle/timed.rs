// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored countdown/DNF clocks and the separate finish receipt-time queue.
//! The edge schedules entity updates in its registration order. Setting clocks
//! does not itself run an update or complete a sequence.
use super::{Effect as HostEffect, Work as HostWork, *};

mod dispatch;

/// Clock/report/update calls participate in the enclosing graph transaction.
/// Supplying clocks alone never triggers an entity update.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Clocks(Clocks),
    Countdown(StateId),
    Transition(usize),
    Waypoint {
        source: StateId,
        participant: Participant,
        lap: u8,
        seconds: f32,
    },
    DnfEvent {
        transition: usize,
        event: u32,
        participant: Option<Participant>,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clocks {
    pub receipt_seconds: f64,
    pub channels: BTreeMap<Property, f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    CountdownDeadline {
        state: StateId,
        deadline: f64,
    },
    /// Stop the state's shared sequence before completing its membership.
    CountdownStop {
        state: StateId,
    },
    /// Native event after completion and deferred transitions have drained.
    CountdownEvent {
        state: StateId,
        event: u32,
    },
    DnfDeadline {
        transition: usize,
        source: StateId,
        participant: Participant,
        deadline: f64,
    },
    Dnf {
        transition: usize,
        source: StateId,
        participant: Participant,
        triggered: bool,
    },
    Finish {
        transition: usize,
        source: StateId,
        participant: Participant,
        seconds: f32,
        position: u32,
        remaining: Vec<Participant>,
    },
}

#[derive(Clone, Debug)]
pub(super) enum Work {
    Command(Command),
    UpdateTransition {
        id: usize,
        processed: usize,
    },
    Waypoint {
        source: StateId,
        participant: Participant,
        lap: u8,
        seconds: f32,
        observer: usize,
    },
    Dnf {
        transition: usize,
        token: u64,
    },
    DnfDone {
        transition: usize,
        token: u64,
    },
    FinishPublish {
        transition: usize,
        entry: FinishEntry,
        seconds: f32,
        position: u32,
    },
    CountdownEvent(StateId),
}

#[derive(Clone, Debug)]
struct DnfEntry {
    token: u64,
    source: StateId,
    participant: Participant,
    deadline: Option<f64>,
}
#[derive(Clone, Debug)]
pub(super) struct FinishEntry {
    source: StateId,
    participant: Participant,
    seconds: f32,
    received: f64,
}
#[derive(Clone, Debug, Default)]
struct FinishQueue {
    entries: Vec<FinishEntry>,
    position: u32,
    seconds: f32,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Timers {
    clocks: Option<Clocks>,
    countdowns: BTreeMap<StateId, f64>,
    dnf: BTreeMap<usize, Vec<DnfEntry>>,
    processing: BTreeSet<usize>,
    finishes: BTreeMap<usize, FinishQueue>,
    serial: u64,
}

fn time(value: f64) -> Result<f64, Error> {
    if value.is_finite() && value >= 0. {
        Ok(value)
    } else {
        Err(Error::Invalid)
    }
}
fn transition(plan: &Plan, id: usize) -> Result<&data::Transition, Error> {
    plan.data
        .transitions
        .iter()
        .find(|t| t.serializer == id)
        .ok_or(Error::Invalid)
}
impl Timers {
    fn owned_time(&self, plan: &Plan, id: usize) -> Result<f64, Error> {
        let key = plan.data.bindings.clocks.get(&id).ok_or(Error::Invalid)?;
        self.clocks
            .as_ref()
            .and_then(|c| c.channels.get(key))
            .copied()
            .ok_or(Error::Invalid)
    }
    fn receipt_time(&self) -> Result<f64, Error> {
        self.clocks
            .as_ref()
            .map(|c| c.receipt_seconds)
            .ok_or(Error::Invalid)
    }
    pub(super) fn enter(
        &mut self,
        plan: &Plan,
        state: &data::State,
        machine: &dyn nfs_world::participant_states::Membership,
    ) -> Result<Option<Effect>, Error> {
        let data::StateConfiguration::Countdown { seconds, .. } = state.configuration else {
            return Ok(None);
        };
        let id = state.serializer as StateId;
        if self.countdowns.contains_key(&id) || machine.members(id)?.is_empty() {
            return Ok(None);
        }
        let deadline = time(self.owned_time(plan, state.serializer)? + f64::from(seconds))?;
        self.countdowns.insert(id, deadline);
        Ok(Some(Effect::CountdownDeadline {
            state: id,
            deadline,
        }))
    }
    pub(super) fn observe(
        &mut self,
        plan: &Plan,
        transition: &data::Transition,
        event: Notification,
    ) -> Result<Vec<Effect>, Error> {
        if !matches!(
            transition.kind,
            data::TransitionKind::Dnf | data::TransitionKind::DnfTriggered
        ) {
            return Ok(vec![]);
        }
        let (source, participant, enter) = match event {
            Notification::Entered {
                state, participant, ..
            }
            | Notification::Existing { state, participant } => (state, participant, true),
            Notification::Leaving { state, participant } => (state, participant, false),
            _ => return Ok(vec![]),
        };
        if !transition
            .sources
            .iter()
            .any(|e| e.serializer == usize::from(source))
        {
            return Ok(vec![]);
        }
        if !enter {
            if !self.processing.contains(&transition.serializer)
                && let Some(entries) = self.dnf.get_mut(&transition.serializer)
            {
                entries.retain(|e| e.source != source || e.participant != participant);
            }
            return Ok(vec![]);
        }
        let deadline = match transition.configuration {
            data::TransitionConfiguration::Dnf { timeout_seconds } => Some(time(
                self.owned_time(plan, transition.serializer)? + f64::from(timeout_seconds),
            )?),
            _ if transition.kind == data::TransitionKind::DnfTriggered => None,
            _ => return Err(Error::Invalid),
        };
        if self.dnf.values().map(Vec::len).sum::<usize>() >= states::MAX_PENDING {
            return Err(Error::Bound);
        }
        self.serial = self.serial.checked_add(1).ok_or(Error::Bound)?;
        self.dnf
            .entry(transition.serializer)
            .or_default()
            .push(DnfEntry {
                token: self.serial,
                source,
                participant,
                deadline,
            });
        Ok(deadline
            .into_iter()
            .map(|deadline| Effect::DnfDeadline {
                transition: transition.serializer,
                source,
                participant,
                deadline,
            })
            .collect())
    }
}

impl Host {
    /// Clock ownership remains explicit: an authored GameTime may pause/reset,
    /// while the process receipt clock used for finish arbitration is monotonic.
    pub fn clocks(&mut self, clocks: Clocks) -> Result<(), Error> {
        self.observers.timed.set_clocks(clocks)
    }
    pub fn update_countdown(
        &mut self,
        state: StateId,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<HostEffect>, Error> {
        self.dispatch_timing(Command::Countdown(state), callback)
    }
    /// Notification from the actual waypoint state. The caller validates the
    /// route report; DNF refresh uses the owned clock, not the supplied score.
    pub fn waypoint(
        &mut self,
        source: StateId,
        participant: Participant,
        lap: u8,
        seconds: f32,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<HostEffect>, Error> {
        self.dispatch_timing(
            Command::Waypoint {
                source,
                participant,
                lap,
                seconds,
            },
            callback,
        )
    }
    pub fn update_transition(
        &mut self,
        id: usize,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<HostEffect>, Error> {
        self.dispatch_timing(Command::Transition(id), callback)
    }
    /// The native DNF-triggered transition accepts only a participant-context
    /// Trigger event. A repeated trigger sees no old source membership.
    pub fn dnf_event(
        &mut self,
        id: usize,
        event: u32,
        participant: Option<Participant>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<HostEffect>, Error> {
        self.dispatch_timing(
            Command::DnfEvent {
                transition: id,
                event,
                participant,
            },
            callback,
        )
    }
}
impl Observers {
    pub(super) fn timing_work(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<HostEffect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        match work {
            Work::Command(command) => {
                self.timing_command(plan, command, machine, effects, callback)
            }
            Work::UpdateTransition { id, processed } => self.timing_transition(plan, id, processed),
            Work::Waypoint {
                source,
                participant,
                lap,
                seconds,
                observer,
            } => self.timing_waypoint(
                plan,
                Work::Waypoint {
                    source,
                    participant,
                    lap,
                    seconds,
                    observer,
                },
                machine,
                effects,
                callback,
            ),
            Work::CountdownEvent(state) => emit(
                machine,
                HostEffect::Timing(Effect::CountdownEvent {
                    state,
                    event: 0x81fb_265b,
                }),
                effects,
                callback,
            ),
            Work::FinishPublish {
                transition,
                entry,
                seconds,
                position,
            } => emit(
                machine,
                HostEffect::Timing(Effect::Finish {
                    transition,
                    source: entry.source,
                    participant: entry.participant,
                    seconds,
                    position,
                    remaining: machine.members(entry.source)?.iter().copied().collect(),
                }),
                effects,
                callback,
            ),
            Work::Dnf {
                transition: id,
                token,
            } => {
                let Some(entry) = self
                    .timed
                    .dnf
                    .get(&id)
                    .and_then(|entries| entries.iter().find(|e| e.token == token))
                    .cloned()
                else {
                    return Ok(vec![]);
                };
                if !self.timed.processing.insert(id) {
                    return Err(Error::Invalid);
                }
                let t = transition(plan, id)?;
                let mut actions = emit(
                    machine,
                    HostEffect::Timing(Effect::Dnf {
                        transition: id,
                        source: entry.source,
                        participant: entry.participant,
                        triggered: t.kind == data::TransitionKind::DnfTriggered,
                    }),
                    effects,
                    callback,
                )?;
                actions.push(Action::Move {
                    source: entry.source,
                    participant: entry.participant,
                    target: t.target.map(|t| t.serializer as StateId),
                });
                actions.extend(self.enqueue([HostWork::Timing(Work::DnfDone {
                    transition: id,
                    token,
                })])?);
                Ok(actions)
            }
            Work::DnfDone { transition, token } => {
                self.timed.processing.remove(&transition);
                if let Some(entries) = self.timed.dnf.get_mut(&transition) {
                    entries.retain(|e| e.token != token);
                }
                Ok(vec![])
            }
        }
    }
}
