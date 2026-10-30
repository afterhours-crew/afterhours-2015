// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Suspend at native observer boundaries without retaining a mutable borrow
//! across a graph callback. The caller commits only a completed execution.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Call {
    pub notification: Notification,
    pub observer: usize,
}

#[derive(Clone, Debug)]
enum Frame {
    Apply(Action),
    EndAction,
    Emit(StateId, Notification),
    Notify(Notification, usize),
    Unguard(StateId),
    Drain(StateId),
    DrainNext(StateId),
    Remove(StateId, Participant, Exit),
    RemoveNext(StateId),
    CheckDeleted(StateId, Participant),
    CompleteMember(StateId, Participant, bool),
    Seed(StateId, Participant, usize),
    MoveSource(StateId, Option<StateId>),
}

#[derive(Clone, Copy, Debug)]
enum Exit {
    Direct,
    Transition(Option<StateId>),
}

/// An isolated, bounded transaction. Dropping it discards all membership,
/// queued transitions and observer effects. A yielded call must be resumed
/// exactly once, even when it returns no actions.
#[derive(Clone, Debug)]
pub struct Execution {
    machine: Machine,
    stack: Vec<Frame>,
    output: Vec<Notification>,
    observer_count: usize,
    broadcast_count: usize,
    steps: usize,
    depth: usize,
    waiting: bool,
    fault: Option<Error>,
}

impl Machine {
    pub fn begin(&self, actions: &[Action], observer_count: usize) -> Result<Execution, Error> {
        if actions.len() > MAX_PENDING || observer_count > MAX_PENDING {
            return Err(Error::Bound);
        }
        Ok(Execution {
            machine: self.clone(),
            stack: actions.iter().rev().cloned().map(Frame::Apply).collect(),
            output: vec![],
            observer_count,
            broadcast_count: observer_count,
            steps: 0,
            depth: 0,
            waiting: false,
            fault: None,
        })
    }

    /// The host owns live per-state subscriber vectors. Ordinary notifications
    /// go to its dispatcher (zero); activation seeding may address a particular
    /// registered observer directly without broadcasting to all slots.
    pub fn begin_routed(
        &self,
        actions: &[Action],
        observer_count: usize,
    ) -> Result<Execution, Error> {
        if observer_count == 0 {
            return Err(Error::Invalid);
        }
        let mut execution = self.begin(actions, observer_count)?;
        execution.broadcast_count = 1;
        Ok(execution)
    }
}

