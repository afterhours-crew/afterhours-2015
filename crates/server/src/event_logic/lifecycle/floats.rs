// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared float participant properties. Native 04055b60 writes before notifying
//! 04055ec0, with numeric equality (including equal signed zero). The server's
//! ordinary finite-value policy also applies at this host boundary.
use super::{listeners::List, *};

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub observer: u64,
    pub source: Property,
    pub participant: Participant,
    pub value: f32,
}
#[derive(Clone, Debug)]
pub(super) enum Work {
    Dispatch {
        source: Property,
        participant: Participant,
        value: f32,
        index: usize,
    },
    Notify(Notification),
}
#[derive(Clone, Debug, Default)]
pub(super) struct State {
    pub values: BTreeMap<(Property, Participant), f32>,
    listeners: BTreeMap<Property, List<u64>>,
}
impl State {
    pub fn observe(
        &mut self,
        source: Property,
        observer: u64,
        subscribe: bool,
    ) -> Result<(), Error> {
        source.validate().map_err(|_| Error::Invalid)?;
        if observer == 0 {
            return Err(Error::Invalid);
        }
        if subscribe {
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
                .push(Some(observer));
        } else if let Some(list) = self.listeners.get_mut(&source) {
            list.remove(observer);
        }
        Ok(())
    }
}
impl Host {
    pub fn float_value(&self, source: &Property, participant: Participant) -> Option<f32> {
        self.observers
            .floats
            .values
            .get(&(source.clone(), participant))
            .copied()
    }
}
impl Observers {
    pub fn write_float(
        &mut self,
        source: Property,
        participant: Participant,
        value: f32,
    ) -> Result<Vec<Action>, Error> {
        source.validate().map_err(|_| Error::Invalid)?;
        if !value.is_finite() {
            return Err(Error::Invalid);
        }
        let key = (source.clone(), participant);
        if self.floats.values.get(&key) == Some(&value) {
            return Ok(vec![]);
        }
        if self.floats.values.len() >= states::MAX_PENDING && !self.floats.values.contains_key(&key)
        {
            return Err(Error::Bound);
        }
        self.floats.values.insert(key, value);
        let Some(list) = self.floats.listeners.get_mut(&source) else {
            return Ok(vec![]);
        };
        list.notifying = true;
        self.enqueue([super::Work::Float(Work::Dispatch {
            source,
            participant,
            value,
            index: 0,
        })])
    }
    pub fn float_work(
        &mut self,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        match work {
            Work::Notify(notification) => {
                emit(machine, Effect::Float(notification), effects, callback)
            }
            Work::Dispatch {
                source,
                participant,
                value,
                index,
            } => {
                let Some(observer) = self
                    .floats
                    .listeners
                    .get_mut(&source)
                    .and_then(|list| list.next(index))
                else {
                    return Ok(vec![]);
                };
                self.enqueue([
                    super::Work::Float(Work::Notify(Notification {
                        observer,
                        source: source.clone(),
                        participant,
                        value,
                    })),
                    super::Work::Float(Work::Dispatch {
                        source,
                        participant,
                        value,
                        index: index + 1,
                    }),
                ])
            }
        }
    }
}

#[cfg(test)]
mod tests;
