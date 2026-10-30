// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared integer stores and native sorter observers owned by one event.
use super::listeners::List;
use super::*;
use nfs_world::scoring::{rank_query::Row, rank_sorter::Sorter};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Effect {
    pub sorter: Identity,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub observer: u64,
    pub source: IntegerSource,
    pub participant: Participant,
    pub value: i32,
}

#[derive(Clone, Debug)]
pub(super) enum Work {
    Write {
        source: IntegerSource,
        participant: Participant,
        value: i32,
    },
    Dispatch {
        source: IntegerSource,
        participant: Participant,
        value: i32,
        index: usize,
    },
    External(Notification),
    Notify {
        sorter: usize,
        participant: Participant,
        value: i32,
    },
    Remove {
        sorter: usize,
        participant: Participant,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Listener {
    Sorter(usize),
    Mirror(usize),
    External(u64),
}

#[derive(Clone, Debug)]
pub(super) struct State {
    pub(super) dead: BTreeSet<IntegerSource>,
    pub(super) values: BTreeMap<(IntegerSource, Participant), i32>,
    listeners: BTreeMap<IntegerSource, List<Listener>>,
    sorters: Vec<Sorter>,
}
impl State {
    pub(super) fn new(plan: &Plan) -> Self {
        Self {
            dead: BTreeSet::new(),
            values: BTreeMap::new(),
            listeners: {
                let mut lists: BTreeMap<IntegerSource, List<Listener>> = BTreeMap::new();
                for observer in &plan.data.bindings.order {
                    if let Observer::Sorter(i) = *observer {
                        lists
                            .entry(plan.data.bindings.sorters[i].source.clone())
                            .or_default()
                            .items
                            .push(Some(Listener::Sorter(i)));
                    }
                }
                lists
            },
            sorters: plan
                .data
                .bindings
                .sorters
                .iter()
                .map(|s| Sorter::new(s.order))
                .collect(),
        }
    }
    pub(super) fn clear_listeners(&mut self) {
        self.listeners.clear();
    }
    /// The graph owner must detach external value subscriptions before source
    /// retirement. Source-destruction callbacks are a separate lifecycle path;
    /// dropping a still-live graph subscription would leave its cache stale.
    pub(super) fn retire_listeners(&mut self, source: &IntegerSource) -> Result<(), Error> {
        if self.listeners.get(source).is_some_and(|list| {
            list.items
                .iter()
                .any(|listener| matches!(listener, Some(Listener::External(_))))
        }) {
            return Err(Error::Invalid);
        }
        self.listeners.remove(source);
        Ok(())
    }
    pub(super) fn subscribe(
        &mut self,
        source: IntegerSource,
        listener: Listener,
    ) -> Result<(), Error> {
        if self.dead.contains(&source) {
            return Err(Error::Invalid);
        }
        if self
            .listeners
            .values()
            .map(|list| list.items.len())
            .sum::<usize>()
            >= states::MAX_PENDING
            || (self.listeners.len() >= 256 && !self.listeners.contains_key(&source))
        {
            return Err(Error::Bound);
        }
        self.listeners
            .entry(source)
            .or_default()
            .items
            .push(Some(listener));
        Ok(())
    }
    pub(super) fn unsubscribe(&mut self, source: &IntegerSource, listener: Listener) {
        if let Some(list) = self.listeners.get_mut(source) {
            list.remove(listener);
        }
    }
    pub(super) fn unsubscribe_mirror(&mut self, mirror: usize) {
        for list in self.listeners.values_mut() {
            list.remove(Listener::Mirror(mirror));
        }
    }
}
impl Plan {
    pub(super) fn validate_integer(&self, source: &IntegerSource) -> Result<(), Error> {
        match source {
            IntegerSource::Local(identity) if self.data.bindings.integers.contains(identity) => {
                Ok(())
            }
            IntegerSource::Channel(key) => key.validate().map_err(|_| Error::Invalid),
            _ => Err(Error::Invalid),
        }
    }
}
impl Host {
    pub fn integer_value(&self, source: &IntegerSource, participant: Participant) -> Option<i32> {
        self.observers
            .rankings
            .values
            .get(&(source.clone(), participant))
            .copied()
    }
    pub fn ranking(&self, sorter: usize) -> Result<&[Row], Error> {
        self.observers.rankings.rows(sorter)
    }

    /// Deliver one actual property write. Sorters subscribe at activation and
    /// do not seed from earlier values. Each callback sees the source updated
    /// and each table publication completes before the next sorter runs.
    pub fn integer(
        &mut self,
        source: IntegerSource,
        participant: Participant,
        value: i32,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        self.plan.validate_integer(&source)?;
        if self.observers.rankings.dead.contains(&source) {
            return Err(Error::Invalid);
        }
        if self.integer_value(&source, participant) == Some(value) {
            return Ok(vec![]);
        }
        let mut next = self.clone();
        let actions = next.observers.enqueue([super::Work::Ranking(Work::Write {
            source,
            participant,
            value,
        })])?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    /// Integer property and sorter teardown on participant destruction. Leaving
    /// a race state alone must retain the result rows for the outro queries.
    pub fn remove_integer_participant(
        &mut self,
        participant: Participant,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        let mut next = self.clone();
        next.observers
            .rankings
            .values
            .retain(|(_, p), _| *p != participant);
        let work = next
            .plan
            .data
            .bindings
            .order
            .iter()
            .filter(|o| next.observers.enabled.contains(o))
            .filter_map(|o| match *o {
                Observer::Sorter(sorter) => Some(super::Work::Ranking(Work::Remove {
                    sorter,
                    participant,
                })),
                _ => None,
            })
            .collect::<Vec<_>>();
        let actions = next.observers.enqueue(work)?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }
}

impl State {
    pub(super) fn rows(&self, sorter: usize) -> Result<&[Row], Error> {
        Ok(self.sorters.get(sorter).ok_or(Error::Invalid)?.rows())
    }
}
impl Observers {
    pub(super) fn ranking_work(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<super::Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        if let Work::Write {
            source,
            participant,
            value,
        } = work
        {
            if self.rankings.dead.contains(&source) {
                return Err(Error::Invalid);
            }
            let key = (source.clone(), participant);
            if self.rankings.values.get(&key) == Some(&value) {
                return Ok(vec![]);
            }
            if self.rankings.values.len() >= states::MAX_PENDING
                && !self.rankings.values.contains_key(&key)
            {
                return Err(Error::Bound);
            }
            self.rankings.values.insert(key, value);
            let Some(list) = self.rankings.listeners.get_mut(&source) else {
                return Ok(vec![]);
            };
            list.notifying = true;
            return self.enqueue([super::Work::Ranking(Work::Dispatch {
                source,
                participant,
                value,
                index: 0,
            })]);
        }
        if let Work::Dispatch {
            source,
            participant,
            value,
            index,
        } = work
        {
            let Some(listener) = self
                .rankings
                .listeners
                .get_mut(&source)
                .and_then(|list| list.next(index))
            else {
                return Ok(vec![]);
            };
            let call = match listener {
                Listener::Sorter(sorter) => super::Work::Ranking(Work::Notify {
                    sorter,
                    participant,
                    value,
                }),
                Listener::Mirror(i) => super::Work::Mirror(super::mirrors::Work::Forward(
                    i,
                    source.clone(),
                    participant,
                    value,
                )),
                Listener::External(observer) => {
                    super::Work::Ranking(Work::External(Notification {
                        observer,
                        source: source.clone(),
                        participant,
                        value,
                    }))
                }
            };
            return self.enqueue([
                call,
                super::Work::Ranking(Work::Dispatch {
                    source,
                    participant,
                    value,
                    index: index + 1,
                }),
            ]);
        }
        if let Work::External(notification) = work {
            return emit(
                machine,
                super::Effect::Integer(notification),
                effects,
                callback,
            );
        }
        let index = match work {
            Work::Notify { sorter, .. } | Work::Remove { sorter, .. } => sorter,
            Work::Write { .. } | Work::Dispatch { .. } | Work::External(_) => unreachable!(),
        };
        let sorter = self.rankings.sorters.get_mut(index).ok_or(Error::Invalid)?;
        match work {
            Work::Notify {
                participant, value, ..
            } => {
                sorter
                    .notify(participant, value)
                    .map_err(|_| Error::Bound)?;
            }
            Work::Remove { participant, .. } if !sorter.remove(participant) => return Ok(vec![]),
            Work::Remove { .. } => {}
            Work::Write { .. } | Work::Dispatch { .. } | Work::External(_) => unreachable!(),
        }
        emit(
            machine,
            super::Effect::Ranking(Effect {
                sorter: plan.data.bindings.sorters[index].identity.clone(),
                rows: sorter.rows().to_vec(),
            }),
            effects,
            callback,
        )
    }
}
