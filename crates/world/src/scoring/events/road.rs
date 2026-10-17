// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::road::{self, Frame, Handler as RoadHandler, Sample};
impl Dispatch<'_> {
    /// Process one caller-scheduled owned road measurement. Limit publication,
    /// speed event, and wrong-way event complete in that order. Missing road
    /// samples preserve the native state; an off-road sample resets durations
    /// and ends wrong-way driving without fabricating a not-speeding event.
    pub fn road<H: RoadHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        frame: Frame,
        sample: Option<Sample>,
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
            let Some(reading) = state.road_query(node)?.prepare(frame, sample)? else {
                return Ok(());
            };
            self.depth += 1;
            let result = (|| {
                if reading.limit_changed && state.road_query(node)?.limit_bound {
                    self.publish(
                        state,
                        Port {
                            node,
                            field: road::SPEED_LIMIT,
                        },
                        Value::Float(reading.limit),
                    )?;
                }
                if !reading.on_road {
                    if let Some(event) = state.road_query(node)?.leave_road() {
                        self.road_event(state, node, event)?;
                    }
                    return Ok(());
                }
                if let Some(event) = state.road_query(node)?.speeding(reading)? {
                    self.road_event(state, node, event)?;
                }
                if let Some(event) = state.road_query(node)?.wrong_way(reading) {
                    self.road_event(state, node, event)?;
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
    fn road_event<H: Handler>(
        &mut self,
        state: &mut H,
        node: u16,
        event: road::Event,
    ) -> Result<(), Error> {
        let source = Endpoint {
            node,
            event: event.id,
        };
        match event.integer {
            Some(value) => self.emit_integer(state, source, value),
            None => self.emit(state, source),
        }
    }
}

#[cfg(test)]
mod tests;
