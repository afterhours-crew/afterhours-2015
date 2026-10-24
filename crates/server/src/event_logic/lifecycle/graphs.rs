// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One atomic owner for shared-mode, participant-logic and lifecycle execution.
//! A graph may call the lifecycle host, which may call another graph, which may
//! synchronously reenter the first graph. Nothing is sent or persisted here.
use super::{Host, commands::Command};
pub mod backend;
use backend::{Backend, Running, Yield};
use nfs_world::scoring::{
    self,
    events::native::{self, Callbacks, Invocation, Request, Scope},
    interpreter::{Inputs, Output, Session, hosted::Action},
    properties::Port,
    value::Value,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_INSTANCES: usize = 256;
// A native boundary adds graph, driver and lifecycle frames. Its depth bound
// must be lower than the ordinary graph dispatch bound, including debug builds
// on Windows test threads. Repeated sequential calls do not consume depth.
const MAX_NATIVE_DEPTH: usize = 16;

#[derive(Clone, Debug)]
pub struct Instance {
    pub session: Session,
    pub inputs: Inputs,
}

#[derive(Clone, Debug)]
pub enum Step<C = Command> {
    Lifecycle(Vec<C>),
    /// Resume an owned observer after its preceding synchronous graph call.
    /// Iterating listeners uses this instead of recursively growing the stack.
    Continue {
        owner: u64,
        token: u64,
    },
    Install {
        id: u64,
        instance: Box<Instance>,
    },
    Graph {
        id: u64,
        action: Action,
    },
    /// Retire callbacks run before the graph's storage is removed.
    Remove {
        id: u64,
    },
}

/// Reads an active graph's candidate through its callback stack, or an idle
/// graph's committed candidate. A missing graph is an error, not an unready cell.
pub trait Reader {
    /// Select the lifecycle instance that owns the following native source
    /// reads. A world driver resolves this from the exact invoking graph.
    fn select(&mut self, _scope: u64) -> Result<(), scoring::Error> {
        Err(scoring::Error::MissingInput)
    }
    fn read(&mut self, instance: u64, port: Port) -> Result<Option<Value>, scoring::Error>;
    fn members(&self, _source: &super::SetSource) -> Result<BTreeSet<u64>, scoring::Error> {
        Err(scoring::Error::MissingInput)
    }
    fn float(
        &self,
        _source: &nfs_world::scoring::participant_properties::Property,
        _participant: u64,
    ) -> Result<Option<f32>, scoring::Error> {
        Err(scoring::Error::MissingInput)
    }
    fn boolean(
        &self,
        _source: &super::BooleanSource,
        _participant: u64,
    ) -> Result<Option<bool>, scoring::Error> {
        Err(scoring::Error::MissingInput)
    }
    fn integer(
        &self,
        _source: &super::IntegerSource,
        _participant: u64,
    ) -> Result<Option<i32>, scoring::Error> {
        Err(scoring::Error::MissingInput)
    }
    fn ranking(
        &self,
        _sorter: usize,
    ) -> Result<Vec<nfs_world::scoring::rank_query::Row>, scoring::Error> {
        Err(scoring::Error::MissingInput)
    }
}

/// Typed event/scene bindings decide what each native entity does. Returning
/// steps releases the driver's borrow before synchronous reentry. All driver
/// state is cloned and committed with the graphs and lifecycle machine.
pub trait Driver<C = Command, E = super::Effect>: Clone {
    fn resume(
        &mut self,
        _owner: u64,
        _token: u64,
        _values: &mut dyn Reader,
    ) -> Result<Vec<Step<C>>, scoring::Error> {
        Err(scoring::Error::InvalidGraph)
    }
    fn invoke(
        &mut self,
        request: Request,
        values: &mut dyn Reader,
    ) -> Result<Vec<Step<C>>, scoring::Error>;
    fn effect(
        &mut self,
        effect: &E,
        machine: &dyn nfs_world::participant_states::Membership,
        values: &mut dyn Reader,
    ) -> Result<Vec<Step<C>>, scoring::Error>;
}

#[derive(Clone, Debug)]
pub struct Runtime<D, H = Host> {
    lifecycle: H,
    driver: D,
    instances: BTreeMap<u64, Instance>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Publication<E = super::Effect> {
    Lifecycle(E),
    /// Output of a completed graph action. Keep graph identity for its own
    /// replication namespace; this is not an instruction to send packets yet.
    Graph {
        id: u64,
        output: Box<Output>,
    },
}

impl<H: Backend, D: Driver<H::Command, H::Effect>> Runtime<D, H> {
    pub fn new(lifecycle: H, driver: D) -> Self {
        Self {
            lifecycle,
            driver,
            instances: BTreeMap::new(),
        }
    }
    pub fn driver(&self) -> &D {
        &self.driver
    }
    pub fn lifecycle(&self) -> &H {
        &self.lifecycle
    }
    pub fn instance(&self, id: u64) -> Option<&Instance> {
        self.instances.get(&id)
    }

    pub fn execute(
        &mut self,
        steps: Vec<Step<H::Command>>,
    ) -> Result<Vec<Publication<H::Effect>>, scoring::Error> {
        let mut next = Transaction {
            runtime: self.clone(),
            execution: None,
            active: BTreeSet::new(),
            starting: BTreeSet::new(),
            removing: BTreeSet::new(),
            publications: Vec::new(),
            steps: 0,
            depth: 0,
        };
        next.run(steps, None)?;
        if next.execution.is_some() || !next.active.is_empty() {
            return Err(scoring::Error::InvalidGraph);
        }
        for id in next.removing {
            next.runtime
                .instances
                .remove(&id)
                .ok_or(scoring::Error::UnknownNode)?;
        }
        *self = next.runtime;
        Ok(next.publications)
    }
}

struct Transaction<D, H: Backend> {
    runtime: Runtime<D, H>,
    execution: Option<H::Execution>,
    active: BTreeSet<u64>,
    starting: BTreeSet<u64>,
    removing: BTreeSet<u64>,
    publications: Vec<Publication<H::Effect>>,
    steps: usize,
    depth: usize,
}

fn state_error(error: nfs_world::participant_states::Error) -> scoring::Error {
    match error {
        nfs_world::participant_states::Error::Bound => scoring::Error::Bound,
        _ => scoring::Error::InvalidGraph,
    }
}

struct Read<'a, 'b, H: Backend> {
    instances: &'a BTreeMap<u64, Instance>,
    host: &'a H,
    execution: Option<&'a H::Execution>,
    scope: u64,
    upstream: Option<&'b mut dyn Callbacks>,
}
impl<H: Backend> Read<'_, '_, H> {
    fn values(&self) -> Result<super::values::Values<'_>, scoring::Error> {
        match self.execution {
            Some(execution) => execution.values(self.scope),
            None => self.host.values(self.scope),
        }
        .map_err(state_error)
    }
}
impl<H: Backend> Reader for Read<'_, '_, H> {
    fn select(&mut self, scope: u64) -> Result<(), scoring::Error> {
        match self.execution {
            Some(execution) => execution.values(scope),
            None => self.host.values(scope),
        }
        .map_err(state_error)?;
        self.scope = scope;
        Ok(())
    }
    fn members(&self, source: &super::SetSource) -> Result<BTreeSet<u64>, scoring::Error> {
        self.values()?.members(source).map_err(state_error)
    }
    fn float(
        &self,
        source: &nfs_world::scoring::participant_properties::Property,
        participant: u64,
    ) -> Result<Option<f32>, scoring::Error> {
        self.values()?
            .float(source, participant)
            .map_err(state_error)
    }
    fn boolean(
        &self,
        source: &super::BooleanSource,
        participant: u64,
    ) -> Result<Option<bool>, scoring::Error> {
        self.values()?
            .boolean(source, participant)
            .map_err(state_error)
    }
    fn integer(
        &self,
        source: &super::IntegerSource,
        participant: u64,
    ) -> Result<Option<i32>, scoring::Error> {
        self.values()?
            .integer(source, participant)
            .map_err(state_error)
    }
    fn ranking(
        &self,
        sorter: usize,
    ) -> Result<Vec<nfs_world::scoring::rank_query::Row>, scoring::Error> {
        self.values()?
            .ranking(sorter)
            .map(<[_]>::to_vec)
            .map_err(state_error)
    }
    fn read(&mut self, instance: u64, port: Port) -> Result<Option<Value>, scoring::Error> {
        if let Some(upstream) = self.upstream.as_mut()
            && let Some(value) = upstream.read(instance, port)?
        {
            return Ok(value);
        }
        self.instances
            .get(&instance)
            .ok_or(scoring::Error::UnknownNode)?
            .session
            .read(port)
    }
}

impl<H: Backend, D: Driver<H::Command, H::Effect>> Transaction<D, H> {
    fn count(&mut self) -> Result<(), scoring::Error> {
        if self.steps >= nfs_world::participant_states::MAX_STEPS {
            return Err(scoring::Error::Bound);
        }
        self.steps += 1;
        Ok(())
    }
    fn publish(&mut self, publication: Publication<H::Effect>) -> Result<(), scoring::Error> {
        if self.publications.len() >= nfs_world::participant_states::MAX_STEPS {
            return Err(scoring::Error::Bound);
        }
        self.publications.push(publication);
        Ok(())
    }
    fn run(
        &mut self,
        steps: Vec<Step<H::Command>>,
        mut upstream: Option<&mut dyn Callbacks>,
    ) -> Result<(), scoring::Error> {
        if self.depth >= MAX_NATIVE_DEPTH
            || steps.len() > nfs_world::participant_states::MAX_PENDING
        {
            return Err(scoring::Error::Bound);
        }
        self.depth += 1;
        let result = (|| {
            let mut pending = VecDeque::from(steps);
            while let Some(step) = pending.pop_front() {
                self.count()?;
                let upstream = upstream.as_mut().map(|u| &mut **u as &mut dyn Callbacks);
                match step {
                    Step::Continue { owner, token } => {
                        let next = self.runtime.driver.resume(
                            owner,
                            token,
                            &mut Read {
                                instances: &self.runtime.instances,
                                host: &self.runtime.lifecycle,
                                execution: self.execution.as_ref(),
                                scope: 0,
                                upstream,
                            },
                        )?;
                        if pending.len().saturating_add(next.len())
                            > nfs_world::participant_states::MAX_PENDING
                        {
                            return Err(scoring::Error::Bound);
                        }
                        for step in next.into_iter().rev() {
                            pending.push_front(step);
                        }
                    }
                    Step::Lifecycle(commands) => self.commands(commands, upstream)?,
                    Step::Install { id, instance } => {
                        if id == 0
                            || self.runtime.instances.contains_key(&id)
                            || self.active.contains(&id)
                        {
                            return Err(scoring::Error::InvalidGraph);
                        }
                        if self.runtime.instances.len() + self.active.len() >= MAX_INSTANCES {
                            return Err(scoring::Error::Bound);
                        }
                        self.runtime.instances.insert(id, *instance);
                    }
                    Step::Graph { id, action } => self.graph(id, action, upstream)?,
                    Step::Remove { id } => {
                        // Deactivate synchronously, including through an active
                        // callback stack. Retain storage until this transaction
                        // unwinds, like the native deferred deletion queue.
                        if self.starting.contains(&id) {
                            return Err(scoring::Error::InvalidGraph);
                        }
                        if !self.removing.insert(id) {
                            continue;
                        }
                        self.graph(id, Action::Retire, upstream)?;
                    }
                }
            }
            Ok(())
        })();
        self.depth -= 1;
        result
    }

    fn graph(
        &mut self,
        id: u64,
        action: Action,
        mut upstream: Option<&mut dyn Callbacks>,
    ) -> Result<(), scoring::Error> {
        let operation = match &action {
            Action::Graph(operation) => Some(operation.clone()),
            Action::Retire => Some(native::Operation::Retire),
            _ => None,
        };
        if operation == Some(native::Operation::Retire) && self.starting.contains(&id) {
            return Err(scoring::Error::InvalidGraph);
        }
        if let Some(operation) = operation
            && let Some(callbacks) = upstream.as_mut()
            && callbacks.apply(
                self,
                Invocation {
                    instance: id,
                    operation,
                },
            )?
        {
            return Ok(());
        }
        // Starting or ticking an already executing session is not a
        // graph callback. Reject it instead of mutating a stale Session clone.
        if !self.active.insert(id) {
            return Err(scoring::Error::InvalidGraph);
        }
        let mut instance = self
            .runtime
            .instances
            .remove(&id)
            .ok_or(scoring::Error::UnknownNode)?;
        if action == Action::Start {
            self.starting.insert(id);
        }
        let output = instance.session.hosted(
            action,
            &instance.inputs,
            Scope {
                instance: id,
                host: &mut *self,
                upstream: upstream.as_mut().map(|u| &mut **u as &mut dyn Callbacks),
            },
        )?;
        self.active.remove(&id);
        self.starting.remove(&id);
        self.runtime.instances.insert(id, instance);
        self.publish(Publication::Graph {
            id,
            output: Box::new(output),
        })
    }

    fn commands(
        &mut self,
        commands: Vec<H::Command>,
        mut upstream: Option<&mut dyn Callbacks>,
    ) -> Result<(), scoring::Error> {
        let nested = match self.execution.as_mut() {
            Some(execution) => Some(execution.call(commands).map_err(state_error)?),
            None => {
                self.execution = Some(
                    self.runtime
                        .lifecycle
                        .begin(commands)
                        .map_err(state_error)?,
                );
                None
            }
        };
        loop {
            self.count()?;
            let execution = self
                .execution
                .as_mut()
                .ok_or(scoring::Error::InvalidGraph)?;
            match execution.advance().map_err(state_error)? {
                Some(Yield::Returned(token)) if nested == Some(token) => return Ok(()),
                Some(Yield::Returned(_)) => return Err(scoring::Error::InvalidGraph),
                Some(Yield::Effect(effect)) => {
                    let steps = self.runtime.driver.effect(
                        &effect,
                        &execution.view(&effect).map_err(state_error)?,
                        &mut Read {
                            instances: &self.runtime.instances,
                            host: &self.runtime.lifecycle,
                            execution: Some(execution),
                            scope: H::Execution::effect_scope(&effect),
                            upstream: upstream.as_mut().map(|u| &mut **u as &mut dyn Callbacks),
                        },
                    )?;
                    self.publish(Publication::Lifecycle(effect))?;
                    self.run(
                        steps,
                        upstream.as_mut().map(|u| &mut **u as &mut dyn Callbacks),
                    )?;
                    self.execution
                        .as_mut()
                        .ok_or(scoring::Error::InvalidGraph)?
                        .resume(vec![])
                        .map_err(state_error)?;
                }
                None if nested.is_none() => {
                    let execution = self.execution.take().ok_or(scoring::Error::InvalidGraph)?;
                    self.runtime.lifecycle = execution.finish().map_err(state_error)?;
                    return Ok(());
                }
                None => return Err(scoring::Error::InvalidGraph),
            }
        }
    }
}

impl<H: Backend, D: Driver<H::Command, H::Effect>> native::Host for Transaction<D, H> {
    fn invoke(
        &mut self,
        request: Request,
        graphs: &mut dyn Callbacks,
    ) -> Result<(), scoring::Error> {
        self.count()?;
        let steps = self.runtime.driver.invoke(
            request,
            &mut Read {
                instances: &self.runtime.instances,
                host: &self.runtime.lifecycle,
                execution: self.execution.as_ref(),
                scope: 0,
                upstream: Some(graphs),
            },
        )?;
        self.run(steps, Some(graphs))
    }
}

#[cfg(test)]
mod retirement_tests;
#[cfg(test)]
mod tests;
