// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resolved typed property cells for a private scoring graph candidate.
//! Aliases share one value; publication never copies values into input ports.
//! Installed binding and listener registration order are the caller's contract.
use super::{Error, events::MAX_NODES, finite, math::Value};
use std::collections::BTreeMap;

pub const MAX_PROPERTIES: usize = 16_384;
pub const MAX_PORTS: usize = 65_536;
pub const MAX_LISTENERS: usize = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Port {
    pub node: u16,
    pub field: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Boolean,
    Integer,
    Float,
    Vec3,
    Vec4,
    Transform,
}
impl Kind {
    pub(crate) fn validate(self, value: Value) -> Result<(), Error> {
        match (self, value) {
            (Self::Boolean, Value::Boolean(_)) | (Self::Integer, Value::Integer(_)) => Ok(()),
            (Self::Float, Value::Float(value)) => finite(value).map(|_| ()),
            (Self::Vec3, Value::Vec3(value)) => validate_components(value),
            (Self::Vec4, Value::Vec4(value)) => validate_components(value),
            (Self::Transform, Value::Transform(value)) => {
                validate_components(value.into_iter().flatten())
            }
            _ => Err(Error::InvalidGraph),
        }
    }
}

fn validate_components(values: impl IntoIterator<Item = f32>) -> Result<(), Error> {
    for value in values {
        finite(value)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub kind: Kind,
    /// None means bound but not ready. An unbound port is a separate error.
    /// Initialization does not notify; subsequent equal publications still do.
    pub initial: Option<Value>,
    /// Output and input aliases already resolved to this single property cell.
    pub ports: Vec<Port>,
    /// Registered listener ports in their registration order. Each must alias
    /// this cell. Repeated registrations are preserved, not deduplicated.
    pub listeners: Vec<Port>,
}

/// Registration order is significant. Forwarding is a publication into a
/// distinct cell, not aliasing the producers of a multiple-source input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Registration {
    Notify(Port),
    Forward { source: Port, target: Port },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    nodes: usize,
    cells: Vec<Definition>,
    ports: BTreeMap<Port, usize>,
    registrations: Vec<Vec<Registration>>,
}

pub(crate) struct Change {
    pub first: bool,
    pub listeners: Vec<Registration>,
}

impl Store {
    /// Build a fixed binding layout. This does not infer aliases, types,
    /// listeners, realms or startup ordering from installed connection rows.
    pub fn new(nodes: usize, cells: Vec<Definition>) -> Result<Self, Error> {
        if nodes == 0 || nodes > MAX_NODES || cells.len() > MAX_PROPERTIES {
            return Err(Error::Bound);
        }
        let mut ports = BTreeMap::new();
        let mut port_count = 0usize;
        let mut listener_count = 0usize;
        for (index, cell) in cells.iter().enumerate() {
            port_count = port_count
                .checked_add(cell.ports.len())
                .ok_or(Error::Bound)?;
            listener_count = listener_count
                .checked_add(cell.listeners.len())
                .ok_or(Error::Bound)?;
            if port_count > MAX_PORTS || listener_count > MAX_LISTENERS {
                return Err(Error::Bound);
            }
            if cell.ports.is_empty() {
                return Err(Error::InvalidGraph);
            }
            if let Some(value) = cell.initial {
                cell.kind.validate(value)?;
            }
            for &port in &cell.ports {
                if usize::from(port.node) >= nodes {
                    return Err(Error::UnknownNode);
                }
                if ports.insert(port, index).is_some() {
                    return Err(Error::InvalidGraph);
                }
            }
            for listener in &cell.listeners {
                if ports.get(listener) != Some(&index) {
                    return Err(Error::InvalidGraph);
                }
            }
        }
        Ok(Self {
            nodes,
            registrations: cells
                .iter()
                .map(|c| {
                    c.listeners
                        .iter()
                        .copied()
                        .map(Registration::Notify)
                        .collect()
                })
                .collect(),
            cells,
            ports,
        })
    }

    /// Build an explicit registration sequence, interleaving forwarding and
    /// node callbacks. Cell-local listener lists must be empty to avoid two
    /// competing sources of order. Initial values are not forwarded here:
    /// publish them in the caller's verified binding/initialization order.
    pub fn with_registrations(
        nodes: usize,
        cells: Vec<Definition>,
        registrations: &[Registration],
    ) -> Result<Self, Error> {
        if registrations.len() > MAX_LISTENERS {
            return Err(Error::Bound);
        }
        if cells.iter().any(|c| !c.listeners.is_empty()) {
            return Err(Error::InvalidGraph);
        }
        let mut store = Self::new(nodes, cells)?;
        for &registration in registrations {
            let source = match registration {
                Registration::Notify(port) => port,
                Registration::Forward { source, target } => {
                    let &src = store.ports.get(&source).ok_or(Error::MissingInput)?;
                    let &dst = store.ports.get(&target).ok_or(Error::MissingInput)?;
                    if store.cells[src].kind != store.cells[dst].kind {
                        return Err(Error::InvalidGraph);
                    }
                    source
                }
            };
            let &index = store.ports.get(&source).ok_or(Error::MissingInput)?;
            store.registrations[index].push(registration);
        }
        Ok(store)
    }

    /// Read the current shared cell, including changes made by nested handlers.
    pub fn read(&self, port: Port) -> Result<Option<Value>, Error> {
        let index = self.ports.get(&port).ok_or(Error::MissingInput)?;
        Ok(self.cells[*index].initial)
    }

    pub(crate) fn nodes(&self) -> usize {
        self.nodes
    }

    /// Only the bounded event/property dispatcher may publish. Retain the cell
    /// before callbacks, snapshot listeners and preserve first-ready metadata
    /// for this publication even if a nested publication happens meanwhile.
    pub(crate) fn write(&mut self, port: Port, value: Value) -> Result<Change, Error> {
        let index = self.ports.get(&port).ok_or(Error::MissingInput)?;
        let cell = &mut self.cells[*index];
        cell.kind.validate(value)?;
        let first = cell.initial.replace(value).is_none();
        Ok(Change {
            first,
            listeners: self.registrations[*index].clone(),
        })
    }
}

#[cfg(test)]
mod tests;
