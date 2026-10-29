// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Server-side synchronized scalar state. Values live in the shared Out cell;
//! replication dirtiness is retained separately, after publication callbacks.
use super::{Error, events::PropertyHandler, properties, value::Value};

pub use super::nodes::{IN, ON_FALSE, ON_TRUE, OUT, SET_FALSE, SET_TRUE, TOGGLE};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    Text,
    Boolean,
    Float,
    Integer,
}
impl Kind {
    pub(crate) fn validate(self, value: &Value) -> Result<(), Error> {
        self.property_kind().validate(value)
    }
    pub fn property_kind(self) -> properties::Kind {
        match self {
            Self::Text => properties::Kind::Text,
            Self::Boolean => properties::Kind::Boolean,
            Self::Float => properties::Kind::Float,
            Self::Integer => properties::Kind::Integer,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Fresh binding reads a ready input if present; no default is invented.
    Bind,
    /// The input's current value must be ready when its callback runs.
    InputChanged,
    Event(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    kind: Kind,
    dirty: bool,
}
impl Node {
    /// Allocate a separate typed Out cell, initially unready, even if no other
    /// node consumes it. Activation itself does not publish or emit anything.
    pub fn new(kind: Kind) -> Self {
        Self { kind, dirty: false }
    }
    pub fn kind(&self) -> Kind {
        self.kind
    }
    pub fn dirty(&self) -> bool {
        self.dirty
    }
    /// Consume only in a candidate whose state and replication queue commit
    /// together. This does not encode a packet or acknowledge transport delivery.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

pub trait Handler: PropertyHandler {
    fn synchronized_node(&mut self, node: u16) -> Result<&mut Node, Error>;
    /// Read the same bound input cell on each call. A nested output callback can
    /// change this cell before the Boolean OnTrue/OnFalse event is selected.
    fn synchronized_input(&self, node: u16) -> Result<Option<Value>, Error>;
}
