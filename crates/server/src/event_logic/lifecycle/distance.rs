// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world::race_pacing::Route;

pub const TOTAL: u32 = 0xa52c_76ce;

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub identity: Identity,
    pub total_metres: f32,
}

/// Ordered waypoint records from the owning state. None means that participant
/// currently has no controllable; its old property is retained, not zeroed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub participant: Participant,
    pub next: usize,
    pub lap: u8,
    pub distance_to_next: Option<f32>,
}

impl Host {
    /// Interval-one update. The first callback caches route measurements and
    /// publishes total; participant distances start on the following callback.
    /// The caller measures navigation distance, using Euclidean fallback only
    /// when the native navigation service has no path.
    pub fn update_distance(
        &mut self,
        index: usize,
        route: Option<&Route>,
        measurements: &[Measurement],
        callback: &mut Callback<'_>,
    ) -> Result<Vec<super::Effect>, Error> {
        if !self.observers.enabled.contains(&Observer::Distance(index)) {
            return Err(Error::Invalid);
        }
        let definition = self
            .plan
            .data
            .bindings
            .distances
            .get(index)
            .ok_or(Error::Invalid)?
            .clone();
        if measurements.len() > states::MAX_PARTICIPANTS
            || measurements
                .iter()
                .map(|m| m.participant)
                .collect::<BTreeSet<_>>()
                .len()
                != measurements.len()
        {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        let output = if let Some(route) = next.observers.distance_routes.get(&index).cloned() {
            let mut output = Vec::new();
            for measurement in measurements {
                if !next
                    .machine
                    .members(definition.state.serializer as StateId)?
                    .contains(&measurement.participant)
                {
                    continue;
                }
                let Some(distance) = measurement.distance_to_next else {
                    continue;
                };
                let value = route
                    .reporter_remaining_millimetres(measurement.next, measurement.lap, distance)
                    .map_err(|_| Error::Invalid)?;
                output.extend(next.integer(
                    definition.output.clone(),
                    measurement.participant,
                    value,
                    callback,
                )?);
                if output.len() > states::MAX_STEPS {
                    return Err(Error::Bound);
                }
            }
            output
        } else {
            let route = route.ok_or(Error::Invalid)?;
            let total_metres = route
                .reporter_total_distance()
                .map_err(|_| Error::Invalid)?;
            next.observers.distance_routes.insert(index, route.clone());
            let actions = next.observers.enqueue([super::Work::Distance(Effect {
                identity: definition.identity,
                total_metres,
            })])?;
            next.execute(&actions, callback)?
        };
        *self = next;
        Ok(output)
    }
}

#[cfg(test)]
mod tests;
