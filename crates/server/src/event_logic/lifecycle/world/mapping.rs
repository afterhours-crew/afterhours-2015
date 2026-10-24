// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

impl Scope {
    pub(super) fn state(&self, state: StateId) -> Result<StateId, Error> {
        self.local.get(&state).copied().ok_or(Error::UnknownState)
    }

    pub(super) fn action(&self, action: Action) -> Result<Action, Error> {
        let target = |target: Option<StateId>| target.map(|s| self.state(s)).transpose();
        let sources = |sources: Vec<StateId>| {
            sources
                .into_iter()
                .map(|s| self.state(s))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(match action {
            Action::Enter { state, participant } => Action::Enter {
                state: self.state(state)?,
                participant,
            },
            Action::Complete { state, participant } => Action::Complete {
                state: self.state(state)?,
                participant,
            },
            Action::CompleteAll {
                state,
                only_incomplete,
            } => Action::CompleteAll {
                state: self.state(state)?,
                only_incomplete,
            },
            Action::Remove { state, participant } => Action::Remove {
                state: self.state(state)?,
                participant,
            },
            Action::RemoveAll { state } => Action::RemoveAll {
                state: self.state(state)?,
            },
            Action::Move {
                source,
                participant,
                target: to,
            } => Action::Move {
                source: self.state(source)?,
                participant,
                target: target(to)?,
            },
            Action::MoveAll {
                sources: from,
                target: to,
            } => Action::MoveAll {
                sources: sources(from)?,
                target: target(to)?,
            },
            Action::MoveFirstCompleted {
                sources: from,
                participant,
                target: to,
            } => Action::MoveFirstCompleted {
                sources: sources(from)?,
                participant,
                target: target(to)?,
            },
            Action::MoveFirstMember {
                sources: from,
                participant,
                target: to,
            } => Action::MoveFirstMember {
                sources: sources(from)?,
                participant,
                target: target(to)?,
            },
            Action::SeedMembers { state, observer } => Action::SeedMembers {
                state: self.state(state)?,
                observer: *self
                    .seed_slots
                    .get(observer.checked_sub(1).ok_or(Error::Invalid)?)
                    .ok_or(Error::Invalid)?,
            },
            Action::MarkForDelete(p) => Action::MarkForDelete(p),
            Action::Invoke { .. } => return Err(Error::Invalid),
        })
    }

    pub(super) fn notification(&self, event: Notification) -> Result<Notification, Error> {
        let state = |id| self.reverse.get(&id).copied().ok_or(Error::UnknownState);
        Ok(match event {
            Notification::Entered {
                state: id,
                participant,
                send_rpc,
            } => Notification::Entered {
                state: state(id)?,
                participant,
                send_rpc,
            },
            Notification::Completed {
                state: id,
                participant,
            } => Notification::Completed {
                state: state(id)?,
                participant,
            },
            Notification::Leaving {
                state: id,
                participant,
            } => Notification::Leaving {
                state: state(id)?,
                participant,
            },
            Notification::Left {
                state: id,
                participant,
                send_rpc,
            } => Notification::Left {
                state: state(id)?,
                participant,
                send_rpc,
            },
            Notification::Existing {
                state: id,
                participant,
            } => Notification::Existing {
                state: state(id)?,
                participant,
            },
            Notification::Transitioned {
                source,
                participant,
                target,
            } => Notification::Transitioned {
                source: state(source)?,
                participant,
                // An unbound destination is outside every local list/range.
                // It must not be confused with an equal serializer elsewhere.
                target: target.and_then(|id| self.reverse.get(&id).copied()),
            },
            Notification::Invoked { .. } => return Err(Error::Invalid),
        })
    }
}

pub(super) fn source(event: Notification) -> Result<StateId, Error> {
    match event {
        Notification::Entered { state, .. }
        | Notification::Completed { state, .. }
        | Notification::Leaving { state, .. }
        | Notification::Left { state, .. }
        | Notification::Existing { state, .. } => Ok(state),
        Notification::Transitioned { source, .. } => Ok(source),
        Notification::Invoked { .. } => Err(Error::Invalid),
    }
}
