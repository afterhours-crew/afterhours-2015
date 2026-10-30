// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Boolean source observers registered at each entity's native activation.
use super::{listeners::List, *};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub observer: u64,
    pub source: BooleanSource,
    pub participant: Participant,
    pub value: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Listener {
    Predicate(usize),
    Set(usize),
    Spawner(usize),
    External(u64),
}

#[derive(Clone, Debug, Default)]
pub(super) struct Listeners(BTreeMap<BooleanSource, List<Listener>>);
impl Listeners {
    pub fn new(plan: &Plan) -> Result<Self, Error> {
        let mut result = Self::default();
        for &observer in &plan.data.bindings.order {
            result.activate(plan, observer)?;
        }
        Ok(result)
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    fn add(&mut self, source: BooleanSource, listener: Listener) -> Result<(), Error> {
        if self.0.values().map(|list| list.items.len()).sum::<usize>() >= states::MAX_PENDING
            || (self.0.len() >= 256 && !self.0.contains_key(&source))
        {
            return Err(Error::Bound);
        }
        self.0.entry(source).or_default().items.push(Some(listener));
        Ok(())
    }
    pub fn external(
        &mut self,
        source: BooleanSource,
        observer: u64,
        subscribe: bool,
    ) -> Result<(), Error> {
        if observer == 0 {
            return Err(Error::Invalid);
        }
        if subscribe {
            self.add(source, Listener::External(observer))
        } else {
            if let Some(list) = self.0.get_mut(&source) {
                list.remove(Listener::External(observer));
            }
            Ok(())
        }
    }
    pub fn activate(&mut self, plan: &Plan, observer: Observer) -> Result<(), Error> {
        match observer {
            Observer::Predicate(i) => {
                if let PredicateKind::Boolean { operands, .. } =
                    &plan.data.bindings.predicates[i].kind
                {
                    // 046e77c0 subscribes every operand occurrence, including repeats.
                    for operand in operands {
                        self.add(plan.boolean_source(operand)?, Listener::Predicate(i))?;
                    }
                }
            }
            Observer::Set(i) => {
                if let SetKind::Boolean(link) = &plan.data.bindings.sets[i].kind {
                    self.add(plan.boolean_source(link)?, Listener::Set(i))?;
                }
            }
            Observer::Spawner(i) => {
                if let Some(Controller::Boolean(link)) = &plan.data.bindings.spawners[i].controller
                {
                    self.add(plan.boolean_source(link)?, Listener::Spawner(i))?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn begin(&mut self, source: &BooleanSource) {
        if let Some(list) = self.0.get_mut(source) {
            list.notifying = true;
        }
    }
    pub fn next(
        &mut self,
        source: &BooleanSource,
        participant: Participant,
        value: bool,
        index: usize,
    ) -> Option<Work> {
        let listener = self.0.get_mut(source)?.next(index)?;
        Some(match listener {
            Listener::Predicate(i) => Work::RecomputePredicate(i, participant),
            Listener::Set(i) => Work::AssignSet(i, participant, value),
            Listener::Spawner(i) => Work::Spawner(i, participant, value),
            Listener::External(observer) => Work::BooleanCallback(Notification {
                observer,
                source: source.clone(),
                participant,
                value,
            }),
        })
    }
}

impl Plan {
    pub(super) fn validate_boolean(&self, source: &BooleanSource) -> Result<(), Error> {
        match source {
            BooleanSource::Local(i) if *i < self.data.bindings.properties.len() => Ok(()),
            BooleanSource::Predicate(i) if *i < self.data.bindings.predicates.len() => Ok(()),
            BooleanSource::Channel(key) => key.validate().map_err(|_| Error::Invalid),
            _ => Err(Error::Invalid),
        }
    }
}
impl Host {
    pub fn boolean_value(
        &self,
        source: &BooleanSource,
        participant: Participant,
    ) -> Result<Option<bool>, Error> {
        self.plan.validate_boolean(source)?;
        Ok(self.observers.stored_boolean(source, participant))
    }
}
impl Observers {
    pub(super) fn stored_boolean(
        &self,
        source: &BooleanSource,
        participant: Participant,
    ) -> Option<bool> {
        match source {
            BooleanSource::Local(i) => self.properties[*i].value(participant),
            BooleanSource::Predicate(i) => self.predicates[*i].get(&participant).copied(),
            BooleanSource::Channel(key) => self.booleans.get(&(key.clone(), participant)).copied(),
        }
    }
}

#[cfg(test)]
mod tests;