impl Execution {
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// Continue until the next observer call or completion. Errors poison the
    /// transaction; the partial candidate can never be committed afterwards.
    pub fn advance(&mut self) -> Result<Option<Call>, Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = self.advance_inner();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    pub fn resume(&mut self, actions: Vec<Action>) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if !self.waiting {
                return Err(Error::Invalid);
            }
            if actions.len() > MAX_PENDING {
                return Err(Error::Bound);
            }
            self.push(actions.into_iter().map(Frame::Apply).collect())?;
            self.waiting = false;
            Ok(())
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    /// Execute a nested call while the current observer remains on the stack.
    /// Yield the supplied continuation after its actions finish, before any
    /// sibling observer. This does not add a state callback or an RPC.
    pub fn interject(&mut self, actions: Vec<Action>, continuation: Call) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if !self.waiting || continuation.observer >= self.observer_count {
                return Err(Error::Invalid);
            }
            if actions.len() > MAX_PENDING {
                return Err(Error::Bound);
            }
            let mut frames = actions.into_iter().map(Frame::Apply).collect::<Vec<_>>();
            frames.push(Frame::Notify(
                continuation.notification,
                continuation.observer,
            ));
            self.push(frames)?;
            self.waiting = false;
            Ok(())
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    pub fn finish(self) -> Result<(Machine, Vec<Notification>), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        if self.waiting
            || !self.stack.is_empty()
            || self.depth != 0
            || self
                .machine
                .states
                .values()
                .any(|s| s.callbacks != 0 || s.draining || !s.pending.is_empty())
        {
            return Err(Error::Invalid);
        }
        Ok((self.machine, self.output))
    }

    fn push(&mut self, frames: Vec<Frame>) -> Result<(), Error> {
        if self.stack.len().saturating_add(frames.len()) > MAX_STEPS {
            return Err(Error::Bound);
        }
        self.stack.extend(frames.into_iter().rev());
        Ok(())
    }
    fn step(&mut self) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > MAX_STEPS || self.depth >= MAX_DEPTH {
            Err(Error::Bound)
        } else {
            Ok(())
        }
    }
    fn record(&mut self, notification: Notification) -> Result<(), Error> {
        if self.output.len() >= MAX_STEPS {
            return Err(Error::Bound);
        }
        self.output.push(notification);
        Ok(())
    }

    fn advance_inner(&mut self) -> Result<Option<Call>, Error> {
        if self.waiting {
            return Err(Error::Invalid);
        }
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Apply(action) => {
                    self.step()?;
                    self.depth += 1;
                    self.push(vec![Frame::EndAction])?;
                    self.apply(action)?;
                }
                Frame::EndAction => self.depth = self.depth.checked_sub(1).ok_or(Error::Invalid)?,
                Frame::Emit(state, notification) => {
                    self.step()?;
                    self.record(notification)?;
                    self.machine.state_mut(state)?.callbacks += 1;
                    let mut frames = (0..self.broadcast_count)
                        .map(|i| Frame::Notify(notification, i))
                        .collect::<Vec<_>>();
                    frames.push(Frame::Unguard(state));
                    self.push(frames)?;
                }
                Frame::Notify(notification, observer) => {
                    self.step()?;
                    if observer >= self.observer_count {
                        return Err(Error::Invalid);
                    }
                    self.waiting = true;
                    return Ok(Some(Call {
                        notification,
                        observer,
                    }));
                }
                Frame::Unguard(state) => {
                    let s = self.machine.state_mut(state)?;
                    s.callbacks = s.callbacks.checked_sub(1).ok_or(Error::Invalid)?;
                }
                Frame::Drain(state) => {
                    let s = self.machine.state_mut(state)?;
                    if s.callbacks == 0 && !s.draining {
                        s.draining = true;
                        self.push(vec![Frame::DrainNext(state)])?;
                    }
                }
                Frame::DrainNext(source) => {
                    if let Some((participant, target)) =
                        self.machine.state_mut(source)?.pending.pop()
                    {
                        self.step()?;
                        let mut frames = Vec::new();
                        if self.machine.state(source)?.members.contains(&participant) {
                            frames.push(Frame::Emit(
                                source,
                                Notification::Leaving {
                                    state: source,
                                    participant,
                                },
                            ));
                            frames.push(Frame::Remove(
                                source,
                                participant,
                                Exit::Transition(target),
                            ));
                        }
                        frames.push(Frame::DrainNext(source));
                        self.push(frames)?;
                    } else {
                        self.machine.state_mut(source)?.draining = false;
                    }
                }
                Frame::Remove(source, participant, exit) => {
                    let s = self.machine.state_mut(source)?;
                    s.members.remove(&participant);
                    s.completed.remove(&participant);
                    let send_rpc = s.definition.send_rpc_to_client;
                    let mut frames = vec![Frame::Emit(
                        source,
                        Notification::Left {
                            state: source,
                            participant,
                            send_rpc,
                        },
                    )];
                    if let Exit::Transition(target) = exit {
                        frames.push(Frame::Emit(
                            source,
                            Notification::Transitioned {
                                source,
                                participant,
                                target,
                            },
                        ));
                        if let Some(state) = target {
                            frames.push(Frame::Apply(Action::Enter { state, participant }));
                        }
                    }
                    self.push(frames)?;
                }
                Frame::RemoveNext(state) => {
                    self.step()?;
                    if let Some(&participant) = self.machine.state(state)?.members.first() {
                        self.push(vec![
                            Frame::Apply(Action::Remove { state, participant }),
                            Frame::RemoveNext(state),
                        ])?;
                    }
                }
                Frame::CheckDeleted(state, participant) => {
                    let s = self.machine.state(state)?;
                    if self.machine.marked.contains(&participant)
                        && !s.definition.handles_marked_for_delete
                        && s.members.contains(&participant)
                        && !s.completed.contains(&participant)
                    {
                        self.push(vec![Frame::Apply(Action::Complete { state, participant })])?;
                    }
                }
                Frame::CompleteMember(state, participant, only_incomplete) => {
                    if !only_incomplete
                        || !self.machine.state(state)?.completed.contains(&participant)
                    {
                        self.push(vec![Frame::Apply(Action::Complete { state, participant })])?;
                    }
                }
                Frame::Seed(state, participant, observer) => {
                    let notification = Notification::Existing { state, participant };
                    self.record(notification)?;
                    self.push(vec![Frame::Notify(notification, observer)])?;
                }
                Frame::MoveSource(source, target) => {
                    let s = self.machine.state_mut(source)?;
                    s.callbacks += 1;
                    let mut frames = s
                        .members
                        .iter()
                        .map(|&participant| {
                            Frame::Apply(Action::Move {
                                source,
                                participant,
                                target,
                            })
                        })
                        .collect::<Vec<_>>();
                    frames.extend([Frame::Unguard(source), Frame::Drain(source)]);
                    self.push(frames)?;
                }
            }
        }
        Ok(None)
    }

    fn apply(&mut self, action: Action) -> Result<(), Error> {
        let frames = match action {
            Action::Invoke { observer, token } => {
                vec![Frame::Notify(Notification::Invoked { token }, observer)]
            }
            Action::Enter { state, participant } => {
                let s = self.machine.state_mut(state)?;
                if !s.members.contains(&participant) && s.members.len() >= MAX_PARTICIPANTS {
                    return Err(Error::Bound);
                }
                s.members.insert(participant);
                vec![
                    Frame::Emit(
                        state,
                        Notification::Entered {
                            state,
                            participant,
                            send_rpc: s.definition.send_rpc_to_client,
                        },
                    ),
                    Frame::Drain(state),
                    Frame::CheckDeleted(state, participant),
                ]
            }
            Action::Complete { state, participant } => {
                let s = self.machine.state_mut(state)?;
                if s.members.contains(&participant) {
                    s.completed.insert(participant);
                    vec![
                        Frame::Emit(state, Notification::Completed { state, participant }),
                        Frame::Drain(state),
                    ]
                } else {
                    vec![]
                }
            }
            Action::CompleteAll {
                state,
                only_incomplete,
            } => {
                let s = self.machine.state_mut(state)?;
                s.callbacks += 1;
                let mut frames = s
                    .members
                    .iter()
                    .map(|&p| Frame::CompleteMember(state, p, only_incomplete))
                    .collect::<Vec<_>>();
                frames.extend([Frame::Unguard(state), Frame::Drain(state)]);
                frames
            }
            Action::SeedMembers { state, observer } => {
                if observer >= self.observer_count {
                    return Err(Error::Invalid);
                }
                let s = self.machine.state_mut(state)?;
                s.callbacks += 1;
                let mut frames = s
                    .members
                    .iter()
                    .map(|&p| Frame::Seed(state, p, observer))
                    .collect::<Vec<_>>();
                frames.extend([Frame::Unguard(state), Frame::Drain(state)]);
                frames
            }
            Action::Move {
                source,
                participant,
                target,
            } => {
                if let Some(target) = target {
                    self.machine.state(target)?;
                }
                if self
                    .machine
                    .states
                    .values()
                    .map(|s| s.pending.len())
                    .sum::<usize>()
                    >= MAX_PENDING
                {
                    return Err(Error::Bound);
                }
                self.machine
                    .state_mut(source)?
                    .pending
                    .push((participant, target));
                vec![Frame::Drain(source)]
            }
            Action::MoveFirstCompleted {
                sources,
                participant,
                target,
            } => {
                self.validate_sources(&sources, target)?;
                sources
                    .into_iter()
                    .find(|s| self.machine.states[s].completed.contains(&participant))
                    .map(|source| {
                        vec![Frame::Apply(Action::Move {
                            source,
                            participant,
                            target,
                        })]
                    })
                    .unwrap_or_default()
            }
            Action::MoveFirstMember {
                sources,
                participant,
                target,
            } => {
                self.validate_sources(&sources, target)?;
                sources
                    .into_iter()
                    .find(|s| self.machine.states[s].members.contains(&participant))
                    .map(|source| {
                        vec![Frame::Apply(Action::Move {
                            source,
                            participant,
                            target,
                        })]
                    })
                    .unwrap_or_default()
            }
            Action::MoveAll { sources, target } => {
                self.validate_sources(&sources, target)?;
                sources
                    .into_iter()
                    .map(|s| Frame::MoveSource(s, target))
                    .collect()
            }
            Action::Remove { state, participant } => {
                if self.machine.state(state)?.members.contains(&participant) {
                    vec![
                        Frame::Emit(state, Notification::Leaving { state, participant }),
                        Frame::Remove(state, participant, Exit::Direct),
                        Frame::Drain(state),
                    ]
                } else {
                    vec![]
                }
            }
            Action::RemoveAll { state } => {
                self.machine.state(state)?;
                vec![Frame::RemoveNext(state)]
            }
            Action::MarkForDelete(participant) => {
                if !self.machine.marked.contains(&participant)
                    && self.machine.marked.len() >= MAX_PARTICIPANTS
                {
                    return Err(Error::Bound);
                }
                self.machine.marked.insert(participant);
                self.machine
                    .states
                    .keys()
                    .map(|&s| Frame::CheckDeleted(s, participant))
                    .collect()
            }
        };
        self.push(frames)
    }
    fn validate_sources(&self, sources: &[StateId], target: Option<StateId>) -> Result<(), Error> {
        if sources.len() > MAX_STATES {
            return Err(Error::Bound);
        }
        if let Some(target) = target {
            self.machine.state(target)?;
        }
        for &source in sources {
            self.machine.state(source)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
