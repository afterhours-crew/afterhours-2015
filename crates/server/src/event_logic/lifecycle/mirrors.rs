// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Event-owned mirror registry. Subscription changes share the state machine's
//! continuation queue, so copied values finish notifying before binding completes.
use super::*;
pub use data::bindings::mirrors::Manager;

pub const BOUND: u32 = 0x3c3b_59d3;

/// A spawned graph's provider must include its instance, not only its blueprint.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ProviderId {
    pub instance: u64,
    pub identity: Identity,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provider {
    pub id: ProviderId,
    pub manager: Manager,
    pub identifier: u32,
    pub source: IntegerSource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub identity: Identity,
    pub bound: bool,
}

#[derive(Clone, Debug)]
pub(super) enum Work {
    Activate(usize),
    Register(Provider),
    Bind(usize, IntegerSource),
    Subscribe(usize, IntegerSource),
    Forward(usize, IntegerSource, Participant, i32),
    Unbind(usize, IntegerSource),
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    // Native registry removal leaves a hole; queries compact it with the last
    // entry. Retaining holes preserves first-match selection after removals.
    providers: BTreeMap<Manager, Vec<Option<Provider>>>,
    bound: BTreeMap<usize, IntegerSource>,
    active: Vec<usize>,
    dead: BTreeSet<Manager>,
}

impl Plan {
    pub(super) fn validate_mirror_manager(&self, manager: &Manager) -> Result<(), Error> {
        match manager {
            Manager::Local(id) if self.data.bindings.mirror_managers.contains(id) => Ok(()),
            Manager::Channel(key) => key.validate().map_err(|_| Error::Invalid),
            _ => Err(Error::Invalid),
        }
    }
}

impl Host {
    pub fn mirror_source(&self, index: usize) -> Option<&IntegerSource> {
        self.observers.mirrors.bound.get(&index)
    }

    pub fn destroy_integer_source(
        &mut self,
        source: &IntegerSource,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        self.plan.validate_integer(source)?;
        let mut next = self.clone();
        if next.observers.rankings.dead.contains(source) {
            return Ok(vec![]);
        }
        if next.observers.rankings.dead.len() >= states::MAX_PENDING {
            return Err(Error::Bound);
        }
        next.observers.rankings.dead.insert(source.clone());
        next.observers.rankings.retire_listeners(source)?;
        let ids = next
            .observers
            .mirrors
            .providers
            .values()
            .flatten()
            .flatten()
            .filter(|p| &p.source == source)
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        for id in ids {
            output.extend(next.unregister_mirror_provider(&id, callback)?);
            if output.len() > states::MAX_STEPS {
                return Err(Error::Bound);
            }
        }
        let work = next
            .observers
            .mirrors
            .bound
            .iter()
            .filter(|(_, key)| *key == source)
            .map(|(&i, key)| super::Work::Mirror(Work::Unbind(i, key.clone())))
            .collect::<Vec<_>>();
        let actions = next.observers.enqueue(work)?;
        output.extend(next.execute(&actions, callback)?);
        next.observers
            .rankings
            .values
            .retain(|(key, _), _| key != source);
        *self = next;
        Ok(output)
    }

