// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native UpdateSet subscriptions, separate from participant membership sets.
//! 04742b00 subscribes at activation, 047460f0 visits the live vector, and
//! 0473ff40 detaches when the source is destroyed. The enclosing scheduler
//! supplies manager updates; this module does not guess a wall-clock interval.
use super::{
    Plan,
    commands::Command,
    dispatch::Bindings,
    graphs::Step,
    links::{Endpoint, Object, Registry},
    listeners::List,
};
use nfs_content::{
    events::lifecycle::{StateConfiguration, shared::Service},
    scoring::events::Identity,
};
use nfs_world::{
    participant_states::{Action, StateId},
    scoring::{
        Error,
        events::native::{Call, Request},
    },
};
use std::collections::{BTreeMap, BTreeSet};

const LIMIT: usize = 4096;
const STATE_SET: u32 = 0x5ba0_872d;
const MANAGER_SETS: u32 = 0x7644_d2cd;

#[derive(Clone, Debug, Default)]
struct Set {
    listeners: List<Object>,
    retired: bool,
    busy: usize,
}
#[derive(Clone, Debug)]
struct State {
    id: StateId,
    source: Option<Object>,
    active: bool,
}
#[derive(Clone, Debug)]
enum Work {
    Sets { objects: Vec<Object>, index: usize },
    Notify { source: Object, index: usize },
}
#[derive(Clone, Debug)]
pub struct Updates {
    owner: u64,
    next: u64,
    sets: BTreeMap<Object, Set>,
    retired_sets: BTreeSet<Object>,
    states: BTreeMap<Object, State>,
    managers: BTreeMap<Object, bool>,
    pending: BTreeMap<u64, Work>,
}
impl Updates {
    pub fn new(owner: u64) -> Result<Self, Error> {
        if owner == 0 {
            return Err(Error::InvalidGraph);
        }
        Ok(Self {
            owner,
            next: 0,
            sets: BTreeMap::new(),
            retired_sets: BTreeSet::new(),
            states: BTreeMap::new(),
            managers: BTreeMap::new(),
            pending: BTreeMap::new(),
        })
    }
    /// Register live occurrences without subscribing before graph activation.
    pub fn install(
        &mut self,
        instance: u64,
        plan: &Plan,
        serializers: &[Identity],
        services: &BTreeMap<Identity, Service>,
        links: &Registry,
        bindings: &Bindings,
    ) -> Result<(), Error> {
        let mut next = self.clone();
        let occupied: BTreeSet<_> = self
            .sets
            .keys()
            .chain(self.states.keys())
            .chain(self.managers.keys())
            .chain(self.retired_sets.iter())
            .copied()
            .collect();
        let mut added = BTreeSet::new();
        let mut add = |object| {
            if occupied.contains(&object) || !added.insert(object) {
                return Err(Error::InvalidGraph);
            }
            if occupied.len() + added.len() > LIMIT {
                return Err(Error::Bound);
            }
            Ok(())
        };
        for (identity, service) in services {
            if *service != Service::UpdateSet {
                continue;
            }
            let object = links.object(instance, identity)?;
            if links.node(object)?.class != "UpdateSetEntityData" {
                return Err(Error::InvalidGraph);
            }
            add(object)?;
            next.sets.insert(object, Set::default());
        }
        for definition in &plan.data.states {
            let StateConfiguration::Updateable { update_set } = &definition.configuration else {
                continue;
            };
            if plan
                .external_state(definition.serializer as StateId)
                .is_some()
            {
                continue;
            }
            let identity = serializers
                .get(definition.serializer)
                .ok_or(Error::UnknownNode)?;
            let object = links.object(instance, identity)?;
            // Derived classes can override the update condition; do not treat
            // them as unconditional completion without their own native model.
            if links.node(object)?.class != "UpdateableParticipantStateEntityData" {
                return Err(Error::UnsupportedEvent);
            }
            let source = links.object(instance, update_set)?;
            if links.input(Endpoint {
                object,
                port: STATE_SET,
            })? != [source]
                || !next.sets.contains_key(&source)
            {
                return Err(Error::InvalidGraph);
            }
            add(object)?;
            next.states.insert(
                object,
                State {
                    id: bindings.state(object)?,
                    source: Some(source),
                    active: false,
                },
            );
        }
        for definition in &plan.data.bindings.update_managers {
            let object = links.object(instance, &definition.identity)?;
            if links.node(object)?.class != "UpdateableManagerEntityData" {
                return Err(Error::InvalidGraph);
            }
            let expected = definition
                .sets
                .iter()
                .map(|i| links.object(instance, i))
                .collect::<Result<Vec<_>, _>>()?;
            if links.input(Endpoint {
                object,
                port: MANAGER_SETS,
            })? != expected
                || expected.iter().any(|s| !next.sets.contains_key(s))
            {
                return Err(Error::InvalidGraph);
            }
            add(object)?;
            next.managers.insert(object, false);
        }
        *self = next;
        Ok(())
    }
    fn queue(&mut self, work: Work) -> Result<Step, Error> {
        if self.pending.len() >= LIMIT {
            return Err(Error::Bound);
        }
        self.next = self.next.checked_add(1).ok_or(Error::Bound)?;
        self.pending.insert(self.next, work);
        Ok(Step::Continue {
            owner: self.owner,
            token: self.next,
        })
    }
    fn update_set(&mut self, source: Object) -> Result<Vec<Step>, Error> {
        let Some(set) = self.sets.get_mut(&source) else {
            // A source in an already-resolved manager list may have retired
            // during the preceding set's synchronous callback.
            return if self.retired_sets.contains(&source) {
                Ok(vec![])
            } else {
                Err(Error::UnknownNode)
            };
        };
        if set.retired {
            return Ok(vec![]);
        }
        set.listeners.notifying = true;
        set.busy = set.busy.checked_add(1).ok_or(Error::Bound)?;
        Ok(vec![self.queue(Work::Notify { source, index: 0 })?])
    }
    pub fn resume(&mut self, owner: u64, token: u64) -> Result<Option<Vec<Step>>, Error> {
        if owner != self.owner {
            return Ok(None);
        }
        let work = self.pending.remove(&token).ok_or(Error::InvalidGraph)?;
        let mut result = Vec::new();
        match work {
            Work::Sets { objects, index } => {
                if let Some(source) = objects.get(index) {
                    result.extend(self.update_set(*source)?);
                    result.push(self.queue(Work::Sets {
                        objects,
                        index: index + 1,
                    })?);
                }
            }
            Work::Notify { source, index } => {
                let set = self.sets.get_mut(&source).ok_or(Error::UnknownNode)?;
                if let Some(object) = set.listeners.next(index) {
                    let state = self.states.get(&object).ok_or(Error::UnknownNode)?;
                    result.push(Step::Lifecycle(vec![Command::State(Action::CompleteAll {
                        state: state.id,
                        only_incomplete: true,
                    })]));
                    result.push(self.queue(Work::Notify {
                        source,
                        index: index + 1,
                    })?);
                } else {
                    set.busy = set.busy.checked_sub(1).ok_or(Error::InvalidGraph)?;
                    if set.retired && set.busy == 0 {
                        self.sets.remove(&source);
                    }
                }
            }
        }
        Ok(Some(result))
    }
    /// Detach this component of an entity's deactivation. For participant
    /// states the enclosing owner must also execute base-state deactivation
    /// (membership removal/destruction observers); unsubscribing is not that.
    pub fn detach(&mut self, object: Object) -> Result<bool, Error> {
        if let Some(state) = self.states.remove(&object) {
            if state.active
                && let Some(source) = state.source
            {
                self.sets
                    .get_mut(&source)
                    .ok_or(Error::UnknownNode)?
                    .listeners
                    .remove(object);
            }
            return Ok(true);
        }
        if self.managers.remove(&object).is_some() {
            return Ok(true);
        }
        let Some(set) = self.sets.get_mut(&object) else {
            return Ok(self.retired_sets.contains(&object));
        };
        if set.retired {
            return Ok(true);
        }
        set.retired = true;
        self.retired_sets.insert(object);
        set.listeners.notifying = true;
        let mut index = 0;
        while let Some(listener) = set.listeners.next(index) {
            index += 1;
            self.states
                .get_mut(&listener)
                .ok_or(Error::UnknownNode)?
                .source = None;
            set.listeners.remove(listener);
        }
        if set.busy == 0 {
            self.sets.remove(&object);
        }
        Ok(true)
    }
    pub fn invoke(
        &mut self,
        request: &Request,
        links: &Registry,
    ) -> Result<Option<Vec<Step>>, Error> {
        let object = Object {
            instance: request.instance,
            node: request.node,
        };
        if let Some(state) = self.states.get_mut(&object) {
            return match request.call {
                Call::Bind => Ok(Some(vec![])),
                Call::Activate => {
                    if state.active {
                        return Err(Error::InvalidGraph);
                    }
                    let source = state.source.ok_or(Error::MissingInput)?;
                    let set = self.sets.get_mut(&source).ok_or(Error::UnknownNode)?;
                    if set.retired {
                        return Err(Error::InvalidGraph);
                    }
                    if set.listeners.items.len() >= LIMIT {
                        return Err(Error::Bound);
                    }
                    set.listeners.items.push(Some(object));
                    state.active = true;
                    Ok(Some(vec![]))
                }
                // Other participant-state behavior belongs to its base owner.
                _ => Ok(None),
            };
        }
        if let Some(active) = self.managers.get_mut(&object) {
            return Ok(Some(match request.call {
                Call::Bind => vec![],
                Call::Activate => {
                    *active = true;
                    vec![]
                }
                Call::Update if *active => {
                    // Native04751fd0 resolves these links at each update.
                    let objects = links.input(Endpoint {
                        object,
                        port: MANAGER_SETS,
                    })?;
                    if objects.len() > LIMIT {
                        return Err(Error::Bound);
                    }
                    vec![self.queue(Work::Sets { objects, index: 0 })?]
                }
                Call::Update => vec![],
                Call::Retire => {
                    self.detach(object)?;
                    vec![]
                }
                _ => return Err(Error::UnsupportedEvent),
            }));
        }
        if self.sets.contains_key(&object) {
            return Ok(Some(match request.call {
                Call::Bind | Call::Activate => vec![],
                Call::Update => self.update_set(object)?,
                Call::Retire => {
                    self.detach(object)?;
                    vec![]
                }
                _ => return Err(Error::UnsupportedEvent),
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
