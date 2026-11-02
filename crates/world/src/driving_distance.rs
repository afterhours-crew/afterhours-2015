// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Distance accumulator for one owned driver-details logic lifetime.
//!
//! The adapter supplies the authored sampling period, accepted chassis state,
//! and the committed starting distance. It must reset the position anchor on
//! an authoritative relocation and create a new session for a new logic
//! lifetime. This model neither accepts client distance totals nor persists or
//! publishes them. Returned totals still need an atomic storage/publication
//! boundary in the adapter.
use crate::scoring::vector_length;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Clock,
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub position: [f32; 3],
    pub airborne: bool,
    pub crashed: bool,
}

impl Sample {
    pub fn from_physics(physics: &crate::replication::vehicle::Physics) -> Result<Self, Error> {
        let motion = physics.decode([0.; 3]).map_err(|_| Error::Invalid)?;
        let result = Self {
            position: motion.position,
            airborne: physics.controls.is_airborne(),
            crashed: physics.controls.is_crashed(),
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(self) -> Result<(), Error> {
        if self.position.iter().all(|v| v.is_finite()) {
            Ok(())
        } else {
            Err(Error::Invalid)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metrics {
    pub kilometres: i32,
    pub miles: i32,
}

impl Metrics {
    /// The metric path divides integers; the imperial path casts metres to
    /// f32, divides by the authored f32 factor, then truncates to an integer.
    pub fn from_metres(metres: i32, metres_per_mile: f32) -> Result<Self, Error> {
        if metres < 0 || !metres_per_mile.is_finite() || metres_per_mile < 1. {
            return Err(Error::Invalid);
        }
        Ok(Self {
            kilometres: metres / 1000,
            miles: integer(metres as f32 / metres_per_mile)?,
        })
    }
}

fn integer(value: f32) -> Result<i32, Error> {
    if !value.is_finite() || value < 0. || f64::from(value) > f64::from(i32::MAX) {
        return Err(Error::Overflow);
    }
    Ok(value as i32)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    baseline: i32,
    accumulated: f32,
    period: f64,
    clock: f64,
    next: f64,
    anchor: Option<[f32; 3]>,
}

impl Session {
    pub fn new(baseline: i32, now: f64, period: f64) -> Result<Self, Error> {
        if baseline < 0 || !period.is_finite() || period <= 0. {
            return Err(Error::Invalid);
        }
        if !now.is_finite() || now < 0. || !(now + period).is_finite() || now + period <= now {
            return Err(Error::Clock);
        }
        Ok(Self {
            baseline,
            accumulated: 0.,
            period,
            clock: now,
            next: now + period,
            anchor: None,
        })
    }

    pub fn metres(&self) -> Result<i32, Error> {
        self.baseline
            .checked_add(integer(self.accumulated)?)
            .ok_or(Error::Overflow)
    }

    fn check_clock(&self, now: f64) -> Result<(), Error> {
        if !now.is_finite()
            || now < self.clock
            || !(now + self.period).is_finite()
            || now + self.period <= now
        {
            Err(Error::Clock)
        } else {
            Ok(())
        }
    }

    /// Explicit local discontinuity boundary, not an inferred speed threshold.
    /// Retains accumulated fractions and starts a fresh sampling interval. An
    /// absent vehicle drops the anchor; its next sample establishes a new one.
    pub fn rebase(&mut self, now: f64, sample: Option<Sample>) -> Result<(), Error> {
        self.check_clock(now)?;
        if let Some(sample) = sample {
            sample.validate()?;
        }
        self.clock = now;
        self.next = now + self.period;
        self.anchor = sample.map(|s| s.position);
        Ok(())
    }

    /// One delayed event samples the latest position once, without fabricating
    /// intermediate positions when the clock advances by several periods.
    /// f32 accumulation precedes truncation, so sub-metre samples are retained.
    /// Airborne or crashed samples exclude vertical displacement only.
    /// Failure leaves the complete state unchanged.
    pub fn advance(&mut self, now: f64, sample: Option<Sample>) -> Result<Option<i32>, Error> {
        self.check_clock(now)?;
        let Some(sample) = sample else {
            self.rebase(now, None)?;
            return Ok(None);
        };
        sample.validate()?;
        let Some(anchor) = self.anchor else {
            self.rebase(now, Some(sample))?;
            return Ok(None);
        };
        if now < self.next {
            self.clock = now;
            return Ok(None);
        }
        let mut delta = std::array::from_fn(|i| sample.position[i] - anchor[i]);
        if sample.airborne || sample.crashed {
            delta[1] = 0.;
        }
        let distance = vector_length::length(delta).map_err(|_| Error::Overflow)?;
        let accumulated = self.accumulated + distance;
        let metres = self
            .baseline
            .checked_add(integer(accumulated)?)
            .ok_or(Error::Overflow)?;
        self.accumulated = accumulated;
        self.clock = now;
        self.next = now + self.period;
        self.anchor = Some(sample.position);
        Ok(Some(metres))
    }
}

#[cfg(test)]
mod tests;
