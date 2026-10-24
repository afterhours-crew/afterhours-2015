// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native state-list and state-range Boolean participant properties. These
//! are observer-driven latches, not a query of the machine's current members.
//! A list observes entry/transition (046fc7c0/046fca10); a range observes entry
//! to its begin state and leaving its end state (046fc7f0/046fc960).
use super::{Error, MAX_PARTICIPANTS, MAX_STATES, Machine, Notification, Participant, StateId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Definition<T = StateId> {
    List(Vec<T>),
    Range { begin: T, end: T },
}

impl<T> Definition<T> {
    pub fn try_map<U, E>(
        &self,
        mut map: impl FnMut(&T) -> Result<U, E>,
    ) -> Result<Definition<U>, E> {
        Ok(match self {
            Self::List(states) => {
                Definition::List(states.iter().map(map).collect::<Result<_, _>>()?)
            }
            Self::Range { begin, end } => Definition::Range {
                begin: map(begin)?,
                end: map(end)?,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Change {
    pub participant: Participant,
    pub value: bool,
}

/// One property instance within one event lifetime. Retaining an explicit false
/// differs from no entry: the first false assignment still notifies subscribers
/// (04055cf0 -> 04055c20). Participant deletion removes that distinction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Property {
    definition: Definition,
    values: BTreeMap<Participant, bool>,
}

impl Property {
    pub fn new(definition: Definition, machine: &Machine) -> Result<Self, Error> {
        let states = match &definition {
            Definition::List(states) => states.clone(),
            Definition::Range { begin, end } => vec![*begin, *end],
        };
        if states.len() > MAX_STATES {
            return Err(Error::Bound);
        }
        for state in states {
            machine.members(state)?;
        }
        Ok(Self {
            definition,
            values: BTreeMap::new(),
        })
    }

    /// Activation registers and seeds each list source in authored order;
    /// a range seeds only its begin state (046fb630/046fb750). The host must
    /// process one source's existing members under that source's iteration
    /// guard, dispatching each returned change synchronously, before advancing
    /// to the next source. Re-query membership after deferred moves drain.
    pub fn seed_sources(&self) -> Vec<StateId> {
        match &self.definition {
            Definition::List(states) => states.clone(),
            Definition::Range { begin, .. } => vec![*begin],
        }
    }

    pub fn seed_member(&mut self, participant: Participant) -> Result<Option<Change>, Error> {
        self.set(participant, true)
    }

    pub fn value(&self, participant: Participant) -> Option<bool> {
        self.values.get(&participant).copied()
    }

    /// Activation-time subscribers enumerate stored entries, including false.
    pub fn stored_values(&self) -> &BTreeMap<Participant, bool> {
        &self.values
    }

    pub fn forget(&mut self, participant: Participant) {
        self.values.remove(&participant);
    }

    pub fn observe(&mut self, event: Notification) -> Result<Option<Change>, Error> {
        let assignment = match (&self.definition, event) {
            (
                Definition::List(states),
                Notification::Entered {
                    state, participant, ..
                },
            ) if states.contains(&state) => Some((participant, true)),
            (
                Definition::List(states),
                Notification::Transitioned {
                    source,
                    participant,
                    target,
                },
            ) if states.contains(&source) => {
                Some((participant, target.is_some_and(|t| states.contains(&t))))
            }
            (
                Definition::Range { begin, .. },
                Notification::Entered {
                    state, participant, ..
                },
            ) if *begin == state => Some((participant, true)),
            (Definition::Range { end, .. }, Notification::Leaving { state, participant })
                if *end == state =>
            {
                Some((participant, false))
            }
            _ => None,
        };
        match assignment {
            Some((participant, value)) => self.set(participant, value),
            None => Ok(None),
        }
    }

    fn set(&mut self, participant: Participant, value: bool) -> Result<Option<Change>, Error> {
        if self.value(participant) == Some(value) {
            return Ok(None);
        }
        if !self.values.contains_key(&participant) && self.values.len() >= MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        self.values.insert(participant, value);
        Ok(Some(Change { participant, value }))
    }
}

#[cfg(test)]
mod tests;
