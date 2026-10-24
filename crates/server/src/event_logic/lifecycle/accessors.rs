// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scalar accessor subscriptions between participant graphs and the event's
//! shared stores. Each accessor joins at its native activation point. A callback
//! targets that accessor alone, preserving interleaved sorter/mirror order.
use super::{
    BooleanSource, Effect, IntegerSource,
    commands::Command,
    graphs::{Reader, Step},
    links::{Endpoint, Object, Registry},
};
use nfs_world::scoring::{
    Error,
    events::native::{Call, Import, Operation, Request},
    interpreter::hosted::Action,
    participant_properties::{Property, Write},
    properties::Kind,
    value::Value,
};
use std::collections::BTreeMap;

const MAX_ACCESSORS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    Integer(IntegerSource),
    Boolean(BooleanSource),
    Float(Property),
}
impl Source {
    fn kind(&self) -> Kind {
        match self {
            Self::Integer(_) => Kind::Integer,
            Self::Boolean(_) => Kind::Boolean,
            Self::Float(_) => Kind::Float,
        }
    }
    fn read(&self, reader: &dyn Reader, participant: u64) -> Result<Option<Value>, Error> {
        match self {
            Self::Float(source) => reader
                .float(source, participant)
                .map(|v| v.map(Value::Float)),
            Self::Integer(source) => reader
                .integer(source, participant)
                .map(|v| v.map(Value::Integer)),
            Self::Boolean(source) => reader
                .boolean(source, participant)
                .map(|v| v.map(Value::Boolean)),
        }
    }
    fn observe(&self, observer: u64, subscribe: bool) -> Command {
        match self {
            Self::Float(source) => Command::FloatObserver {
                source: source.clone(),
                observer,
                subscribe,
            },
            Self::Integer(source) => Command::IntegerObserver {
                source: source.clone(),
                observer,
                subscribe,
            },
            Self::Boolean(source) => Command::BooleanObserver {
                source: source.clone(),
                observer,
                subscribe,
            },
        }
    }
    fn write(&self, participant: u64, value: &Value) -> Result<Command, Error> {
        match (self, value) {
            (Self::Float(source), Value::Float(value)) => Ok(Command::Float {
                source: source.clone(),
                participant,
                value: *value,
            }),
            (Self::Integer(source), Value::Integer(value)) => Ok(Command::Integer {
                source: source.clone(),
                participant,
                value: *value,
            }),
            (Self::Boolean(BooleanSource::Channel(key)), Value::Boolean(value)) => {
                Ok(Command::Boolean {
                    key: key.clone(),
                    participant,
                    value: *value,
                })
            }
            _ => Err(Error::InvalidGraph),
        }
    }
}

#[derive(Clone, Debug)]
struct Binding {
    source: Source,
    property: Property,
    participant: u64,
    observer: u64,
}

#[derive(Clone, Debug)]
pub struct ParticipantAccessors {
    namespace: u32,
    next: u32,
    sources: BTreeMap<Object, Source>,
    bindings: BTreeMap<Object, Binding>,
    observers: BTreeMap<u64, Object>,
}
impl ParticipantAccessors {
    /// Namespace belongs to this component of the enclosing graph driver.
    /// Source mappings name real objects resolved from installed event bindings.
    pub fn new(namespace: u32, sources: BTreeMap<Object, Source>) -> Result<Self, Error> {
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
    fn source(&self, object: Object, kind: Kind, links: &Registry) -> Result<Source, Error> {
        let port = match kind {
            Kind::Integer => nfs_content::scoring::participant_properties::IDENTIFIER,
            Kind::Boolean => 0x969a_1637,
            Kind::Float => 0xf279_5629,
            _ => return Err(Error::InvalidGraph),
        };
        let targets = links.input(Endpoint { object, port })?;
        let [target] = targets.as_slice() else {
            return Err(Error::InvalidGraph);
        };
        let source = self.sources.get(target).ok_or(Error::MissingInput)?;
        if source.kind() != kind {
            return Err(Error::InvalidGraph);
        }
        Ok(source.clone())
    }
    fn import(object: Object, property: Property, participant: u64, value: Value) -> Step {
        Step::Graph {
            id: object.instance,
            action: Action::Graph(Operation::Import(Import::Accessor {
                node: object.node,
                write: Write {
                    participant,
                    property,
                    value,
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
            Call::ParticipantObserve {
                property,
                participant,
                kind,
                seed,
            } if matches!(kind, Kind::Integer | Kind::Boolean | Kind::Float) => {
                property.validate()?;
                if self.bindings.contains_key(&object) {
                    return Err(Error::InvalidGraph);
                }
                if self.bindings.len() >= MAX_ACCESSORS {
                    return Err(Error::Bound);
                }
                let source = self.source(object, *kind, links)?;
                let value = if *seed {
                    source.read(reader, *participant)?
                } else {
                    None
                };
                let next = self.next.checked_add(1).ok_or(Error::Bound)?;
                let observer = (u64::from(self.namespace) << 32) | u64::from(next);
                let mut steps = vec![Step::Lifecycle(vec![source.observe(observer, true)])];
                if let Some(value) = value {
                    steps.push(Self::import(object, property.clone(), *participant, value));
                }
                self.bindings.insert(
                    object,
                    Binding {
                        source,
                        property: property.clone(),
                        participant: *participant,
                        observer,
                    },
                );
                self.observers.insert(observer, object);
                self.next = next;
                Ok(Some(steps))
            }
            Call::ParticipantWrite(Write {
                participant,
                property,
                value,
            }) if matches!(
                value,
                Value::Integer(_) | Value::Boolean(_) | Value::Float(_)
            ) =>
            {
                let binding = self.bindings.get(&object).ok_or(Error::MissingInput)?;
                if binding.property != *property || binding.participant != *participant {
                    return Err(Error::InvalidGraph);
                }
                Ok(Some(vec![Step::Lifecycle(vec![
                    binding.source.write(*participant, value)?,
                ])]))
            }
            Call::ParticipantUnobserve => {
                let Some(binding) = self.bindings.remove(&object) else {
                    return Ok(None);
                };
                self.observers.remove(&binding.observer);
                Ok(Some(vec![Step::Lifecycle(vec![
                    binding.source.observe(binding.observer, false),
                ])]))
            }
            _ => Ok(None),
        }
    }
    pub fn effect(&self, effect: &Effect) -> Result<Option<Vec<Step>>, Error> {
        let (observer, source, participant, value) = match effect {
            Effect::Float(n) => (
                n.observer,
                Source::Float(n.source.clone()),
                n.participant,
                Value::Float(n.value),
            ),
            Effect::Integer(n) => (
                n.observer,
                Source::Integer(n.source.clone()),
                n.participant,
                Value::Integer(n.value),
            ),
            Effect::BooleanCallback(n) => (
                n.observer,
                Source::Boolean(n.source.clone()),
                n.participant,
                Value::Boolean(n.value),
            ),
            _ => return Ok(None),
        };
        let Some(object) = self.observers.get(&observer) else {
            return Ok(None);
        };
        let binding = self.bindings.get(object).ok_or(Error::InvalidGraph)?;
        if binding.source != source {
            return Err(Error::InvalidGraph);
        }
        Ok(Some(if participant == binding.participant {
            vec![Self::import(
                *object,
                binding.property.clone(),
                participant,
                value,
            )]
        } else {
            vec![]
        }))
    }
}

#[cfg(test)]
mod tests;
