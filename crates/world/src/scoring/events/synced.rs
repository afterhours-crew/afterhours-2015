// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::synced::{self, Action, Handler as SyncedHandler, Kind};

impl Dispatch<'_> {
    /// Bind, deliver an input callback, or handle a Boolean command. Equal ready
    /// values are suppressed, including opposite signed float zeros. First-ready
    /// false/zero still publishes. Retain Out, complete its nested callbacks,
    /// mark replication dirty, then emit a Boolean event. Input-based events
    /// reread the original input cell; explicit commands keep their local value.
    /// No saved-state restore or network serialization is performed here.
    pub fn synchronized<H: SyncedHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        action: Action,
    ) -> Result<bool, Error> {
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
            let kind = state.synchronized_node(node)?.kind();
            let output = Port {
                node,
                field: synced::OUT,
            };
            let from_input = !matches!(action, Action::Event(_));
            let value = match action {
                Action::Bind | Action::InputChanged => match state.synchronized_input(node)? {
                    Some(value) => value,
                    None if action == Action::Bind => return Ok(false),
                    None => return Err(Error::MissingInput),
                },
                Action::Event(event) if kind == Kind::Boolean => Value::Boolean(match event {
                    synced::SET_TRUE => true,
                    synced::SET_FALSE => false,
                    synced::TOGGLE => match state.properties().read(output)? {
                        Some(Value::Boolean(value)) => !value,
                        None => true,
                        _ => return Err(Error::InvalidGraph),
                    },
                    _ => return Ok(false),
                }),
                Action::Event(_) => return Ok(false),
            };
            kind.validate(value)?;
            if let Some(previous) = state.properties().read(output)? {
                kind.validate(previous)?;
                if previous == value {
                    return Ok(false);
                }
            }
            self.depth += 1;
            let result = (|| {
                self.publish(state, output, value)?;
                state.synchronized_node(node)?.mark_dirty();
                if kind == Kind::Boolean {
                    let current = if from_input {
                        state.synchronized_input(node)?.ok_or(Error::MissingInput)?
                    } else {
                        value
                    };
                    let Value::Boolean(current) = current else {
                        return Err(Error::InvalidGraph);
                    };
                    self.emit(
                        state,
                        Endpoint {
                            node,
                            event: if current {
                                synced::ON_TRUE
                            } else {
                                synced::ON_FALSE
                            },
                        },
                    )?;
                }
                Ok(true)
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
