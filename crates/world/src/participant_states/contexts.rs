// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant context ownership. Native category add/remove increments and
//! decrements a count keyed by CategoryHash (046d8a00/046dd830). Removing one
//! state's context must not clear a category still owned by another context.
use super::{Error, MAX_PARTICIPANTS, MAX_STATES, Machine, Notification, Participant, StateId};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CATEGORIES: usize = 128;
pub const MAX_CONTEXTS: usize = 4096;
/// Host-assigned identity, scoped to one manager lifetime.
pub type Owner = u64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub participant: Participant,
    /// Active category hashes. A change is still emitted when this set is
    /// unchanged but its ownership counts changed, as native observers are.
    pub categories: BTreeSet<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Manager {
    owned: BTreeMap<(Owner, Participant), Vec<u32>>,
    counts: BTreeMap<Participant, BTreeMap<u32, usize>>,
}

impl Manager {
    pub fn categories(&self, participant: Participant) -> BTreeSet<u32> {
        self.counts
            .get(&participant)
            .map(|c| c.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn add(
        &mut self,
        owner: Owner,
        participant: Participant,
        categories: &[u32],
    ) -> Result<Option<Change>, Error> {
        if let Some(old) = self.owned.get(&(owner, participant)) {
            return if old == categories {
                Ok(None)
            } else {
                Err(Error::Invalid)
            };
        }
        if self.owned.len() >= MAX_CONTEXTS
            || categories.len() > MAX_CATEGORIES
            || !self.counts.contains_key(&participant) && self.counts.len() >= MAX_PARTICIPANTS
        {
            return Err(Error::Bound);
        }
        let mut counts = self.counts.get(&participant).cloned().unwrap_or_default();
        for &category in categories {
            *counts.entry(category).or_default() += 1;
        }
        if counts.len() > MAX_CATEGORIES {
            return Err(Error::Bound);
        }
        self.counts.insert(participant, counts);
        self.owned.insert((owner, participant), categories.to_vec());
        Ok(Some(Change {
            participant,
            categories: self.categories(participant),
        }))
    }

    pub fn remove(&mut self, owner: Owner, participant: Participant) -> Option<Change> {
        let categories = self.owned.remove(&(owner, participant))?;
        if let Some(counts) = self.counts.get_mut(&participant) {
            for category in categories {
                if let Some(count) = counts.get_mut(&category) {
                    *count -= 1;
                    if *count == 0 {
                        counts.remove(&category);
                    }
                }
            }
        }
        if !self.owned.keys().any(|&(_, p)| p == participant) {
            self.counts.remove(&participant);
        }
        Some(Change {
            participant,
            categories: self.categories(participant),
        })
    }

    /// Teardown is explicit and bounded; a manager is never shared implicitly
    /// across connections or reused with dangling owners after a restart.
    pub fn remove_owner(&mut self, owner: Owner) -> Vec<Change> {
        let participants: Vec<_> = self
            .owned
            .keys()
            .filter_map(|&(o, p)| (o == owner).then_some(p))
            .collect();
        participants
            .into_iter()
            .filter_map(|p| self.remove(owner, p))
            .collect()
    }

    pub fn forget(&mut self, participant: Participant) {
        self.owned.retain(|&(_, p), _| p != participant);
        self.counts.remove(&participant);
    }
}

/// State context observes entry and transition, not leaving. A transition
/// within its watched states keeps the same context (046e9890/046e9940).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateContext {
    owner: Owner,
    states: Vec<StateId>,
    categories: Vec<u32>,
}

impl StateContext {
    pub fn new(
        owner: Owner,
        states: Vec<StateId>,
        categories: Vec<u32>,
        machine: &Machine,
    ) -> Result<Self, Error> {
        if states.len() > MAX_STATES || categories.len() > MAX_CATEGORIES {
            return Err(Error::Bound);
        }
        for &state in &states {
            machine.members(state)?;
        }
        Ok(Self {
            owner,
            states,
            categories,
        })
    }

    pub fn observe(
        &self,
        manager: &mut Manager,
        event: Notification,
    ) -> Result<Option<Change>, Error> {
        match event {
            Notification::Entered {
                state, participant, ..
            } if self.states.contains(&state) => {
                manager.add(self.owner, participant, &self.categories)
            }
            Notification::Transitioned {
                source,
                participant,
                target,
            } if self.states.contains(&source)
                && !target.is_some_and(|s| self.states.contains(&s)) =>
            {
                Ok(manager.remove(self.owner, participant))
            }
            _ => Ok(None),
        }
    }

    pub fn retire(&self, manager: &mut Manager) -> Vec<Change> {
        manager.remove_owner(self.owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::participant_states::{Action, Definition};

    #[test]
    fn overlapping_scopes_count_duplicates_and_notify_when_categories_do_not_change() {
        let mut m = Manager::default();
        assert_eq!(
            m.add(1, 7, &[10, 10, 20]).unwrap().unwrap().categories,
            BTreeSet::from([10, 20])
        );
        assert!(m.add(1, 7, &[10, 10, 20]).unwrap().is_none());
        assert!(m.add(2, 7, &[10]).unwrap().is_some());
        assert_eq!(m.remove(1, 7).unwrap().categories, BTreeSet::from([10]));
        assert_eq!(m.remove(2, 7).unwrap().categories, BTreeSet::new());
        assert_eq!(m.remove(2, 7), None);
        assert!(m.counts.is_empty());
    }

    #[test]
    fn state_context_survives_internal_moves_and_releases_on_null_exit() {
        let mut machine = Machine::new(&[1, 2].map(|id| Definition {
            id,
            handles_marked_for_delete: false,
            send_rpc_to_client: false,
        }))
        .unwrap();
        let context = StateContext::new(1, vec![1, 2], vec![10], &machine).unwrap();
        let mut manager = Manager::default();
        manager.add(2, 7, &[10]).unwrap();
        let mut changes = Vec::new();
        machine
            .run(
                &[
                    Action::Enter {
                        state: 1,
                        participant: 7,
                    },
                    Action::Move {
                        source: 1,
                        participant: 7,
                        target: Some(2),
                    },
                    Action::Move {
                        source: 2,
                        participant: 7,
                        target: None,
                    },
                ],
                |_, e| {
                    if let Some(c) = context.observe(&mut manager, e)? {
                        changes.push(c);
                    }
                    Ok(vec![])
                },
            )
            .unwrap();
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|c| c.categories == BTreeSet::from([10])));
        assert!(context.retire(&mut manager).is_empty());
        assert_eq!(manager.remove_owner(2).len(), 1);
    }

    #[test]
    fn failed_acquisition_is_atomic_and_teardown_preserves_other_participants() {
        let mut m = Manager::default();
        m.add(1, 7, &[3]).unwrap();
        m.add(1, 8, &[4]).unwrap();
        let before = m.clone();
        assert_eq!(m.add(1, 7, &[9]), Err(Error::Invalid));
        assert_eq!(m.add(2, 7, &vec![0; MAX_CATEGORIES + 1]), Err(Error::Bound));
        assert_eq!(m, before);
        m.forget(7);
        assert_eq!(m.categories(8), BTreeSet::from([4]));
        assert_eq!(
            m.remove_owner(1),
            [Change {
                participant: 8,
                categories: BTreeSet::new()
            }]
        );
        for participant in 0..MAX_PARTICIPANTS as u64 {
            m.add(1, participant, &[]).unwrap();
        }
        let before = m.clone();
        assert_eq!(m.add(1, 999, &[]), Err(Error::Bound));
        assert_eq!(m, before);
        m.forget(7);
        assert!(m.add(1, 999, &[]).is_ok());
    }
}
