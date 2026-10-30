// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bound schematic channels (04050ae0, 04054600, 04055680). References copy
//! ready properties on activation, then receive event/property notifications.
//! Native listener removal leaves tombstones, compacted by swapping the tail.
use super::graphs::{Reader, Step};
use super::listeners::List;
use nfs_world::scoring::{
    Error,
    events::{
        Context, Endpoint,
        native::{Call, Operation, Request},
    },
    interpreter::hosted::Action,
    properties::Port,
    value::Value,
};
use std::collections::{BTreeMap, BTreeSet};

pub mod install;

const MAX_CHANNELS: usize = 256;
const MAX_REFERENCES: usize = 1024;
const MAX_PENDING: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Location {
    pub instance: u64,
    pub node: u16,
}
impl Location {
    fn publish(self, field: u32, value: Value) -> Step {
        Step::Graph {
            id: self.instance,
            action: Action::Graph(Operation::Publish {
                port: Port {
                    node: self.node,
                    field,
                },
                value,
            }),
        }
    }
    fn emit(self, event: u32, context: Option<Context>) -> Step {
        Step::Graph {
            id: self.instance,
            action: Action::Graph(Operation::Emit {
                source: Endpoint {
                    node: self.node,
                    event,
                },
                context,
            }),
        }
    }
}

#[derive(Clone, Debug)]
struct Definition {
    /// Only property handles retained in the compiled graph. Preserve schema
    /// order and repeats; omitted/unbound handles have no value to seed.
    fields: Vec<u32>,
    listeners: List<Location>,
    /// Resource/lifetime guard, separate from the native Boolean iteration flag.
    busy: usize,
}
#[derive(Clone, Debug)]
struct Reference {
    source: Location,
    fields: BTreeSet<u32>,
    active: bool,
}
#[derive(Clone, Debug)]
enum Message {
    Property(u32, Value),
    Event(u32, Option<Context>),
}
#[derive(Clone, Debug)]
enum Work {
    Seed {
        source: Location,
        reference: Location,
        index: usize,
    },
    Notify {
        source: Location,
        index: usize,
        message: Message,
    },
}

