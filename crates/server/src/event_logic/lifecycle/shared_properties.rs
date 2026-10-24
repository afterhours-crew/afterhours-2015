// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared-mode Boolean accessors select a participant through an event provider,
//! rather than the owner of a spawned participant graph. Native 04782ac0,
//! 0472ca70, 046dae50, 046dc490 and 046dc940 define their synchronous callbacks.
use super::{
    BooleanSource, Effect,
    commands::Command,
    graphs::{Reader, Step},
    links::{Endpoint, Object, Registry},
    listeners::List,
};
use nfs_content::{events::lifecycle::shared::Service, scoring::events::Identity};
use nfs_world::scoring::{
    Error,
    events::{
        Context,
        native::{Call, Operation, Request},
    },
    interpreter::hosted::Action,
    nodes::{FLOAT_VALUE, OUTPUT_VALUE},
    program::Program,
    properties::Port,
    value::Value,
};
use std::collections::BTreeMap;

const MAX_OBJECTS: usize = 4096;
/// ParticipantEventToParticipantProviderEntityData.In, 04782ac0.
pub const IN: u32 = 0x0059_7302;

#[derive(Clone, Debug)]
struct Provider {
    retired: bool,
    participant: Option<u64>,
    listeners: List<Object>,
    // Resource lifetime guard, distinct from the native Boolean iteration flag.
    busy: usize,
}

#[derive(Clone, Debug)]
struct Accessor {
    provider: Option<Object>,
    source: Option<BooleanSource>,
    default: bool,
    input_bound: bool,
    output_bound: bool,
    observer: Option<u64>,
}

#[derive(Clone, Debug)]
struct Notify {
    provider: Object,
    participant: u64,
    index: usize,
}

#[derive(Clone, Debug)]
pub struct SharedProperties {
    namespace: u32,
    next_observer: u32,
    next_token: u64,
    providers: BTreeMap<Object, Provider>,
    accessors: BTreeMap<Object, Accessor>,
    observers: BTreeMap<u64, Object>,
    pending: BTreeMap<u64, Notify>,
}

impl SharedProperties {
    /// Reserve this namespace for both observer IDs and continuation ownership
    /// in the containing Driver. Source maps identify live objects, not assets.
    pub fn new(namespace: u32) -> Result<Self, Error> {
        if namespace == 0 {
            return Err(Error::InvalidGraph);
        }
        Ok(Self {
            namespace,
            next_observer: 0,
            next_token: 0,
            providers: BTreeMap::new(),
            accessors: BTreeMap::new(),
            observers: BTreeMap::new(),
            pending: BTreeMap::new(),
        })
    }

    /// Register definitions without activating listeners. The compiled graph's
    /// native activation steps determine subscription and seeding order.
    pub fn install(
        &mut self,
        instance: u64,
        services: &BTreeMap<Identity, Service>,
        program: &Program,
        links: &Registry,
        sources: &BTreeMap<Object, BooleanSource>,
    ) -> Result<(), Error> {
        let mut next = self.clone();
        for (identity, service) in services {
            if !matches!(
                service,
                Service::ParticipantEventProvider | Service::BooleanAccessor { .. }
            ) {
                continue;
            }
            let object = links.object(instance, identity)?;
            if next.providers.contains_key(&object) || next.accessors.contains_key(&object) {
                return Err(Error::InvalidGraph);
            }
            if next.providers.len() + next.accessors.len() >= MAX_OBJECTS {
                return Err(Error::Bound);
            }
            match service {
                Service::ParticipantEventProvider => {
                    next.providers.insert(
                        object,
                        Provider {
                            retired: false,
                            participant: None,
                            listeners: List::default(),
                            busy: 0,
                        },
                    );
                }
                Service::BooleanAccessor {
                    source,
                    provider,
                    default,
                } => {
                    source.validate()?;
                    let provider = links.object(instance, provider)?;
                    if links.input(Endpoint {
                        object,
                        port: 0xb68b_0dbb,
                    })? != [provider]
                    {
                        return Err(Error::InvalidGraph);
                    }
                    let targets = links.input(Endpoint {
                        object,
                        port: 0x969a_1637,
                    })?;
                    let [target] = targets.as_slice() else {
                        return Err(Error::InvalidGraph);
                    };
                    let source = sources.get(target).ok_or(Error::MissingInput)?.clone();
                    let bound = |field| {
                        program.cells.iter().any(|cell| {
                            cell.ports.contains(&Port {
                                node: object.node,
                                field,
                            })
                        })
                    };
                    next.accessors.insert(
                        object,
                        Accessor {
                            provider: Some(provider),
                            source: Some(source),
                            default: *default,
                            input_bound: bound(FLOAT_VALUE),
                            output_bound: bound(OUTPUT_VALUE),
                            observer: None,
                        },
                    );
                }
                _ => unreachable!(),
            }
        }
        if next
            .accessors
            .values()
            .any(|a| a.provider.is_some_and(|p| !next.providers.contains_key(&p)))
        {
            return Err(Error::MissingInput);
        }
        *self = next;
        Ok(())
    }

