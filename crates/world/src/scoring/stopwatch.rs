// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Fresh-session stopwatch group for one realm/update pass. Start/Stop change
//! next-update membership; Reset publishes before clearing the trigger latch.
use super::{
    Error,
    events::{MAX_NODES, MAX_STEPS, PropertyHandler},
    finite,
};
use std::collections::BTreeMap;

pub const START: u32 = 0x0dc0_efa5;
pub const STOP: u32 = 0x7c89_bb7d;
pub const RESET: u32 = super::nodes::RESET;
pub const TIME: u32 = 2_089_313_744;
pub const TRIGGER: u32 = 1_975_570_652;
pub const MULTIPLIER: u32 = 2_037_512_782;
pub const TRIGGER_TIME: u32 = 2_068_878_889;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    pub multiplier: f32,
    pub trigger_time: f32,
    /// Injected engine-context scale; never substitute an assumed constant.
    pub time_scale: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Watch {
    elapsed: f32,
    triggered: bool,
    output_bound: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Manager {
    nodes: usize,
    watches: BTreeMap<u16, Watch>,
    active: Vec<u16>,
    pending: Vec<(u16, bool)>,
}
impl Manager {
    /// Supply the stopwatch node IDs and whether their Time output is bound.
    /// Seed each bound Time property to Float(0) during graph construction.
    /// Fresh watches are stopped. Engine save-state restoration is not modeled.
    pub fn new(nodes: usize, watches: &[(u16, bool)]) -> Result<Self, Error> {
        if nodes == 0 || nodes > MAX_NODES || watches.len() > MAX_NODES {
            return Err(Error::Bound);
        }
        let mut result = Self {
            nodes,
            watches: BTreeMap::new(),
            active: vec![],
            pending: vec![],
        };
        for &(node, output_bound) in watches {
            if node as usize >= nodes {
                return Err(Error::UnknownNode);
            }
            if result
                .watches
                .insert(
                    node,
                    Watch {
                        elapsed: 0.0,
                        triggered: false,
                        output_bound,
                    },
                )
                .is_some()
            {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(result)
    }
    pub fn elapsed(&self, node: u16) -> Result<f32, Error> {
        Ok(self.watches.get(&node).ok_or(Error::UnknownNode)?.elapsed)
    }
    /// Effective membership includes pending changes, so repeated Start/Stop is
    /// idempotent even before the next update applies those changes.
    pub fn running(&self, node: u16) -> Result<bool, Error> {
        if !self.watches.contains_key(&node) {
            return Err(Error::UnknownNode);
        }
        Ok(self
            .pending
            .iter()
            .rev()
            .find(|(id, _)| *id == node)
            .map_or_else(|| self.active.contains(&node), |(_, active)| *active))
    }
    pub(crate) fn nodes(&self) -> usize {
        self.nodes
    }
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub(crate) fn membership(&mut self, node: u16, running: bool) -> Result<(), Error> {
        if self.running(node)? == running {
            return Ok(());
        }
        if self.pending.len() >= MAX_STEPS {
            return Err(Error::Bound);
        }
        self.pending.push((node, running));
        Ok(())
    }
    pub(crate) fn begin_update(&mut self) -> Vec<u16> {
        for (node, running) in self.pending.drain(..) {
            if running {
                self.active.push(node);
            } else if let Some(index) = self.active.iter().position(|id| *id == node) {
                self.active.swap_remove(index);
            }
        }
        self.active.clone()
    }
    pub(crate) fn advance(&mut self, node: u16, delta: f32, inputs: Inputs) -> Result<bool, Error> {
        finite(delta)?;
        finite(inputs.multiplier)?;
        finite(inputs.trigger_time)?;
        finite(inputs.time_scale)?;
        if delta < 0.0 {
            return Err(Error::InvalidGraph);
        }
        let watch = self.watches.get_mut(&node).ok_or(Error::UnknownNode)?;
        // Keep the native Float32 operation order. Do not reassociate or use FMA.
        let elapsed = finite(inputs.time_scale * (inputs.multiplier * delta) + watch.elapsed)?;
        let trigger =
            inputs.trigger_time > 0.0 && !watch.triggered && elapsed > inputs.trigger_time;
        watch.elapsed = elapsed;
        if trigger {
            watch.triggered = true;
        }
        Ok(trigger)
    }
    pub(crate) fn reset_begin(&mut self, node: u16) -> Result<(), Error> {
        self.watches
            .get_mut(&node)
            .ok_or(Error::UnknownNode)?
            .elapsed = 0.0;
        Ok(())
    }
    pub(crate) fn reset_end(&mut self, node: u16) -> Result<(), Error> {
        self.watches
            .get_mut(&node)
            .ok_or(Error::UnknownNode)?
            .triggered = false;
        Ok(())
    }
    pub(crate) fn output(&self, node: u16) -> Result<Option<f32>, Error> {
        let watch = self.watches.get(&node).ok_or(Error::UnknownNode)?;
        Ok(watch.output_bound.then_some(watch.elapsed))
    }
}

/// Keep the manager inside the same private graph candidate as properties and
/// node state. Resolve inputs at each node's turn; earlier callbacks may change
/// them. One manager covers one update pass; cross-pass scheduling is external.
pub trait Handler: PropertyHandler {
    fn stopwatches(&mut self) -> &mut Manager;
    fn stopwatch_inputs(&self, node: u16) -> Result<Inputs, Error>;
}

#[cfg(test)]
mod tests;
