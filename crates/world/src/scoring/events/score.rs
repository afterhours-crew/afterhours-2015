// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::score::{self, Handler as ScoreHandler, Request};

impl Dispatch<'_> {
    /// Award reads Score once, then the context's current participant once, and
    /// visits its activated keepers in order. Repeat events are significant.
    /// No output property/event or implicit total/category sum is fabricated.
    pub fn score<H: ScoreHandler>(
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
            if node as usize >= self.graph.nodes {
                return Err(Error::UnknownNode);
            }
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            if state.properties().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            if event != score::AWARD {
                return Ok(());
            }
            let binding = state.score_binding(node)?;
            if binding.context as usize >= self.graph.nodes {
                return Err(Error::UnknownNode);
            }
            let amount = match state.score_input(node)? {
                Some(Value::Integer(value)) => value,
                None => binding.default,
                _ => return Err(Error::InvalidGraph),
            };
            let context = state.score_context(binding.context)?;
            if context.keepers.len() > score::MAX_KEEPERS {
                return Err(Error::Bound);
            }
            self.depth += 1;
            let result = (|| {
                for keeper in context.keepers {
                    self.step()?;
                    state.score_request(
                        Request {
                            keeper,
                            participant: context.participant,
                            amount,
                        },
                        self,
                    )?;
                    if let Some(error) = self.fault {
                        return Err(error);
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

#[cfg(test)]
mod tests;
