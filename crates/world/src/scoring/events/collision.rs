// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::collision::{self, Action, Command, Handler as CollisionHandler};

impl Dispatch<'_> {
    /// Change query state, enqueue a control command when needed, then publish
    /// the current querying value. Reports clear state, publish querying and the
    /// integer result in order, then emit a contextual event. Repeated controls
    /// do not enqueue duplicates; repeated reports remain distinct measurements.
    pub fn collision<H: CollisionHandler>(
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
            if state.properties().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            let query = state.collision_query(node)?;
            let outputs = query.outputs;
            let command = match action {
                Action::Event(collision::BEGIN) if !query.querying => {
                    query.querying = true;
                    Some(Command::Begin)
                }
                Action::Event(collision::TERMINATE) if query.querying => {
                    query.querying = false;
                    Some(Command::Terminate)
                }
                Action::Report { .. } => {
                    query.querying = false;
                    None
                }
                Action::Event(_) => None,
            };
            self.depth += 1;
            let result = (|| {
                if let Some(command) = command {
                    state.collision_control(node, command, self)?;
                    if let Some(error) = self.fault {
                        return Err(error);
                    }
                }
                if outputs.querying {
                    let port = Port {
                        node,
                        field: collision::QUERYING,
                    };
                    let value = Value::Boolean(state.collision_query(node)?.querying);
                    let previous = state.properties().read(port)?;
                    if !matches!(previous, None | Some(Value::Boolean(_))) {
                        return Err(Error::InvalidGraph);
                    }
                    if !outputs.suppress_equal_querying || previous.as_ref() != Some(&value) {
                        self.publish(state, port, value)?;
                    }
                }
                if let Action::Report { value, context } = action {
                    if outputs.value {
                        self.publish(
                            state,
                            Port {
                                node,
                                field: collision::VALUE,
                            },
                            Value::Integer(value),
                        )?;
                    }
                    self.emit_context(
                        state,
                        Endpoint {
                            node,
                            event: collision::COLLISION,
                        },
                        context,
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

#[cfg(test)]
mod tests;
