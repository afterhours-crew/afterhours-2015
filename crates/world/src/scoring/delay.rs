// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ordered fresh-session delays with caller-owned, fixed-size event context.
//! This models the queue, not a wire context schema or engine time-scale table.
use super::{
    Error,
    events::{Handler as EventHandler, MAX_NODES},
    finite,
};
use std::collections::BTreeMap;
pub const IN: u32 = super::nodes::IN;
pub const OUT: u32 = super::nodes::OUT;
pub const RESET: u32 = super::nodes::RESET;
/// Observed alternate input that replaces pending work for positive delays.
pub const RETRIGGER: u32 = 0x4db6_54d2;
pub const DELAY: u32 = 0x0c71_8d70;
pub const MAX_PENDING: usize = 4096;
pub const MAX_PER_NODE: usize = 255;
pub const MAX_CONTEXT_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub auto_start: bool,
    pub run_once: bool,
    pub remove_duplicates: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    definition: Definition,
    initialized: bool,
    fired: bool,
}
#[derive(Clone, Debug, PartialEq)]
struct Entry<C: Copy> {
    id: u64,
    node: u16,
    remaining: f32,
    context: C,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Manager<C: Copy> {
    nodes: usize,
    definitions: BTreeMap<u16, Node>,
    queue: Vec<Entry<C>>,
    next_id: u64,
    updating: bool,
    cursor: Option<u64>,
}
pub(crate) enum Command {
    None,
    Schedule { replace: bool },
    ImmediateOrSchedule { replace: bool },
}
impl<C: Copy> Manager<C> {
    /// Context must be a plain value or owned identity, with no external mutable
    /// state. Copy and this size bound prevent retaining an arbitrary heap payload.
    pub fn new(nodes: usize, definitions: &[(u16, Definition)]) -> Result<Self, Error> {
        if nodes == 0
            || nodes > MAX_NODES
            || definitions.len() > MAX_NODES
            || std::mem::size_of::<C>() > MAX_CONTEXT_BYTES
        {
            return Err(Error::Bound);
        }
        let mut result = Self {
            nodes,
            definitions: BTreeMap::new(),
            queue: vec![],
            next_id: 0,
            updating: false,
            cursor: None,
        };
        for &(node, definition) in definitions {
            if node as usize >= nodes {
                return Err(Error::UnknownNode);
            }
            if result
                .definitions
                .insert(
                    node,
                    Node {
                        definition,
                        initialized: false,
                        fired: false,
                    },
                )
                .is_some()
            {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(result)
    }
    pub fn pending(&self, node: u16) -> Result<usize, Error> {
        if !self.definitions.contains_key(&node) {
            return Err(Error::UnknownNode);
        }
        Ok(self.queue.iter().filter(|e| e.node == node).count())
    }
    pub(crate) fn nodes(&self) -> usize {
        self.nodes
    }
    pub(crate) fn initialize(&mut self, node: u16) -> Result<Command, Error> {
        let n = self.definitions.get_mut(&node).ok_or(Error::UnknownNode)?;
        if n.initialized {
            return Err(Error::InvalidGraph);
        }
        n.initialized = true;
        if n.definition.auto_start {
            n.fired = true;
            Ok(Command::Schedule {
                replace: n.definition.remove_duplicates,
            })
        } else {
            Ok(Command::None)
        }
    }
    pub(crate) fn event(&mut self, node: u16, event: u32) -> Result<Command, Error> {
        let n = self.definitions.get_mut(&node).ok_or(Error::UnknownNode)?;
        if !n.initialized {
            return Err(Error::InvalidGraph);
        }
        match event {
            IN => {
                if n.definition.run_once && n.fired {
                    return Ok(Command::None);
                }
                n.fired = true;
                Ok(Command::ImmediateOrSchedule {
                    replace: n.definition.remove_duplicates,
                })
            }
            RETRIGGER => Ok(Command::ImmediateOrSchedule { replace: true }),
            RESET => {
                n.fired = false;
                self.cancel(node);
                Ok(Command::None)
            }
            _ => Err(Error::UnsupportedEvent),
        }
    }
    fn cancel(&mut self, node: u16) {
        let mut index = 0;
        while index < self.queue.len() {
            if self.queue[index].node == node {
                if self.cursor == Some(self.queue[index].id) {
                    self.cursor = self.queue.get(index + 1).map(|e| e.id);
                }
                self.queue.remove(index);
            } else {
                index += 1;
            }
        }
    }
    pub(crate) fn schedule(
        &mut self,
        node: u16,
        remaining: f32,
        context: C,
        replace: bool,
    ) -> Result<(), Error> {
        finite(remaining)?;
        if replace {
            self.cancel(node);
        }
        if self.queue.len() >= MAX_PENDING || self.pending(node)? >= MAX_PER_NODE {
            return Err(Error::Bound);
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(Error::Bound)?;
        self.queue.push(Entry {
            id,
            node,
            remaining,
            context,
        });
        Ok(())
    }
    pub(crate) fn begin_update(&mut self) -> Result<(), Error> {
        // Engine updates are top-level operations, not recursive graph inputs.
        if self.updating {
            return Err(Error::InvalidGraph);
        }
        self.updating = true;
        self.cursor = self.queue.first().map(|e| e.id);
        Ok(())
    }
    pub(crate) fn current_node(&self) -> Result<Option<u16>, Error> {
        self.cursor
            .map(|id| {
                self.queue
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.node)
                    .ok_or(Error::InvalidGraph)
            })
            .transpose()
    }
    pub(crate) fn advance(&mut self, delta: f32, scale: f32) -> Result<Option<(u16, C)>, Error> {
        finite(scale)?;
        let id = self.cursor.ok_or(Error::InvalidGraph)?;
        let index = self
            .queue
            .iter()
            .position(|e| e.id == id)
            .ok_or(Error::InvalidGraph)?;
        self.cursor = self.queue.get(index + 1).map(|e| e.id);
        let entry = &mut self.queue[index];
        entry.remaining = finite(entry.remaining - scale * delta)?;
        if entry.remaining <= 0.0 {
            let entry = self.queue.remove(index);
            Ok(Some((entry.node, entry.context)))
        } else {
            Ok(None)
        }
    }
    pub(crate) fn end_update(&mut self) {
        self.updating = false;
        self.cursor = None;
    }
}
/// Keep this manager in the same candidate as other scoring state. Context is a
/// bounded local identity/value, never a borrowed packet or fabricated wire data.
pub trait Handler: EventHandler {
    type Context: Copy;
    fn delays(&mut self) -> &mut Manager<Self::Context>;
    fn delay_input(&self, node: u16) -> Result<f32, Error>;
    /// Resolve the authored time domain and current engine-context scale.
    fn delay_scale(&self, node: u16) -> Result<f32, Error>;
    fn delay_output(
        &mut self,
        node: u16,
        context: Self::Context,
        dispatch: &mut super::events::Dispatch<'_>,
    ) -> Result<(), Error>;
}
#[cfg(test)]
mod tests;