    pub fn register_mirror_provider(
        &mut self,
        provider: Provider,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        self.plan.validate_mirror_manager(&provider.manager)?;
        self.plan.validate_integer(&provider.source)?;
        let mut next = self.clone();
        let actions = next
            .observers
            .enqueue([super::Work::Mirror(Work::Register(provider))])?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    /// Unregistering does not query for another provider, even if one already
    /// has the same identifier. A later registration can bind an empty mirror.
    pub fn unregister_mirror_provider(
        &mut self,
        id: &ProviderId,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        let mut next = self.clone();
        let mut found = None;
        for rows in next.observers.mirrors.providers.values_mut() {
            for row in rows {
                if row.as_ref().is_some_and(|p| &p.id == id) {
                    found = row.take();
                    break;
                }
            }
        }
        let Some(provider) = found else {
            return Ok(vec![]);
        };
        let work = next
            .observers
            .mirrors
            .active
            .iter()
            .filter_map(|&i| {
                let mirror = &next.plan.data.bindings.mirrors[i];
                (mirror.manager.as_ref() == Some(&provider.manager)
                    && mirror.identifier == provider.identifier)
                    .then(|| super::Work::Mirror(Work::Unbind(i, provider.source.clone())))
            })
            .collect::<Vec<_>>();
        let actions = next.observers.enqueue(work)?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }

    pub fn destroy_mirror_manager(
        &mut self,
        manager: &Manager,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        self.plan.validate_mirror_manager(manager)?;
        let mut next = self.clone();
        if next.observers.mirrors.dead.len() >= 512
            && !next.observers.mirrors.dead.contains(manager)
        {
            return Err(Error::Bound);
        }
        if !next.observers.mirrors.dead.insert(manager.clone()) {
            return Ok(vec![]);
        }
        next.observers.mirrors.providers.remove(manager);
        let work = next
            .observers
            .mirrors
            .active
            .iter()
            .filter_map(|&i| {
                if next.plan.data.bindings.mirrors[i].manager.as_ref() != Some(manager) {
                    return None;
                }
                next.observers
                    .mirrors
                    .bound
                    .get(&i)
                    .cloned()
                    .map(|s| super::Work::Mirror(Work::Unbind(i, s)))
            })
            .collect::<Vec<_>>();
        let actions = next.observers.enqueue(work)?;
        let output = next.execute(&actions, callback)?;
        *self = next;
        Ok(output)
    }
}

#[cfg(test)]
mod tests;

impl Observers {
    pub(super) fn mirror_work(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<super::Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        match work {
            Work::Register(provider) => {
                if self.mirrors.dead.contains(&provider.manager)
                    || self.rankings.dead.contains(&provider.source)
                {
                    return Err(Error::Invalid);
                }
                if self
                    .mirrors
                    .providers
                    .values()
                    .flatten()
                    .flatten()
                    .any(|p| p.id == provider.id)
                {
                    return Err(Error::Invalid);
                }
                if self.mirrors.providers.values().map(Vec::len).sum::<usize>()
                    >= states::MAX_PENDING
                {
                    return Err(Error::Bound);
                }
                self.mirrors
                    .providers
                    .entry(provider.manager.clone())
                    .or_default()
                    .push(Some(provider.clone()));
                self.enqueue(
                    self.mirrors
                        .active
                        .iter()
                        .filter_map(|&i| {
                            let mirror = &plan.data.bindings.mirrors[i];
                            (mirror.manager.as_ref() == Some(&provider.manager)
                                && mirror.identifier == provider.identifier
                                && !self.mirrors.bound.contains_key(&i))
                            .then(|| super::Work::Mirror(Work::Bind(i, provider.source.clone())))
                        })
                        .collect::<Vec<_>>(),
                )
            }
            Work::Activate(i) => {
                if self.mirrors.active.contains(&i) {
                    return Err(Error::Invalid);
                }
                self.mirrors.active.push(i);
                let mirror = &plan.data.bindings.mirrors[i];
                let mut source = None;
                if let Some(manager) = &mirror.manager {
                    if self.mirrors.dead.contains(manager) {
                        return Err(Error::Invalid);
                    }
                    if let Some(providers) = self.mirrors.providers.get_mut(manager) {
                        let mut cursor = 0;
                        while cursor < providers.len() {
                            if let Some(provider) = &providers[cursor] {
                                if provider.identifier == mirror.identifier && source.is_none() {
                                    source = Some(provider.source.clone());
                                }
                                cursor += 1;
                            } else {
                                providers.swap_remove(cursor);
                            }
                        }
                    }
                }
                if let Some(source) = source {
                    self.enqueue([super::Work::Mirror(Work::Bind(i, source))])
                } else {
                    emit(
                        machine,
                        super::Effect::Mirror(Effect {
                            identity: mirror.identity.clone(),
                            bound: false,
                        }),
                        effects,
                        callback,
                    )
                }
            }
            Work::Bind(i, source) => {
                if self.mirrors.bound.contains_key(&i) {
                    return Ok(vec![]);
                }
                self.mirrors.bound.insert(i, source.clone());
                let destination =
                    IntegerSource::Local(plan.data.bindings.mirrors[i].identity.clone());
                let mut work = self
                    .rankings
                    .values
                    .iter()
                    .filter(|((key, _), _)| key == &source)
                    .map(|((_, participant), value)| {
                        super::Work::Ranking(rankings::Work::Write {
                            source: destination.clone(),
                            participant: *participant,
                            value: *value,
                        })
                    })
                    .collect::<Vec<_>>();
                work.push(super::Work::Mirror(Work::Subscribe(i, source)));
                self.enqueue(work)
            }
            Work::Subscribe(i, source) => {
                if self.mirrors.bound.get(&i) != Some(&source) {
                    return Ok(vec![]);
                }
                self.rankings
                    .subscribe(source, rankings::Listener::Mirror(i))?;
                emit(
                    machine,
                    super::Effect::Mirror(Effect {
                        identity: plan.data.bindings.mirrors[i].identity.clone(),
                        bound: true,
                    }),
                    effects,
                    callback,
                )
            }
            Work::Forward(i, source, participant, value) => {
                if self.mirrors.bound.get(&i) != Some(&source) {
                    return Ok(vec![]);
                }
                self.enqueue([super::Work::Ranking(rankings::Work::Write {
                    source: IntegerSource::Local(plan.data.bindings.mirrors[i].identity.clone()),
                    participant,
                    value,
                })])
            }
            Work::Unbind(i, source) => {
                if self.mirrors.bound.get(&i) != Some(&source) {
                    return Ok(vec![]);
                }
                self.mirrors.bound.remove(&i);
                self.rankings.unsubscribe_mirror(i);
                let destination =
                    IntegerSource::Local(plan.data.bindings.mirrors[i].identity.clone());
                self.rankings
                    .values
                    .retain(|(key, _), _| *key != destination);
                // Property clear calls observer slot +10. Both mirror and sorter
                // implement that slot as a no-op; participant destruction is a
                // separate notification which removes the actual ranking row.
                emit(
                    machine,
                    super::Effect::Mirror(Effect {
                        identity: plan.data.bindings.mirrors[i].identity.clone(),
                        bound: false,
                    }),
                    effects,
                    callback,
                )
            }
        }
    }
}