    pub fn participant(&self, provider: Object) -> Result<Option<u64>, Error> {
        Ok(self
            .providers
            .get(&provider)
            .ok_or(Error::UnknownNode)?
            .participant)
    }

    fn publish(object: Object, value: bool) -> Step {
        Step::Graph {
            id: object.instance,
            action: Action::Graph(Operation::Publish {
                port: Port {
                    node: object.node,
                    field: OUTPUT_VALUE,
                },
                value: Value::Boolean(value),
            }),
        }
    }

    fn input(
        object: Object,
        accessor: &Accessor,
        reader: &mut dyn Reader,
    ) -> Result<Option<bool>, Error> {
        if !accessor.input_bound {
            return Ok(None);
        }
        match reader.read(
            object.instance,
            Port {
                node: object.node,
                field: FLOAT_VALUE,
            },
        )? {
            None => Ok(None),
            Some(Value::Boolean(value)) => Ok(Some(value)),
            _ => Err(Error::InvalidGraph),
        }
    }

    fn write(source: &BooleanSource, participant: u64, value: bool) -> Result<Step, Error> {
        let BooleanSource::Channel(key) = source else {
            return Err(Error::InvalidGraph);
        };
        Ok(Step::Lifecycle(vec![Command::Boolean {
            key: key.clone(),
            participant,
            value,
        }]))
    }

    /// The provider callback uses its captured participant, even if an earlier
    /// listener has synchronously changed the provider again.
    fn refresh(
        &self,
        object: Object,
        participant: u64,
        reader: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        let accessor = self.accessors.get(&object).ok_or(Error::UnknownNode)?;
        let Some(source) = &accessor.source else {
            return Ok(vec![]);
        };
        if let Some(value) = Self::input(object, accessor, reader)? {
            Ok(vec![Self::write(source, participant, value)?])
        } else if accessor.output_bound
            && let Some(value) = reader.boolean(source, participant)?
        {
            Ok(vec![Self::publish(object, value)])
        } else {
            Ok(vec![])
        }
    }

    fn queue(&mut self, work: Notify) -> Result<Step, Error> {
        if self.pending.len() >= MAX_OBJECTS {
            return Err(Error::Bound);
        }
        self.next_token = self.next_token.checked_add(1).ok_or(Error::Bound)?;
        self.pending.insert(self.next_token, work);
        Ok(Step::Continue {
            owner: u64::from(self.namespace),
            token: self.next_token,
        })
    }

    pub fn resume(
        &mut self,
        owner: u64,
        token: u64,
        reader: &mut dyn Reader,
    ) -> Result<Option<Vec<Step>>, Error> {
        if owner != u64::from(self.namespace) {
            return Ok(None);
        }
        let mut work = self.pending.remove(&token).ok_or(Error::InvalidGraph)?;
        let provider = self
            .providers
            .get_mut(&work.provider)
            .ok_or(Error::UnknownNode)?;
        let Some(object) = provider.listeners.next(work.index) else {
            provider.busy = provider.busy.checked_sub(1).ok_or(Error::InvalidGraph)?;
            if provider.retired && provider.busy == 0 {
                self.providers.remove(&work.provider);
            }
            return Ok(Some(vec![]));
        };
        work.index += 1;
        let mut steps = self.refresh(object, work.participant, reader)?;
        steps.push(self.queue(work)?);
        Ok(Some(steps))
    }

    /// 04783d20 clears the selected participant without a provider-change event.
    pub fn participant_destroyed(&mut self, participant: u64) {
        for provider in self.providers.values_mut() {
            if provider.participant == Some(participant) {
                provider.participant = None;
            }
        }
    }

