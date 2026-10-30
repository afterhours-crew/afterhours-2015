// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synchronous native-host calls from ordinary graph execution. Both the host
//! and graph must be private candidates; no network or persistent I/O occurs.
use super::super::properties::Kind;
use super::*;
use std::{cell::RefCell, rc::Rc};

// A native boundary includes interpreter, host and callback routing frames.
// Bound that stack separately from ordinary event propagation.
const MAX_NATIVE_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    ParticipantObserve {
        property: super::super::participant_properties::Property,
        participant: u64,
        kind: Kind,
        seed: bool,
    },
    ParticipantUnobserve,
    ParticipantWrite(super::super::participant_properties::Write),
    SetObserve {
        set: super::super::participant_sets::Key,
        participant: u64,
    },
    SetUnobserve,
    SetChange(super::super::participant_sets::Change),
    Transition(super::super::transitions::Request),
    ChannelEvent(super::super::transitions::ChannelEvent),
    Bind,
    Activate,
    /// The enclosing owner scheduled this native entity's update callback.
    /// This is an internal call, not a schematic event or network opcode.
    Update,
    Event {
        event: u32,
        context: Option<Context>,
    },
    Property {
        field: u32,
        first: bool,
    },
    Retire,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub instance: u64,
    pub node: u16,
    pub call: Call,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Synchronous entity deactivation; storage remains owned by the caller
    /// until every suspended graph invocation has returned.
    Retire,
    Update {
        node: u16,
    },
    Import(Import),
    Publish {
        port: Port,
        value: Value,
    },
    Emit {
        source: Endpoint,
        context: Option<Context>,
    },
    Deliver {
        target: Endpoint,
        context: Option<Context>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Import {
    /// Deliver exactly one native accessor callback, without broadcasting to
    /// other accessors that subscribed at different positions in the host list.
    Accessor {
        node: u16,
        write: super::super::participant_properties::Write,
    },
    Participant(super::super::participant_properties::Write),
    SetAccessor {
        node: u16,
        change: super::super::participant_sets::Change,
    },
    Set(super::super::participant_sets::Change),
    RankTable {
        sorter: super::super::participant_properties::Property,
        rows: Vec<super::super::rank_query::Row>,
    },
    Context {
        source: super::super::participant_properties::Property,
        participant: u64,
        categories: Vec<u32>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub instance: u64,
    pub operation: Operation,
}

/// A transaction-local budget shared across graph instances and native calls.
/// This is never shared across independent sessions or committed as game data.
#[derive(Clone, Debug, Default)]
pub struct Budget(Rc<RefCell<(usize, usize)>>);
impl Budget {
    fn enter(&self) -> Result<(), Error> {
        let mut counts = self.0.borrow_mut();
        if counts.0 >= MAX_STEPS || counts.1 >= MAX_NATIVE_DEPTH {
            return Err(Error::Bound);
        }
        counts.0 += 1;
        counts.1 += 1;
        Ok(())
    }
    fn leave(&self) {
        self.0.borrow_mut().1 -= 1;
    }
}

/// The active graph stack. `false` / `None` means the instance is not on this
/// stack; the host may route it to another owned instance. No default values
/// or events are invented for a missing instance.
pub trait Callbacks {
    fn apply(&mut self, host: &mut dyn Host, invocation: Invocation) -> Result<bool, Error> {
        self.route(host, invocation, None)
    }
    /// Route with the other suspended graphs retained. Moving the callback's
    /// root must not hide descendants from a reentered ancestor.
    fn route(
        &mut self,
        host: &mut dyn Host,
        invocation: Invocation,
        suspended: Option<&mut dyn Callbacks>,
    ) -> Result<bool, Error>;
    fn contains(&self, instance: u64) -> bool;
    fn read(&mut self, instance: u64, port: Port) -> Result<Option<Option<Value>>, Error>;
    fn budget(&self) -> Budget;
}

pub trait Host {
    /// Complete the call, including all synchronous graph callbacks, before
    /// returning. Changes belong to the caller's surrounding transaction.
    fn invoke(&mut self, request: Request, graphs: &mut dyn Callbacks) -> Result<(), Error>;
}

pub struct Scope<'a> {
    pub instance: u64,
    pub host: &'a mut dyn Host,
    pub upstream: Option<&'a mut dyn Callbacks>,
}

impl Graph {
    /// Run against an already isolated candidate. The enclosing transaction
    /// must discard both candidate and native-host changes on any error.
    pub fn execute_hosted<'a, H>(
        &'a self,
        state: &mut H,
        scope: Scope<'a>,
        action: impl FnOnce(&mut H, &mut Dispatch<'_>) -> Result<(), Error>,
    ) -> Result<Vec<RemoteEmission>, Error> {
        let mut dispatch = Dispatch::new(self);
        dispatch.native = Some(scope);
        action(state, &mut dispatch)?;
        if let Some(error) = dispatch.fault {
            return Err(error);
        }
        Ok(dispatch.output)
    }
}

struct Receiver<'s, 'g, 'u, H> {
    dispatch: &'s mut Dispatch<'g>,
    state: &'s mut H,
    instance: u64,
    upstream: Option<&'u mut dyn Callbacks>,
    budget: Budget,
}

