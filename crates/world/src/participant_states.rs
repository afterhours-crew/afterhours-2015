// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-state membership and deferred transitions. Completion is a
//! separate set from membership (0402c800/0402abc0). Transition requests are
//! drained last-in-first-out per source after its observer callbacks return
//! (0402c730/0402c090); requests for participants already removed are ignored.
//! Specialized countdown, waypoint and sequence conditions belong to the host.
use std::collections::{BTreeMap, BTreeSet};

pub mod contexts;
pub mod execution;
pub mod properties;

pub type StateId = u16;
pub type Participant = u64;
pub const MAX_STATES: usize = 256;
/// Aggregate bound for the session's scene and live mode states. Individual
/// authored plans retain their smaller MAX_STATES validation bound.
pub const MAX_WORLD_STATES: usize = 4096;
pub const MAX_PARTICIPANTS: usize = 128;
pub const MAX_PENDING: usize = 4096;
pub const MAX_STEPS: usize = 65_536;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Bound,
    UnknownState,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub id: StateId,
    pub handles_marked_for_delete: bool,
    pub send_rpc_to_client: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Resume a host-owned synchronous listener. The token has meaning only
    /// within the caller's candidate transaction. Its returned actions finish
    /// before the next listener, retaining any enclosing state's guard.
    Invoke {
        observer: usize,
        token: u64,
    },
    Enter {
        state: StateId,
        participant: Participant,
    },
    Complete {
        state: StateId,
        participant: Participant,
    },
    /// Countdown completes every member (047647a0); updateable-state updates
    /// skip members already completed (0465d040). Both defer moves until the
    /// complete iteration has finished.
    CompleteAll {
        state: StateId,
        only_incomplete: bool,
    },
    /// Activation-time enumeration for one registered host observer. This is
    /// not entry: it performs no RPC, entry hook or membership insertion.
    SeedMembers {
        state: StateId,
        observer: usize,
    },
    Move {
        source: StateId,
        participant: Participant,
        target: Option<StateId>,
    },
    /// ParticipantLogicParticipantTransition checks completed membership in
    /// authored source order and moves from the first match (0473a180).
    MoveFirstCompleted {
        sources: Vec<StateId>,
        participant: Participant,
        target: Option<StateId>,
    },
    /// Root request-to-final (040605b0) chooses the first registered state that
    /// contains this participant. Completion is not a prerequisite.
    MoveFirstMember {
        sources: Vec<StateId>,
        participant: Participant,
        target: Option<StateId>,
    },
    /// TriggerAll iterates every active member under an iteration guard, then
    /// drains that source's queue before visiting the next authored source.
    MoveAll {
        sources: Vec<StateId>,
        target: Option<StateId>,
    },
    /// Direct native removal (0402ae40), including removal during another
    /// callback. Emits Leaving/Left, but no Transitioned notification.
    Remove {
        state: StateId,
        participant: Participant,
    },
    /// Membership-clearing phase of base-state deactivation0402b170. Re-read
    /// the first member after each removal; exit callbacks can change the set.
    /// Source-destruction listeners and entity deactivation remain host-owned.
    RemoveAll {
        state: StateId,
    },
    MarkForDelete(Participant),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Notification {
    /// Internal host continuation, delivered only to the selected observer.
    /// It is neither a membership notification nor a network message.
    Invoked { token: u64 },
    /// One existing member, visited under its state's iteration guard while
    /// activating a particular observer. Only that observer receives this.
    Existing {
        state: StateId,
        participant: Participant,
    },
    /// Membership is present; RPC enter precedes the specialized entry hook.
    Entered {
        state: StateId,
        participant: Participant,
        send_rpc: bool,
    },
    /// Completion is present; membership is retained until a move is applied.
    Completed {
        state: StateId,
        participant: Participant,
    },
    /// Removal observers run while the old membership is still visible.
    Leaving {
        state: StateId,
        participant: Participant,
    },
    /// Membership and completion are removed; RPC exit precedes the exit hook.
    Left {
        state: StateId,
        participant: Participant,
        send_rpc: bool,
    },
    /// After source exit and before target entry, including a null target.
    Transitioned {
        source: StateId,
        participant: Participant,
        target: Option<StateId>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    definition: Definition,
    members: BTreeSet<Participant>,
    completed: BTreeSet<Participant>,
    pending: Vec<(Participant, Option<StateId>)>,
    callbacks: usize,
    draining: bool,
}

/// Read-only membership interface. A bound view can translate an authored
/// state alias without copying membership into another machine.
pub trait Membership {
    fn members(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error>;
    fn completed(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error>;
}

/// Owned by one session transaction. State IDs are internal handles; wire
/// serializer identities belong to the enclosing instance bindings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Machine {
    states: BTreeMap<StateId, State>,
    marked: BTreeSet<Participant>,
}

#[cfg(test)]
struct Run<'a, F> {
    machine: &'a mut Machine,
    observer: F,
    output: Vec<Notification>,
    steps: usize,
    depth: usize,
    observer_count: usize,
}

impl Machine {
    pub fn empty() -> Self {
        Self {
            states: BTreeMap::new(),
            marked: BTreeSet::new(),
        }
    }

    /// Install definitions atomically, without entering any participants.
    pub fn install(&mut self, definitions: &[Definition]) -> Result<(), Error> {
        if definitions.len() > MAX_STATES
            || self.states.len().saturating_add(definitions.len()) > MAX_WORLD_STATES
        {
            return Err(Error::Bound);
        }
        let mut ids = BTreeSet::new();
        for definition in definitions {
            if self.states.contains_key(&definition.id) || !ids.insert(definition.id) {
                return Err(Error::Invalid);
            }
        }
        for &definition in definitions {
            self.states.insert(
                definition.id,
                State {
                    definition,
                    members: BTreeSet::new(),
                    completed: BTreeSet::new(),
                    pending: vec![],
                    callbacks: 0,
                    draining: false,
                },
            );
        }
        Ok(())
    }

    /// Storage retirement only, after the host has dispatched removal and
    /// destruction callbacks. No occupied or still referenced state is erased.
    pub fn retire(&mut self, ids: &[StateId]) -> Result<(), Error> {
        let unique: BTreeSet<_> = ids.iter().copied().collect();
        if unique.len() != ids.len() {
            return Err(Error::Invalid);
        }
        for &id in ids {
            let state = self.state(id)?;
            if !state.members.is_empty()
                || !state.completed.is_empty()
                || !state.pending.is_empty()
                || state.callbacks != 0
                || state.draining
            {
                return Err(Error::Invalid);
            }
        }
        if self.states.values().any(|s| {
            s.pending
                .iter()
                .any(|(_, target)| target.is_some_and(|id| unique.contains(&id)))
        }) {
            return Err(Error::Invalid);
        }
        for id in ids {
            self.states.remove(id);
        }
        Ok(())
    }

    pub fn new(definitions: &[Definition]) -> Result<Self, Error> {
        if definitions.is_empty() || definitions.len() > MAX_STATES {
            return Err(Error::Bound);
        }
        let mut states = BTreeMap::new();
        for &definition in definitions {
            if states
                .insert(
                    definition.id,
                    State {
                        definition,
                        members: BTreeSet::new(),
                        completed: BTreeSet::new(),
                        pending: Vec::new(),
                        callbacks: 0,
                        draining: false,
                    },
                )
                .is_some()
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Self {
            states,
            marked: BTreeSet::new(),
        })
    }

    pub fn members(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        Ok(&self.state(state)?.members)
    }

    pub fn completed(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        Ok(&self.state(state)?.completed)
    }

    fn state(&self, id: StateId) -> Result<&State, Error> {
        self.states.get(&id).ok_or(Error::UnknownState)
    }

    fn state_mut(&mut self, id: StateId) -> Result<&mut State, Error> {
        self.states.get_mut(&id).ok_or(Error::UnknownState)
    }

    /// Execute one atomic batch. The observer receives each native callback
    /// point and can return synchronous actions. A host whose observer mutates
    /// graphs must use a candidate copy of those graphs and commit them only
    /// after this succeeds, together with the returned notifications.
    pub fn run<F>(
        &mut self,
        actions: &[Action],
        mut observer: F,
    ) -> Result<Vec<Notification>, Error>
    where
        F: FnMut(&Machine, Notification) -> Result<Vec<Action>, Error>,
    {
        self.run_ordered(actions, 1, |machine, event, _| observer(machine, event))
    }

    /// Each observer's synchronous actions run before the next observer is
    /// called. Returning a combined action list from one callback cannot model
    /// this ordering when an early observer completes a state recursively.
    /// Registration order belongs to the host; the last entry callback can
    /// implement the specialized state's entry hook after its observers.
    pub fn run_ordered<F>(
        &mut self,
        actions: &[Action],
        observer_count: usize,
        mut observer: F,
    ) -> Result<Vec<Notification>, Error>
    where
        F: FnMut(&Machine, Notification, usize) -> Result<Vec<Action>, Error>,
    {
        let mut execution = self.begin(actions, observer_count)?;
        while let Some(call) = execution.advance()? {
            let actions = observer(execution.machine(), call.notification, call.observer)?;
            execution.resume(actions)?;
        }
        let (next, output) = execution.finish()?;
        *self = next;
        Ok(output)
    }
}

impl Membership for Machine {
    fn members(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        Machine::members(self, state)
    }
    fn completed(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        Machine::completed(self, state)
    }
}

// Prior recursive implementation retained only as a test oracle while the
// resumable dispatcher is integrated. Production has one execution path.
#[cfg(test)]
impl<F> Run<'_, F>
where
    F: FnMut(&Machine, Notification, usize) -> Result<Vec<Action>, Error>,
{
    fn step(&mut self) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > MAX_STEPS || self.depth >= MAX_DEPTH {
            Err(Error::Bound)
        } else {
            Ok(())
        }
    }

    fn emit(&mut self, state: StateId, event: Notification) -> Result<(), Error> {
        self.step()?;
        if self.output.len() >= MAX_STEPS {
            return Err(Error::Bound);
        }
        self.output.push(event);
        self.machine.state_mut(state)?.callbacks += 1;
        for observer in 0..self.observer_count {
            self.notify(event, observer)?;
        }
        self.machine.state_mut(state)?.callbacks -= 1;
        Ok(())
    }

    fn notify(&mut self, event: Notification, observer: usize) -> Result<(), Error> {
        self.step()?;
        let actions = (self.observer)(self.machine, event, observer)?;
        if actions.len() > MAX_PENDING {
            return Err(Error::Bound);
        }
        for action in actions {
            self.apply(action)?;
        }
        Ok(())
    }

    fn apply(&mut self, action: Action) -> Result<(), Error> {
        self.step()?;
        self.depth += 1;
        match action {
            Action::Invoke { observer, token } => {
                if observer >= self.observer_count {
                    return Err(Error::Invalid);
                }
                self.notify(Notification::Invoked { token }, observer)?;
            }
            Action::Enter { state, participant } => {
                let s = self.machine.state(state)?;
                if !s.members.contains(&participant) && s.members.len() >= MAX_PARTICIPANTS {
                    return Err(Error::Bound);
                }
                let send_rpc = s.definition.send_rpc_to_client;
                self.machine.state_mut(state)?.members.insert(participant);
                self.emit(
                    state,
                    Notification::Entered {
                        state,
                        participant,
                        send_rpc,
                    },
                )?;
                self.drain(state)?;
                self.check_deleted(state, participant)?;
            }
            Action::Complete { state, participant } => {
                // The host reports completion only for a still-owned member.
                // Repeated completion remains a callback, as in 0402c800.
                if self.machine.state(state)?.members.contains(&participant) {
                    self.machine.state_mut(state)?.completed.insert(participant);
                    self.emit(state, Notification::Completed { state, participant })?;
                    self.drain(state)?;
                }
            }
            Action::CompleteAll {
                state,
                only_incomplete,
            } => {
                let members = self
                    .machine
                    .state(state)?
                    .members
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                self.machine.state_mut(state)?.callbacks += 1;
                for participant in members {
                    if !only_incomplete
                        || !self.machine.state(state)?.completed.contains(&participant)
                    {
                        self.apply(Action::Complete { state, participant })?;
                    }
                }
                self.machine.state_mut(state)?.callbacks -= 1;
                self.drain(state)?;
            }
            Action::SeedMembers { state, observer } => {
                if observer >= self.observer_count {
                    return Err(Error::Invalid);
                }
                let members = self
                    .machine
                    .state(state)?
                    .members
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                self.machine.state_mut(state)?.callbacks += 1;
                for participant in members {
                    let event = Notification::Existing { state, participant };
                    if self.output.len() >= MAX_STEPS {
                        return Err(Error::Bound);
                    }
                    self.output.push(event);
                    self.notify(event, observer)?;
                }
                self.machine.state_mut(state)?.callbacks -= 1;
                self.drain(state)?;
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
                self.drain(source)?;
            }
            Action::MoveFirstCompleted {
                sources,
                participant,
                target,
            } => {
                if sources.len() > MAX_STATES {
                    return Err(Error::Bound);
                }
                if let Some(target) = target {
                    self.machine.state(target)?;
                }
                // Validate all sources before selecting one; malformed content
                // cannot hide an unknown endpoint behind an earlier match.
                for &source in &sources {
                    self.machine.state(source)?;
                }
                if let Some(source) = sources
                    .into_iter()
                    .find(|s| self.machine.states[s].completed.contains(&participant))
                {
                    self.apply(Action::Move {
                        source,
                        participant,
                        target,
                    })?;
                }
            }
            Action::MoveFirstMember {
                sources,
                participant,
                target,
            } => {
                if sources.len() > MAX_STATES {
                    return Err(Error::Bound);
                }
                if let Some(target) = target {
                    self.machine.state(target)?;
                }
                for &source in &sources {
                    self.machine.state(source)?;
                }
                if let Some(source) = sources
                    .into_iter()
                    .find(|s| self.machine.states[s].members.contains(&participant))
                {
                    self.apply(Action::Move {
                        source,
                        participant,
                        target,
                    })?;
                }
            }
            Action::MoveAll { sources, target } => {
                if sources.len() > MAX_STATES {
                    return Err(Error::Bound);
                }
                if let Some(target) = target {
                    self.machine.state(target)?;
                }
                for &source in &sources {
                    self.machine.state(source)?;
                }
                for source in sources {
                    let members = self
                        .machine
                        .state(source)?
                        .members
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    self.machine.state_mut(source)?.callbacks += 1;
                    for participant in members {
                        self.apply(Action::Move {
                            source,
                            participant,
                            target,
                        })?;
                    }
                    self.machine.state_mut(source)?.callbacks -= 1;
                    self.drain(source)?;
                }
            }
            Action::Remove { state, participant } => {
                if self.machine.state(state)?.members.contains(&participant) {
                    self.emit(state, Notification::Leaving { state, participant })?;
                    let s = self.machine.state_mut(state)?;
                    s.members.remove(&participant);
                    s.completed.remove(&participant);
                    let send_rpc = s.definition.send_rpc_to_client;
                    self.emit(
                        state,
                        Notification::Left {
                            state,
                            participant,
                            send_rpc,
                        },
                    )?;
                    self.drain(state)?;
                }
            }
            Action::RemoveAll { state } => {
                while let Some(&participant) = self.machine.state(state)?.members.first() {
                    self.apply(Action::Remove { state, participant })?;
                }
            }
            Action::MarkForDelete(participant) => {
                if !self.machine.marked.contains(&participant)
                    && self.machine.marked.len() >= MAX_PARTICIPANTS
                {
                    return Err(Error::Bound);
                }
                self.machine.marked.insert(participant);
                let states = self.machine.states.keys().copied().collect::<Vec<_>>();
                for state in states {
                    self.check_deleted(state, participant)?;
                }
            }
        }
        self.depth -= 1;
        Ok(())
    }

    fn check_deleted(&mut self, state: StateId, participant: Participant) -> Result<(), Error> {
        let s = self.machine.state(state)?;
        if self.machine.marked.contains(&participant)
            && !s.definition.handles_marked_for_delete
            && s.members.contains(&participant)
            && !s.completed.contains(&participant)
        {
            self.apply(Action::Complete { state, participant })?;
        }
        Ok(())
    }

    fn drain(&mut self, source: StateId) -> Result<(), Error> {
        let s = self.machine.state(source)?;
        if s.callbacks != 0 || s.draining {
            return Ok(());
        }
        self.machine.state_mut(source)?.draining = true;
        while let Some((participant, target)) = self.machine.state_mut(source)?.pending.pop() {
            self.step()?;
            if !self.machine.state(source)?.members.contains(&participant) {
                continue;
            }
            self.emit(
                source,
                Notification::Leaving {
                    state: source,
                    participant,
                },
            )?;
            let s = self.machine.state_mut(source)?;
            s.members.remove(&participant);
            s.completed.remove(&participant);
            let send_rpc = s.definition.send_rpc_to_client;
            self.emit(
                source,
                Notification::Left {
                    state: source,
                    participant,
                    send_rpc,
                },
            )?;
            self.emit(
                source,
                Notification::Transitioned {
                    source,
                    participant,
                    target,
                },
            )?;
            if let Some(state) = target {
                self.apply(Action::Enter { state, participant })?;
            }
        }
        self.machine.state_mut(source)?.draining = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