    pub fn invoke(
        &mut self,
        request: &Request,
        reader: &mut dyn Reader,
    ) -> Result<Option<Vec<Step>>, Error> {
        let object = Object {
            instance: request.instance,
            node: request.node,
        };
        if let Some(provider) = self.providers.get_mut(&object) {
            if provider.retired {
                return Ok(Some(vec![]));
            }
            return Ok(Some(match request.call {
                Call::Event {
                    event: IN,
                    context: Some(Context::Participant(participant)),
                } if provider.participant != Some(participant) => {
                    provider.participant = Some(participant);
                    provider.listeners.notifying = true;
                    provider.busy = provider.busy.checked_add(1).ok_or(Error::Bound)?;
                    vec![self.queue(Notify {
                        provider: object,
                        participant,
                        index: 0,
                    })?]
                }
                Call::Retire => {
                    provider.retired = true;
                    provider.participant = None;
                    provider.listeners.notifying = true;
                    let mut index = 0;
                    while let Some(listener) = provider.listeners.next(index) {
                        index += 1;
                        self.accessors
                            .get_mut(&listener)
                            .ok_or(Error::UnknownNode)?
                            .provider = None;
                        provider.listeners.remove(listener);
                    }
                    if provider.busy == 0 {
                        self.providers.remove(&object);
                    }
                    vec![]
                }
                Call::Bind | Call::Activate | Call::Event { .. } | Call::Property { .. } => vec![],
                _ => return Err(Error::UnsupportedEvent),
            }));
        }
        let Some(accessor) = self.accessors.get_mut(&object) else {
            return Ok(None);
        };
        Ok(Some(match request.call {
            Call::Activate => {
                if accessor.observer.is_some() {
                    return Err(Error::InvalidGraph);
                }
                let provider = self
                    .providers
                    .get_mut(&accessor.provider.ok_or(Error::MissingInput)?)
                    .ok_or(Error::UnknownNode)?;
                if provider.listeners.items.len() >= MAX_OBJECTS {
                    return Err(Error::Bound);
                }
                self.next_observer = self.next_observer.checked_add(1).ok_or(Error::Bound)?;
                let observer = (u64::from(self.namespace) << 32) | u64::from(self.next_observer);
                provider.listeners.items.push(Some(object));
                accessor.observer = Some(observer);
                self.observers.insert(observer, object);
                let mut steps = vec![Step::Lifecycle(vec![Command::BooleanObserver {
                    source: accessor.source.clone().ok_or(Error::MissingInput)?,
                    observer,
                    subscribe: true,
                }])];
                if let Some(participant) = provider.participant {
                    steps.extend(self.refresh(object, participant, reader)?);
                }
                steps
            }
            Call::Property {
                field: FLOAT_VALUE, ..
            } => {
                if let (Some(provider), Some(source)) = (accessor.provider, &accessor.source)
                    && let Some(participant) = self
                        .providers
                        .get(&provider)
                        .ok_or(Error::UnknownNode)?
                        .participant
                {
                    let value = Self::input(object, accessor, reader)?.unwrap_or(accessor.default);
                    vec![Self::write(source, participant, value)?]
                } else {
                    vec![]
                }
            }
            Call::Retire => {
                let accessor = self.accessors.remove(&object).ok_or(Error::UnknownNode)?;
                if let Some(observer) = accessor.observer {
                    self.observers.remove(&observer);
                    if let Some(provider) = accessor.provider {
                        self.providers
                            .get_mut(&provider)
                            .ok_or(Error::UnknownNode)?
                            .listeners
                            .remove(object);
                    }
                    accessor
                        .source
                        .map(|source| {
                            vec![Step::Lifecycle(vec![Command::BooleanObserver {
                                source,
                                observer,
                                subscribe: false,
                            }])]
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                }
            }
            Call::Bind | Call::Event { .. } | Call::Property { .. } => vec![],
            _ => return Err(Error::UnsupportedEvent),
        }))
    }

    pub fn effect(&self, effect: &Effect) -> Result<Option<Vec<Step>>, Error> {
        let Effect::BooleanCallback(notification) = effect else {
            return Ok(None);
        };
        let Some(object) = self.observers.get(&notification.observer) else {
            return Ok(None);
        };
        let accessor = self.accessors.get(object).ok_or(Error::UnknownNode)?;
        if accessor.source.as_ref() != Some(&notification.source) {
            return Err(Error::InvalidGraph);
        }
        // Source callbacks filter against the provider now, not the participant
        // captured by an outer provider-change notification.
        let current = accessor
            .provider
            .map(|provider| self.participant(provider))
            .transpose()?
            .flatten();
        Ok(Some(
            if accessor.output_bound && current == Some(notification.participant) {
                vec![Self::publish(*object, notification.value)]
            } else {
                vec![]
            },
        ))
    }
}

#[cfg(test)]
mod tests;
