// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    nodes,
    property_cast::{self, Action, Channel, Handler as CastHandler},
};

impl Dispatch<'_> {
    /// Activation selects the first ready bound Boolean/integer/float input.
    /// Input notifications snapshot that input once, even before activation.
    /// Publish each bound float/integer/Boolean output in order, retaining the
    /// original input across nested callbacks. All effects share rollback/bounds.
    pub fn property_cast<H: CastHandler>(
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
            if node as usize >= self.graph.nodes {
                return Err(Error::UnknownNode);
            }
            if self.depth >= MAX_DEPTH {
                return Err(Error::Bound);
            }
            if state.properties().nodes() != self.graph.nodes {
                return Err(Error::InvalidGraph);
            }
            let bindings = state.property_cast(node)?;
            let mut selected = None;
            for channel in property_cast::CHANNELS {
                if let Action::PropertyChanged(field) = action
                    && field != channel.input()
                {
                    continue;
                }
                if !bindings.input(channel) {
                    if matches!(action, Action::PropertyChanged(_)) {
                        return Err(Error::MissingInput);
                    }
                    continue;
                }
                match state.properties().read(Port {
                    node,
                    field: channel.input(),
                })? {
                    Some(value) => {
                        channel.kind().validate(&value)?;
                        selected = Some(value);
                        break;
                    }
                    None if matches!(action, Action::PropertyChanged(_)) => {
                        return Err(Error::MissingInput);
                    }
                    None => {}
                }
            }
            let Some(value) = selected else {
                return Ok(false);
            };
            let effects = nodes::cast_numeric(value)?;
            self.depth += 1;
            let result = (|| {
                for (channel, effect) in [Channel::Float, Channel::Integer, Channel::Boolean]
                    .into_iter()
                    .zip(effects)
                {
                    if bindings.output(channel) {
                        let nodes::Effect::Publish(field, value) = effect else {
                            return Err(Error::InvalidGraph);
                        };
                        self.publish(state, Port { node, field }, value)?;
                    }
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
