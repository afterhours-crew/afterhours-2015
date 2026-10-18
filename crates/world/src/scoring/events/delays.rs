// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    delay::{Command, Handler as DelayHandler},
    finite,
};
impl Dispatch<'_> {
    /// Fresh binding schedules AutoStart, including a nonpositive delay. Supply
    /// the caller's empty/base context, not a prior incoming event's context.
    pub fn initialize_delay<H: DelayHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        context: H::Context,
    ) -> Result<(), Error> {
        self.delay_control(state, node, None, context)
    }
    pub fn delay_event<H: DelayHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        event: u32,
        context: H::Context,
    ) -> Result<(), Error> {
        self.delay_control(state, node, Some(event), context)
    }
    fn delay_control<H: DelayHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        event: Option<u32>,
        context: H::Context,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            self.step()?;
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            if state.delays().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            let command = match event {
                Some(event) => state.delays().event(node, event)?,
                None => state.delays().initialize(node)?,
            };
            let (replace, immediate) = match command {
                Command::None => return Ok(()),
                Command::Schedule { replace } => (replace, false),
                Command::ImmediateOrSchedule { replace } => (replace, true),
            };
            let delay = finite(state.delay_input(node)?)?;
            if !immediate || delay > 0.0 {
                return state.delays().schedule(node, delay, context, replace);
            }
            // Immediate In/Retrigger never cancels existing queued events.
            self.depth += 1;
            let result = state.delay_output(node, context, self);
            self.depth -= 1;
            result
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }
    /// Visit the live insertion-ordered queue. Removal precedes the callback;
    /// cancellation repairs the saved next cursor. New tail entries may be
    /// visited in this update only while an existing next entry remains.
    pub fn update_delays<H: DelayHandler>(
        &mut self,
        state: &mut H,
        delta: f32,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            finite(delta)?;
            if delta < 0.0 {
                return Err(Error::InvalidGraph);
            }
            self.step()?;
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            if state.delays().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            state.delays().begin_update()?;
            self.depth += 1;
            let result = (|| {
                while let Some(node) = state.delays().current_node()? {
                    self.step()?;
                    let scale = state.delay_scale(node)?;
                    if let Some((node, context)) = state.delays().advance(delta, scale)? {
                        state.delay_output(node, context, self)?;
                    }
                }
                Ok(())
            })();
            self.depth -= 1;
            state.delays().end_update();
            result
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }
}
