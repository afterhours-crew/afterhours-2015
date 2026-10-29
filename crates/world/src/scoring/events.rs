// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synchronous, bounded scoring event dispatch. Callers compile installed
//! connections and own node behavior; this module does not interpret RPCs.
use super::{
    Error,
    math::Value,
    properties::{DefaultMode, Port, Registration, Store},
};
use std::collections::{BTreeMap, BTreeSet};

pub mod native;

pub const MAX_NODES: usize = 4096;
pub const MAX_CONNECTIONS: usize = 16384;
/// Non-executable scene inventories include several independently owned graphs.
pub const MAX_TOPOLOGY_CONNECTIONS: usize = 262_144;
pub const MAX_STEPS: usize = 262_144;
pub const MAX_DEPTH: usize = 256;
pub const MAX_REMOTE: usize = 4096;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct Endpoint {
    pub node: u16,
    pub event: u32,
}
/// Nullable replicated-object context, distinct from an ordinary scalar event.
/// The session interpreter resolves the object's current class and lifetime;
/// carrying this ID never grants authority over another player's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntityContext {
    pub object: Option<u16>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Context {
    Entity(EntityContext),
    Integer(i32),
    /// Native ParticipantEvent, distinct from PlayerEvent and entity collision
    /// contexts. The ID belongs to the host's owned participant model.
    Participant(u64),
}
impl From<EntityContext> for Context {
    fn from(value: EntityContext) -> Self {
        Self::Entity(value)
    }
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct Remote {
    pub bus: u16,
    /// Original connection index in this bus, before realm filtering.
    pub index: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Delivery {
    Local,
    NetworkedClient(Remote),
    NetworkedClientAndServer(Remote),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub source: Endpoint,
    pub target: Endpoint,
    pub delivery: Delivery,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEmission {
    pub source: Endpoint,
    pub connections: Vec<Remote>,
    /// Retained for a future verified encoder; never silently discard it when
    /// encoding an ordinary event message.
    pub context: Option<Context>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Graph {
    nodes: usize,
    edges: BTreeMap<Endpoint, Vec<Connection>>,
}

/// Check topology without constructing an executable graph. Scene inventories
/// can be larger than a single execution context; callers enforce their own
/// inventory limit, while endpoint width and connection bounds apply here.
pub fn validate_connections(nodes: usize, connections: &[Connection]) -> Result<(), Error> {
    if nodes == 0
        || nodes > usize::from(u16::MAX) + 1
        || connections.len() > MAX_TOPOLOGY_CONNECTIONS
    {
        return Err(Error::Bound);
    }
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
    }
    Ok(())
}
impl Graph {
    /// Connections must be in authored order, with client-only edges excluded.
    /// Duplicate local edges are significant; remote row identities are unique.
    pub fn new(nodes: usize, connections: Vec<Connection>) -> Result<Self, Error> {
        if nodes == 0 || nodes > MAX_NODES || connections.len() > MAX_CONNECTIONS {
            return Err(Error::Bound);
        }
        validate_connections(nodes, &connections)?;
        let mut edges: BTreeMap<Endpoint, Vec<Connection>> = BTreeMap::new();
        for connection in connections {
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
    /// A deactivated entity has no live event/property binding. Already-entered
    /// handlers may still unwind while its storage remains allocated.
    fn enabled(&self, _node: u16) -> bool {
        true
    }
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error>;

    /// Explicit opt-in for contextual events. Existing interpreters reject
    /// them rather than silently treating them as ordinary events. Forwarding
    /// nodes must pass this context to emit_context; newly constructed scalar
    /// events use emit and do not inherit a surrounding collision's context.
    fn handle_context(
        &mut self,
        _target: Endpoint,
        _context: EntityContext,
        _dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }

    fn handle_integer(
        &mut self,
        _target: Endpoint,
        _value: i32,
        _dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
    fn handle_participant(
        &mut self,
        _target: Endpoint,
        _participant: u64,
        _dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
}

/// Typed property interpreter for the ordinary synchronous publication path.
/// Read values through the shared Store during the callback; an earlier nested
/// callback may have changed them. `first` belongs to the outer publication,
/// not the value's current readiness. Interface defaults report `first = true`
/// even when explicitly replacing a ready value. Never commit external effects.
pub trait PropertyHandler: Handler {
    fn properties(&mut self) -> &mut Store;
    fn native_update(&mut self, _node: u16, _dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
    fn retire(&mut self, _dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
    /// A synchronous notification from an enclosing owner. Graphs that do not
    /// model these stores reject imports instead of inventing local values.
    fn import(
        &mut self,
        _value: native::Import,
        _dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
    fn property_changed(
        &mut self,
        target: Port,
        first: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error>;
}

pub struct Dispatch<'a> {
    native: Option<native::Scope<'a>>,
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
            native: None,
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
        self.emit_optional_context(state, source, None)
    }

    pub fn emit_context<H: Handler>(
        &mut self,
        state: &mut H,
        source: Endpoint,
        context: EntityContext,
    ) -> Result<(), Error> {
        self.emit_optional_context(state, source, Some(context.into()))
    }

    pub fn emit_integer<H: Handler>(
        &mut self,
        state: &mut H,
        source: Endpoint,
        value: i32,
    ) -> Result<(), Error> {
        self.emit_optional_context(state, source, Some(Context::Integer(value)))
    }
    pub fn emit_participant<H: Handler>(
        &mut self,
        state: &mut H,
        source: Endpoint,
        participant: u64,
    ) -> Result<(), Error> {
        self.emit_optional_context(state, source, Some(Context::Participant(participant)))
    }

    fn emit_optional_context<H: Handler>(
        &mut self,
        state: &mut H,
        source: Endpoint,
        context: Option<Context>,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = if matches!(context, Some(Context::Entity(c)) if c.object.is_some_and(|id| id == 0 || id > 8191))
        {
            Err(Error::InvalidGraph)
        } else {
            self.emit_inner(state, source, context)
        };
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
        let result = self.publish_inner(state, source, value, false);
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    /// Retain all outputs before notifying any listener, in the supplied order.
    /// TransformSplitter uses this contract: a callback observing translation
    /// must already see the new basis. This shares normal transaction budgets.
    pub fn publish_together<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        values: &[(Port, Value)],
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if values.len() > super::properties::MAX_PROPERTIES {
                return Err(Error::Bound);
            }
            if state.properties().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            let mut ports = BTreeSet::new();
            let mut changes = Vec::with_capacity(values.len());
            for (source, value) in values {
                self.step()?;
                if !ports.insert(*source) {
                    return Err(Error::InvalidGraph);
                }
                if !state.enabled(source.node) {
                    continue;
                }
                changes.push((*source, state.properties().write(*source, value.clone())?));
            }
            for (source, mut change) in changes {
                if !state.enabled(source.node) {
                    continue;
                }
                // Each native notify takes its own listener snapshot. A prior
                // output's callback may have registered a later output's listener.
                change.listeners = state.properties().listeners(source)?;
                self.notify_change(state, change, false)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    /// Register in the caller's verified construction order. Registration never
    /// invokes a callback or forwards an existing value. During a publication,
    /// new registrations take effect only on subsequent (including nested)
    /// publications; the current traversal retains its snapshot.
    pub fn register<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        registration: Registration,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if state.properties().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            self.step()?;
            state.properties().register(registration)
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    /// Apply a typed interface default, reporting whether it was published.
    /// An absent authored default must be skipped by the caller, never replaced
    /// with zero. The caller selects the verified binding mode: ordinary defaults
    /// fill unready cells, while explicit overrides also replace ready cells.
    /// Applied defaults always notify with `first = true`, retain before nested
    /// callbacks, and share the complete transaction's budgets and rollback.
    pub fn initialize<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        source: Port,
        value: Value,
        mode: DefaultMode,
    ) -> Result<bool, Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if state.properties().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            self.step()?;
            let ready = state.properties().read(source)?.is_some();
            if ready && mode == DefaultMode::IfUnready {
                return Ok(false);
            }
            self.publish_inner(state, source, value, true)?;
            Ok(true)
        })();
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
        initialization: bool,
    ) -> Result<(), Error> {
        if state.properties().nodes() != self.graph.nodes {
            return Err(Error::InvalidGraph);
        }
        if !state.enabled(source.node) {
            return Ok(());
        }
        self.step()?;
        if self.depth >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        let change = state.properties().write(source, value)?;
        self.notify_change(state, change, initialization)
    }

    fn notify_change<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        change: super::properties::Change,
        initialization: bool,
    ) -> Result<(), Error> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        self.depth += 1;
        let result = (|| {
            for listener in change.listeners {
                self.step()?;
                match listener {
                    super::properties::Registration::Notify(target) => {
                        if state.enabled(target.node) {
                            state.property_changed(target, initialization || change.first, self)?;
                        }
                    }
                    super::properties::Registration::Forward { source, target } => {
                        // Native callbacks receive a pointer into the current
                        // source cell. Earlier nested writes can change it.
                        let value = state
                            .properties()
                            .read(source)?
                            .ok_or(Error::MissingInput)?;
                        self.publish(state, target, value)?;
                    }
                }
                if let Some(error) = self.fault {
                    return Err(error);
                }
            }
            Ok(())
        })();
        self.depth -= 1;
        result
    }

    fn emit_inner<H: Handler>(
        &mut self,
        state: &mut H,
        source: Endpoint,
        context: Option<Context>,
    ) -> Result<(), Error> {
        if usize::from(source.node) >= self.graph.nodes {
            return Err(Error::UnknownNode);
        }
        if !state.enabled(source.node) {
            return Ok(());
        }
        self.step()?;
        if self.depth >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        self.depth += 1;
        let result = self.traverse(state, source, context);
        self.depth -= 1;
        result
    }

    fn traverse<H: Handler>(
        &mut self,
        state: &mut H,
        source: Endpoint,
        context: Option<Context>,
    ) -> Result<(), Error> {
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
            if !matches!(connection.delivery, Delivery::NetworkedClient(_))
                && state.enabled(connection.target.node)
            {
                match context {
                    Some(Context::Entity(context)) => {
                        state.handle_context(connection.target, context, self)?
                    }
                    Some(Context::Integer(value)) => {
                        state.handle_integer(connection.target, value, self)?
                    }
                    Some(Context::Participant(value)) => {
                        state.handle_participant(connection.target, value, self)?
                    }
                    None => state.handle(connection.target, self)?,
                }
                if let Some(error) = self.fault {
                    return Err(error);
                }
            }
        }
        if !remote.is_empty() {
            self.output.push(RemoteEmission {
                source,
                connections: remote,
                context,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod tests;

mod collision;
mod delays;
mod property_casts;
mod property_gates;
mod road;
mod score;
mod stopwatches;
mod synced;
mod vector_lengths;
mod vehicle_queries;
