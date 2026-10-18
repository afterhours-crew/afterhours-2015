// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    nodes::Action,
    vector_length::{self as vector, Handler as VectorHandler},
};

impl Dispatch<'_> {
    /// Publish Length and finish its callbacks before rereading A for Normal.
    /// All nested publications share this transaction's fault latch and bounds.
    pub fn vector_length<H: VectorHandler>(
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
            let config = state.vector_length(node)?;
            if !config.prepare(action) {
                return Ok(());
            }
            let config = *config;
            let Some(input) = state.vector_length_input(node)? else {
                return Ok(());
            };
            let length = vector::length(vector::vector(input)?)?;
            self.depth += 1;
            let result = (|| {
                if config.length_bound {
                    self.publish(
                        state,
                        Port {
                            node,
                            field: vector::LENGTH,
                        },
                        Value::Float(length),
                    )?;
                }
                if config.normal_bound {
                    let current = state
                        .vector_length_input(node)?
                        .ok_or(Error::MissingInput)?;
                    let normal = vector::normal(vector::vector(current)?, length)?;
                    self.publish(
                        state,
                        Port {
                            node,
                            field: vector::NORMAL,
                        },
                        Value::Vec3(normal),
                    )?;
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
