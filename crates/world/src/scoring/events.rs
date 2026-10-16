// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synchronous, bounded scoring event dispatch. Callers compile installed
//! connections and own node behavior; this module does not interpret RPCs.
use super::{
    Error,
    math::Value,
    properties::{Port, Store},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_NODES: usize = 4096;
pub const MAX_CONNECTIONS: usize = 16384;
pub const MAX_STEPS: usize = 16384;
pub const MAX_DEPTH: usize = 32;
pub const MAX_REMOTE: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Endpoint {
    pub node: u16,
    pub event: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Remote {
    pub bus: u16,
    /// Original connection index in this bus, before realm filtering.
    pub index: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Local,
    NetworkedClient(Remote),
    NetworkedClientAndServer(Remote),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Connection {
    pub source: Endpoint,
    pub target: Endpoint,
    pub delivery: Delivery,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEmission {
    pub source: Endpoint,
    pub connections: Vec<Remote>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Graph {
    nodes: usize,
    edges: BTreeMap<Endpoint, Vec<Connection>>,
}
impl Graph {
    /// Connections must be in authored order, with client-only edges excluded.
    /// Duplicate local edges are significant; remote row identities are unique.
    pub fn new(nodes: usize, connections: Vec<Connection>) -> Result<Self, Error> {
        if nodes == 0 || nodes > MAX_NODES || connections.len() > MAX_CONNECTIONS {
            return Err(Error::Bound);
        }
        let mut edges: BTreeMap<Endpoint, Vec<Connection>> = BTreeMap::new();
        let mut remote = BTreeSet::new();
        for connection in connections {
            if [connection.source, connection.target]
                .iter()
                .any(|e| usize::from(e.node) >= nodes)
            {
                return Err(Error::UnknownNode);
            }
            if let Delivery::NetworkedClient(row) | Delivery::NetworkedClientAndServer(row) =
                connection.delivery
                && !remote.insert(row)
            {
                return Err(Error::InvalidGraph);
            }
            edges.entry(connection.source).or_default().push(connection);
        }
        Ok(Self { nodes, edges })
    }

    /// Evaluate against a private candidate. On success the caller commits both
    /// returned state and remote emissions with its bounded output queue.
    /// Failure returns neither state changes nor partial remote output.
    pub fn apply<H: Handler + Clone>(
        &self,
        state: &H,
        source: Endpoint,
    ) -> Result<(H, Vec<RemoteEmission>), Error> {
        self.transaction(state, |candidate, dispatch| {
            dispatch.emit(candidate, source)
        })
    }

    /// Publish an external measurement into a private graph candidate. Local
    /// property callbacks and nested events share one work/depth/output budget.
    /// The caller commits candidate state and remote output together.
    pub fn publish<H: PropertyHandler + Clone>(
        &self,
        state: &H,
        source: Port,
        value: Value,
    ) -> Result<(H, Vec<RemoteEmission>), Error> {
        self.transaction(state, |candidate, dispatch| {
            dispatch.publish(candidate, source, value)
        })
    }

    /// Run trusted, bounded interpreter work against one private candidate.
    /// Use Dispatch for every event/property/timer operation; it latches errors
    /// and shares limits across the complete operation. No external writes.
    pub fn transaction<H: Clone>(
        &self,
        state: &H,
        action: impl FnOnce(&mut H, &mut Dispatch<'_>) -> Result<(), Error>,
    ) -> Result<(H, Vec<RemoteEmission>), Error> {
        let mut candidate = state.clone();
        let mut dispatch = Dispatch::new(self);
        action(&mut candidate, &mut dispatch)?;
        if let Some(error) = dispatch.fault {
            return Err(error);
        }
        Ok((candidate, dispatch.output))
    }
}

/// A pure per-session node interpreter. Do not perform external writes, queue
/// network output or share mutable state through Clone. Property changes and
/// subsequent emissions must happen in their native order inside this callback.
pub trait Handler {
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error>;
}

/// Typed property interpreter for the ordinary synchronous publication path.
/// Read values through the shared Store during the callback; an earlier nested
/// callback may have changed them. `first` belongs to the outer publication,
/// not the value's current readiness. Never commit external effects here.
pub trait PropertyHandler: Handler {
    fn properties(&mut self) -> &mut Store;
    fn property_changed(
        &mut self,
        target: Port,
        first: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error>;
}

pub struct Dispatch<'a> {
    graph: &'a Graph,
    steps: usize,
    depth: usize,
    remote_count: usize,
    output: Vec<RemoteEmission>,
    fault: Option<Error>,
}
impl<'a> Dispatch<'a> {
    fn new(graph: &'a Graph) -> Self {
        Self {
            graph,
            steps: 0,
            depth: 0,
            remote_count: 0,
            output: Vec::new(),
            fault: None,
        }
    }
    fn step(&mut self) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            Err(Error::Bound)
        } else {
            Ok(())
        }
    }

    /// Finish nested emissions before the handler continues or the next sibling
    /// runs. Remote rows are emitted after this source's complete local traversal.
    pub fn emit<H: Handler>(&mut self, state: &mut H, source: Endpoint) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = self.emit_inner(state, source);
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    /// Store before notifying, including equal values. Callbacks run one at a
    /// time in supplied registration order; nested work completes before the
    /// next listener. Any error poisons this whole event/property transaction.
    pub fn publish<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        source: Port,
        value: Value,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = self.publish_inner(state, source, value);
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    fn publish_inner<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        source: Port,
        value: Value,
    ) -> Result<(), Error> {
        if state.properties().nodes() != self.graph.nodes {
            return Err(Error::InvalidGraph);
        }
        self.step()?;
        if self.depth >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        let change = state.properties().write(source, value)?;
        self.depth += 1;
        let result = (|| {
            for target in change.listeners {
                self.step()?;
                state.property_changed(target, change.first, self)?;
                if let Some(error) = self.fault {
                    return Err(error);
                }
            }
            Ok(())
        })();
        self.depth -= 1;
        result
    }

    fn emit_inner<H: Handler>(&mut self, state: &mut H, source: Endpoint) -> Result<(), Error> {
        if usize::from(source.node) >= self.graph.nodes {
            return Err(Error::UnknownNode);
        }
        self.step()?;
        if self.depth >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        self.depth += 1;
        let result = self.traverse(state, source);
        self.depth -= 1;
        result
    }

    fn traverse<H: Handler>(&mut self, state: &mut H, source: Endpoint) -> Result<(), Error> {
        let graph = self.graph;
        let mut remote = Vec::new();
        for connection in graph.edges.get(&source).into_iter().flatten() {
            self.step()?;
            if let Delivery::NetworkedClient(row) | Delivery::NetworkedClientAndServer(row) =
                connection.delivery
            {
                self.remote_count += 1;
                if self.remote_count > MAX_REMOTE {
                    return Err(Error::Bound);
                }
                remote.push(row);
            }
            if !matches!(connection.delivery, Delivery::NetworkedClient(_)) {
                state.handle(connection.target, self)?;
            }
        }
        if !remote.is_empty() {
            self.output.push(RemoteEmission {
                source,
                connections: remote,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

mod delays;
mod property_gates;
mod stopwatches;
