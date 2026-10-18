// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    finite,
    stopwatch::{self, Handler as StopwatchHandler},
};

impl Dispatch<'_> {
    /// Start/Stop affect next-update membership. Reset publishes zero before
    /// clearing the trigger latch, so nested callbacks see the native ordering.
    pub fn stopwatch_event<H: StopwatchHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        event: u32,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            self.step()?;
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            if state.stopwatches().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            self.depth += 1;
            let result = (|| match event {
                stopwatch::START => state.stopwatches().membership(node, true),
                stopwatch::STOP => state.stopwatches().membership(node, false),
                stopwatch::RESET => {
                    state.stopwatches().reset_begin(node)?;
                    self.stopwatch_output(state, node)?;
                    state.stopwatches().reset_end(node)
                }
                _ => Err(Error::UnsupportedEvent),
            })();
            self.depth -= 1;
            result
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    /// Apply pending memberships, then visit the fixed current-pass snapshot.
    /// Nested Start/Stop changes are deferred to the next call. Delta and the
    /// per-context scale are supplied by the caller, not a wall-clock read.
    pub fn update_stopwatches<H: StopwatchHandler>(
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
            if state.stopwatches().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            self.depth += 1;
            let result = (|| {
                for _ in 0..state.stopwatches().pending_count() {
                    self.step()?;
                }
                let snapshot = state.stopwatches().begin_update();
                for node in snapshot {
                    self.step()?;
                    let inputs = state.stopwatch_inputs(node)?;
                    if state.stopwatches().advance(node, delta, inputs)? {
                        self.emit(
                            state,
                            Endpoint {
                                node,
                                event: stopwatch::TRIGGER,
                            },
                        )?;
                    }
                    // A trigger callback can reset elapsed. Read it only now.
                    self.stopwatch_output(state, node)?;
                }
                Ok(())
            })();
            self.depth -= 1;
            result
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    fn stopwatch_output<H: StopwatchHandler>(
        &mut self,
        state: &mut H,
        node: u16,
    ) -> Result<(), Error> {
        if let Some(elapsed) = state.stopwatches().output(node)? {
            self.publish(
                state,
                Port {
                    node,
                    field: stopwatch::TIME,
                },
                Value::Float(elapsed),
            )?;
        }
        Ok(())
    }
}
