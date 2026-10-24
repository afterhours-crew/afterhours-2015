// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Individual ParticipantSetAccessor callbacks, in shared-store listener order.
use super::{
    Effect, SetSource,
    commands::Command,
    graphs::{Reader, Step},
    links::{Endpoint, Object, Registry},
    sets::{Change, Subscription},
};
use nfs_world::scoring::{
    Error,
    events::native::{Call, Import, Operation, Request},
    interpreter::hosted::Action,
    participant_sets::{self, Key},
};
use std::collections::BTreeMap;

const MAX_ACCESSORS: usize = 4096;

#[derive(Clone, Debug)]
struct Binding {
    source: SetSource,
    key: Key,
    participant: u64,
    observer: u64,
}
#[derive(Clone, Debug)]
pub struct SetAccessors {
    namespace: u32,
    next: u32,
    sources: BTreeMap<Object, SetSource>,
    bindings: BTreeMap<Object, Binding>,
    observers: BTreeMap<u64, Object>,
}
impl SetAccessors {
    pub fn new(namespace: u32, sources: BTreeMap<Object, SetSource>) -> Result<Self, Error> {
        if namespace == 0 || sources.len() > MAX_ACCESSORS {
            return Err(Error::Bound);
        }
        Ok(Self {
            namespace,
            next: 0,
            sources,
            bindings: BTreeMap::new(),
            observers: BTreeMap::new(),
        })
    }
    fn observe(binding: &Binding, subscribe: bool) -> Command {
        Command::SetObserver {
            source: binding.source.clone(),
            subscription: Subscription::Membership,
            observer: binding.observer,
            subscribe,
        }
    }
    fn import(object: Object, binding: &Binding, member: bool) -> Step {
        Step::Graph {
            id: object.instance,
            action: Action::Graph(Operation::Import(Import::SetAccessor {
                node: object.node,
                change: participant_sets::Change {
                    set: binding.key.clone(),
                    participant: binding.participant,
                    member,
                },
            })),
        }
    }
    pub fn invoke(
        &mut self,
        request: &Request,
        links: &Registry,
        reader: &dyn Reader,
    ) -> Result<Option<Vec<Step>>, Error> {
        let object = Object {
            instance: request.instance,
            node: request.node,
        };
        match &request.call {
            Call::SetObserve { set, participant } => {
                set.validate()?;
                if self.bindings.contains_key(&object) {
                    return Err(Error::InvalidGraph);
                }
                if self.bindings.len() >= MAX_ACCESSORS {
                    return Err(Error::Bound);
                }
                let targets = links.input(Endpoint {
                    object,
                    port: 0x2e0d_19d8,
                })?;
                let [target] = targets.as_slice() else {
                    return Err(Error::InvalidGraph);
                };
                let source = self.sources.get(target).ok_or(Error::MissingInput)?.clone();
                let member = reader.members(&source)?.contains(participant);
                let next = self.next.checked_add(1).ok_or(Error::Bound)?;
                let observer = (u64::from(self.namespace) << 32) | u64::from(next);
                let binding = Binding {
                    source,
                    key: set.clone(),
                    participant: *participant,
                    observer,
                };
                let steps = vec![
                    Step::Lifecycle(vec![Self::observe(&binding, true)]),
                    Self::import(object, &binding, member),
                ];
                self.bindings.insert(object, binding);
                self.observers.insert(observer, object);
                self.next = next;
                Ok(Some(steps))
            }
            Call::SetChange(change) => {
                let binding = self.bindings.get(&object).ok_or(Error::MissingInput)?;
                if binding.key != change.set || binding.participant != change.participant {
                    return Err(Error::InvalidGraph);
                }
                Ok(Some(vec![Step::Lifecycle(vec![Command::Set {
                    source: binding.source.clone(),
                    participant: change.participant,
                    member: change.member,
                }])]))
            }
            Call::SetUnobserve => {
                let Some(binding) = self.bindings.remove(&object) else {
                    return Ok(None);
                };
                self.observers.remove(&binding.observer);
                Ok(Some(vec![Step::Lifecycle(vec![Self::observe(
                    &binding, false,
                )])]))
            }
            _ => Ok(None),
        }
    }
    pub fn effect(&self, effect: &Effect) -> Result<Option<Vec<Step>>, Error> {
        let Effect::SetCallback(notification) = effect else {
            return Ok(None);
        };
        let Some(object) = self.observers.get(&notification.observer) else {
            return Ok(None);
        };
        let binding = self.bindings.get(object).ok_or(Error::InvalidGraph)?;
        if binding.source != notification.source {
            return Err(Error::InvalidGraph);
        }
        let Change::Membership {
            participant,
            member,
        } = notification.change
        else {
            return Err(Error::InvalidGraph);
        };
        Ok(Some(if binding.participant == participant {
            vec![Self::import(*object, binding, member)]
        } else {
            vec![]
        }))
    }
}

#[cfg(test)]
mod tests;
