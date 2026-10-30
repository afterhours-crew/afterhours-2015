// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-set observers (0405cae0/0405cf70, 0405e0b0/0405e160).
//! Membership and empty notifications have independent live listener lists.
use super::{listeners::List, *};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Subscription {
    Membership,
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Membership {
        participant: Participant,
        member: bool,
    },
    Empty,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub observer: u64,
    pub source: SetSource,
    pub change: Change,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Listener {
    Counter(usize),
    Predicate(usize),
    Spawner(usize),
    External(u64),
}

#[derive(Clone, Debug, Default)]
pub(super) struct Listeners(BTreeMap<(SetSource, Subscription), List<Listener>>);
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
    fn add(
        &mut self,
        source: SetSource,
        subscription: Subscription,
        listener: Listener,
    ) -> Result<(), Error> {
        let key = (source, subscription);
        if self.0.values().map(|list| list.items.len()).sum::<usize>() >= states::MAX_PENDING
            || (self.0.len() >= 512 && !self.0.contains_key(&key))
        {
            return Err(Error::Bound);
        }
        self.0.entry(key).or_default().items.push(Some(listener));
        Ok(())
    }
    pub fn external(
        &mut self,
        source: SetSource,
        subscription: Subscription,
        observer: u64,
        subscribe: bool,
    ) -> Result<(), Error> {
        if observer == 0 {
            return Err(Error::Invalid);
        }
        if subscribe {
            self.add(source, subscription, Listener::External(observer))
        } else {
            if let Some(list) = self.0.get_mut(&(source, subscription)) {
                list.remove(Listener::External(observer));
            }
            Ok(())
        }
    }
    pub fn activate(&mut self, plan: &Plan, observer: Observer) -> Result<(), Error> {
        match observer {
            Observer::SetCounter(i) => {
                if let Some(source) = &plan.data.bindings.set_counters[i].source {
                    self.add(
                        source.clone(),
                        Subscription::Membership,
                        Listener::Counter(i),
                    )?;
                }
            }
            Observer::Set(i) => {
                if let SetKind::Predicate { operands, .. } = &plan.data.bindings.sets[i].kind {
                    // 040548f0 retains repeated operand slots, but subscribes once
                    // per distinct source. Preserve first-occurrence order here.
                    let mut seen = BTreeSet::new();
                    for source in operands {
                        if seen.insert(source) {
                            self.add(
                                source.clone(),
                                Subscription::Membership,
                                Listener::Predicate(i),
                            )?;
                        }
                    }
                }
            }
            Observer::Spawner(i) => {
                if let Some(Controller::Set(link)) = &plan.data.bindings.spawners[i].controller {
                    self.add(
                        plan.set_source(link)?,
                        Subscription::Membership,
                        Listener::Spawner(i),
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn begin(&mut self, source: &SetSource, subscription: Subscription) {
        if let Some(list) = self.0.get_mut(&(source.clone(), subscription)) {
            list.notifying = true;
        }
    }
    pub fn next(&mut self, source: &SetSource, change: Change, index: usize) -> Option<Work> {
        let subscription = match change {
            Change::Membership { .. } => Subscription::Membership,
            Change::Empty => Subscription::Empty,
        };
        let listener = self
            .0
            .get_mut(&(source.clone(), subscription))?
            .next(index)?;
        Some(match (listener, change) {
            (Listener::Counter(i), _) => Work::SetCount(i),
            (Listener::Predicate(i), Change::Membership { participant, .. }) => {
                Work::Recompute(i, participant)
            }
            (
                Listener::Spawner(i),
                Change::Membership {
                    participant,
                    member,
                },
            ) => Work::Spawner(i, participant, member),
            (Listener::External(observer), _) => Work::SetCallback(Notification {
                observer,
                source: source.clone(),
                change,
            }),
            // Internal listeners are registered only on the membership list.
            _ => unreachable!("internal observer on an empty-list subscription"),
        })
    }
}

impl Host {
    pub fn source_members(&self, source: &SetSource) -> Result<BTreeSet<Participant>, Error> {
        self.plan.validate_set(source)?;
        Ok(self
            .observers
            .source_members(&self.plan, source)
            .cloned()
            .unwrap_or_default())
    }
}

impl Observers {
    pub(super) fn copy_set(
        &mut self,
        plan: &Plan,
        identity: &Identity,
        event: u32,
    ) -> Result<Vec<Action>, Error> {
        let i = *plan.sets.get(identity).ok_or(Error::Invalid)?;
        let SetKind::Copy { source } = &plan.data.bindings.sets[i].kind else {
            return Err(Error::Invalid);
        };
        let clear = event == 0x0cac_9f9c;
        let desired = match event {
            0x7c81_3ec0 => {
                // The native source pointer is acquired at activation. Copy
                // with no source leaves even accessor-written members intact.
                if !self.enabled.contains(&Observer::Set(i)) {
                    return Ok(vec![]);
                }
                let Some(source) = source else {
                    return Ok(vec![]);
                };
                self.source_members(plan, source)
                    .cloned()
                    .unwrap_or_default()
            }
            0x0cac_9f9c => BTreeSet::new(),
            _ => return Ok(vec![]),
        };
        if desired.len() > states::MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        let source = SetSource::Local(identity.clone());
        let mut work: Vec<_> = desired
            .difference(&self.sets[i])
            .map(|&p| Work::SetDelta(source.clone(), p, true))
            .chain(
                self.sets[i]
                    .difference(&desired)
                    .map(|&p| Work::SetDelta(source.clone(), p, false)),
            )
            .collect();
        // Publish only after the complete replacement, additions before
        // removals. Copy never emits Empty. Clear emits it even if the set
        // was already empty or a removal callback adds a member back.
        self.sets[i] = desired;
        if clear {
            work.push(Work::SetEmpty(source));
        }
        self.enqueue(work)
    }
}

#[cfg(test)]
mod copy_tests;
#[cfg(test)]
mod tests;
