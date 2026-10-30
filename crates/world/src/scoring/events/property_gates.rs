// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::property_gate::{Action, Handler as GateHandler};

impl Dispatch<'_> {
    /// Retain gate state before callbacks. A nested Close does not cancel the
    /// remainder of an already-started flush; each later input is read afresh.
    pub fn property_gate<H: GateHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        action: Action,
    ) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            self.step()?;
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            if node as usize >= self.graph.nodes {
                return Err(Error::UnknownNode);
            }
            let channels = state.property_gate(node)?.prepare(action)?;
            self.depth += 1;
            let result = (|| {
                for channel in channels {
                    self.step()?;
                    if let Some(value) = state.property_gate_input(node, channel)? {
                        channel.kind().validate(&value)?;
                        self.publish(
                            state,
                            Port {
                                node,
                                field: channel.output(),
                            },
                            value,
                        )?;
                    }
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
}
