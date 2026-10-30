// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::vehicle_query::{self as query, Bindings, Handler as QueryHandler, Query};

impl Dispatch<'_> {
    /// Process one scheduled update of an active, owner-resolved query. The
    /// caller owns activation and cadence. Publications complete synchronously
    /// and share all transaction limits and rollback with the rest of the graph.
    pub fn vehicle_query<H: QueryHandler>(
        &mut self,
        state: &mut H,
        node: u16,
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
            let bindings = state.vehicle_query(node)?;
            let Some(vehicle) = state.query_vehicle(node)? else {
                return Ok(());
            };
            self.depth += 1;
            let result = (|| {
                match bindings.query {
                    Query::Physics => {
                        // The constructor binds transform first; updates publish velocity first.
                        for index in [1, 0, 2, 3, 4, 5, 6] {
                            let field = Query::Physics.outputs()[index].field;
                            if let Some(value) = state.physics_measurement(vehicle, field)? {
                                self.query_publish(state, node, bindings, field, value)?;
                            }
                        }
                    }
                    Query::Drift => {
                        let sample = state.drift_measurement(vehicle)?;
                        sample.validate()?;
                        self.query_publish(
                            state,
                            node,
                            bindings,
                            query::VELOCITY,
                            Value::Vec3(sample.velocity),
                        )?;
                        let angle = sample.angle()?;
                        self.query_publish(
                            state,
                            node,
                            bindings,
                            query::DRIFT_ANGLE,
                            Value::Float(angle),
                        )?;
                    }
                    Query::Draft => {
                        let sample = state.draft_measurement(node, vehicle)?;
                        for (output, value) in Query::Draft.outputs().iter().zip([
                            Value::Float(sample.strength()),
                            Value::Float(sample.delta_speed()),
                            Value::Boolean(sample.active()),
                        ]) {
                            self.query_publish(state, node, bindings, output.field, value)?;
                        }
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

    fn query_publish<H: PropertyHandler>(
        &mut self,
        state: &mut H,
        node: u16,
        bindings: Bindings,
        field: u32,
        value: Value,
    ) -> Result<(), Error> {
        let output = bindings
            .query
            .outputs()
            .iter()
            .find(|o| o.field == field)
            .ok_or(Error::InvalidGraph)?;
        output.kind.validate(&value)?;
        let Some(suppress_equal) = bindings.suppression(field) else {
            return Ok(());
        };
        let port = Port { node, field };
        let previous = state.properties().read(port)?;
        if let Some(previous) = &previous {
            output.kind.validate(previous)?;
        }
        if !suppress_equal || previous.as_ref() != Some(&value) {
            self.publish(state, port, value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