impl<H: PropertyHandler> Callbacks for Receiver<'_, '_, '_, H> {
    fn route(
        &mut self,
        host: &mut dyn Host,
        invocation: Invocation,
        suspended: Option<&mut dyn Callbacks>,
    ) -> Result<bool, Error> {
        if invocation.instance != self.instance {
            let Some(parent) = self.upstream.as_mut() else {
                return Ok(false);
            };
            if !parent.contains(invocation.instance) {
                return Ok(false);
            }
            let mut tail = Receiver {
                dispatch: &mut *self.dispatch,
                state: &mut *self.state,
                instance: self.instance,
                upstream: suspended,
                budget: self.budget.clone(),
            };
            return parent.route(host, invocation, Some(&mut tail));
        }
        // Reborrow the host only for this callback. The surrounding native
        // invocation remains suspended and owns no borrowed graph state.
        let mut nested = Dispatch::new(self.dispatch.graph);
        nested.steps = self.dispatch.steps;
        nested.depth = self.dispatch.depth + 1;
        nested.remote_count = self.dispatch.remote_count;
        nested.fault = self.dispatch.fault;
        let mut parent = Neighbors {
            left: self
                .upstream
                .as_mut()
                .map(|p| &mut **p as &mut dyn Callbacks),
            right: suspended,
            budget: self.budget.clone(),
        };
        nested.native = Some(Scope {
            instance: self.instance,
            host,
            upstream: Some(&mut parent),
        });
        let result = match invocation.operation {
            Operation::Retire => self.state.retire(&mut nested),
            Operation::Update { node } => self.state.native_update(node, &mut nested),
            Operation::Import(value) => self.state.import(value, &mut nested),
            Operation::Publish { port, value } => nested.publish(self.state, port, value),
            Operation::Emit { source, context } => {
                nested.emit_optional_context(self.state, source, context)
            }
            Operation::Deliver { target, context } => match context {
                Some(Context::Participant(p)) => {
                    self.state.handle_participant(target, p, &mut nested)
                }
                Some(Context::Integer(v)) => self.state.handle_integer(target, v, &mut nested),
                Some(Context::Entity(c)) => self.state.handle_context(target, c, &mut nested),
                None => self.state.handle(target, &mut nested),
            },
        };
        self.dispatch.steps = nested.steps;
        self.dispatch.remote_count = nested.remote_count;
        self.dispatch.output.extend(nested.output);
        self.dispatch.fault = nested.fault.or(result.as_ref().err().copied());
        result?;
        if let Some(error) = self.dispatch.fault {
            return Err(error);
        }
        Ok(true)
    }
    fn contains(&self, instance: u64) -> bool {
        self.instance == instance || self.upstream.as_ref().is_some_and(|p| p.contains(instance))
    }
    fn read(&mut self, instance: u64, port: Port) -> Result<Option<Option<Value>>, Error> {
        if instance == self.instance {
            return self.state.properties().read(port).map(Some);
        }
        match self.upstream.as_mut() {
            Some(parent) => parent.read(instance, port),
            None => Ok(None),
        }
    }
    fn budget(&self) -> Budget {
        self.budget.clone()
    }
}

struct Neighbors<'a, 'b> {
    left: Option<&'a mut dyn Callbacks>,
    right: Option<&'b mut dyn Callbacks>,
    budget: Budget,
}
impl Callbacks for Neighbors<'_, '_> {
    fn route(
        &mut self,
        host: &mut dyn Host,
        invocation: Invocation,
        suspended: Option<&mut dyn Callbacks>,
    ) -> Result<bool, Error> {
        if let Some(left) = self.left.as_mut()
            && left.contains(invocation.instance)
        {
            let mut others = Neighbors {
                left: self.right.as_mut().map(|r| &mut **r as &mut dyn Callbacks),
                right: suspended,
                budget: self.budget.clone(),
            };
            return left.route(host, invocation, Some(&mut others));
        }
        if let Some(right) = self.right.as_mut()
            && right.contains(invocation.instance)
        {
            let mut others = Neighbors {
                left: self.left.as_mut().map(|r| &mut **r as &mut dyn Callbacks),
                right: suspended,
                budget: self.budget.clone(),
            };
            return right.route(host, invocation, Some(&mut others));
        }
        Ok(false)
    }
    fn contains(&self, instance: u64) -> bool {
        self.left.as_ref().is_some_and(|p| p.contains(instance))
            || self.right.as_ref().is_some_and(|p| p.contains(instance))
    }
    fn read(&mut self, instance: u64, port: Port) -> Result<Option<Option<Value>>, Error> {
        if let Some(left) = self.left.as_mut()
            && left.contains(instance)
        {
            return left.read(instance, port);
        }
        if let Some(right) = self.right.as_mut()
            && right.contains(instance)
        {
            return right.read(instance, port);
        }
        Ok(None)
    }
    fn budget(&self) -> Budget {
        self.budget.clone()
    }
}

impl Dispatch<'_> {
    pub fn is_hosted(&self) -> bool {
        self.native.is_some()
    }

    pub fn native<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        call: Call,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            self.step()?;
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            let Some(Scope {
                instance,
                host,
                upstream,
            }) = self.native.take()
            else {
                return Err(Error::MissingInput);
            };
            let budget = upstream.as_ref().map(|p| p.budget()).unwrap_or_default();
            if let Err(error) = budget.enter() {
                self.native = Some(Scope {
                    instance,
                    host,
                    upstream,
                });
                return Err(error);
            }
            let mut receiver = Receiver {
                dispatch: self,
                state,
                instance,
                upstream,
                budget: budget.clone(),
            };
            let result = host.invoke(
                Request {
                    instance,
                    node,
                    call,
                },
                &mut receiver,
            );
            let upstream = receiver.upstream.take();
            budget.leave();
            self.native = Some(Scope {
                instance,
                host,
                upstream,
            });
            result
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }
}

#[cfg(test)]
mod tests;