#[derive(Clone, Debug)]
pub struct Router {
    /// Namespace assigned by the containing driver to continuation tokens.
    owner: u64,
    definitions: BTreeMap<Location, Definition>,
    references: BTreeMap<Location, Reference>,
    pending: BTreeMap<u64, Work>,
    next: u64,
}
impl Router {
    pub fn new(owner: u64) -> Self {
        Self {
            owner,
            definitions: BTreeMap::new(),
            references: BTreeMap::new(),
            pending: BTreeMap::new(),
            next: 0,
        }
    }
    pub fn definition(&mut self, location: Location, fields: Vec<u32>) -> Result<(), Error> {
        if location.instance == 0
            || self.definitions.contains_key(&location)
            || self.references.contains_key(&location)
        {
            return Err(Error::InvalidGraph);
        }
        if self.definitions.len() >= MAX_CHANNELS || fields.len() > 512 {
            return Err(Error::Bound);
        }
        if fields.contains(&0) {
            return Err(Error::InvalidGraph);
        }
        self.definitions.insert(
            location,
            Definition {
                fields,
                listeners: List::default(),
                busy: 0,
            },
        );
        Ok(())
    }
    pub fn reference(
        &mut self,
        location: Location,
        source: Location,
        fields: BTreeSet<u32>,
    ) -> Result<(), Error> {
        if location.instance == 0
            || self.definitions.contains_key(&location)
            || self.references.contains_key(&location)
            || !self.definitions.contains_key(&source)
        {
            return Err(Error::InvalidGraph);
        }
        if self.references.len() >= MAX_REFERENCES || fields.len() > 512 {
            return Err(Error::Bound);
        }
        if fields.contains(&0) {
            return Err(Error::InvalidGraph);
        }
        self.references.insert(
            location,
            Reference {
                source,
                fields,
                active: false,
            },
        );
        Ok(())
    }
    fn queue(&mut self, work: Work) -> Result<Step, Error> {
        if self.pending.len() >= MAX_PENDING {
            return Err(Error::Bound);
        }
        self.next = self.next.checked_add(1).ok_or(Error::Bound)?;
        self.pending.insert(self.next, work);
        Ok(Step::Continue {
            owner: self.owner,
            token: self.next,
        })
    }
    fn retire(&mut self, location: Location) -> Result<(), Error> {
        let reference = self
            .references
            .remove(&location)
            .ok_or(Error::UnknownNode)?;
        if !reference.active {
            return Ok(());
        }
        let definition = self
            .definitions
            .get_mut(&reference.source)
            .ok_or(Error::UnknownNode)?;
        definition.listeners.remove(location);
        Ok(())
    }
    /// None means this endpoint belongs to another typed native service.
    pub fn invoke(
        &mut self,
        request: &Request,
        values: &mut dyn Reader,
    ) -> Result<Option<Vec<Step>>, Error> {
        let location = Location {
            instance: request.instance,
            node: request.node,
        };
        if self.definitions.contains_key(&location) {
            let message = match &request.call {
                Call::Bind | Call::Activate => return Ok(Some(vec![])),
                Call::Property { field, .. } => {
                    if !self.definitions[&location].fields.contains(field) {
                        return Ok(Some(vec![]));
                    }
                    let Some(value) = values.read(
                        location.instance,
                        Port {
                            node: location.node,
                            field: *field,
                        },
                    )?
                    else {
                        return Ok(Some(vec![]));
                    };
                    Message::Property(*field, value)
                }
                Call::Event { event, context } => Message::Event(*event, *context),
                Call::Retire => {
                    let definition = &self.definitions[&location];
                    if definition.busy != 0
                        || definition.listeners.items.iter().any(Option::is_some)
                    {
                        return Err(Error::InvalidGraph);
                    }
                    self.definitions.remove(&location);
                    return Ok(Some(vec![]));
                }
                _ => return Err(Error::UnsupportedEvent),
            };
            let definition = self
                .definitions
                .get_mut(&location)
                .ok_or(Error::UnknownNode)?;
            if definition.busy >= 16 {
                return Err(Error::Bound);
            }
            definition.busy += 1;
            definition.listeners.notifying = true;
            return self
                .queue(Work::Notify {
                    source: location,
                    index: 0,
                    message,
                })
                .map(|step| Some(vec![step]));
        }
        let Some(reference) = self.references.get_mut(&location) else {
            return Ok(None);
        };
        Ok(Some(match request.call {
            Call::Bind | Call::Property { .. } | Call::Event { .. } => vec![],
            Call::Activate if reference.active => vec![],
            Call::Activate => {
                reference.active = true;
                let source = reference.source;
                let definition = self
                    .definitions
                    .get_mut(&source)
                    .ok_or(Error::UnknownNode)?;
                if definition.listeners.items.len() >= MAX_REFERENCES {
                    return Err(Error::Bound);
                }
                definition.listeners.items.push(Some(location));
                vec![self.queue(Work::Seed {
                    source,
                    reference: location,
                    index: 0,
                })?]
            }
            Call::Retire => {
                self.retire(location)?;
                vec![]
            }
            _ => return Err(Error::UnsupportedEvent),
        }))
    }
    /// Resume after the previous graph callback, inspecting current listeners
    /// rather than a stale snapshot. The enclosing Runtime owns rollback.
    pub fn resume(
        &mut self,
        owner: u64,
        token: u64,
        values: &mut dyn Reader,
    ) -> Result<Option<Vec<Step>>, Error> {
        if owner != self.owner {
            return Ok(None);
        }
        let work = self.pending.remove(&token).ok_or(Error::InvalidGraph)?;
        let mut steps = Vec::new();
        match work {
            Work::Seed {
                source,
                reference,
                mut index,
            } => {
                while let Some(&field) = self
                    .definitions
                    .get(&source)
                    .ok_or(Error::UnknownNode)?
                    .fields
                    .get(index)
                {
                    index += 1;
                    let Some(target) = self.references.get(&reference) else {
                        break;
                    };
                    if target.fields.contains(&field)
                        && let Some(value) = values.read(
                            source.instance,
                            Port {
                                node: source.node,
                                field,
                            },
                        )?
                    {
                        steps.push(reference.publish(field, value));
                        steps.push(self.queue(Work::Seed {
                            source,
                            reference,
                            index,
                        })?);
                        break;
                    }
                }
            }
            Work::Notify {
                source,
                mut index,
                message,
            } => {
                let definition = self
                    .definitions
                    .get_mut(&source)
                    .ok_or(Error::UnknownNode)?;
                loop {
                    let Some(reference) = definition.listeners.next(index) else {
                        definition.busy =
                            definition.busy.checked_sub(1).ok_or(Error::InvalidGraph)?;
                        break;
                    };
                    index += 1;
                    let target = self.references.get(&reference).ok_or(Error::InvalidGraph)?;
                    let step = match &message {
                        Message::Event(event, context) => Some(reference.emit(*event, *context)),
                        Message::Property(field, value) if target.fields.contains(field) => {
                            Some(reference.publish(*field, value.clone()))
                        }
                        _ => None,
                    };
                    if let Some(step) = step {
                        steps.push(step);
                        steps.push(self.queue(Work::Notify {
                            source,
                            index,
                            message,
                        })?);
                        break;
                    }
                }
            }
        }
        Ok(Some(steps))
    }
}

#[cfg(test)]
mod tests;
