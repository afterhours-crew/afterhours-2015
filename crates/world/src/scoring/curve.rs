// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed normalized reaction curves, with finite-input validation.
use super::{Error, finite};

pub const MAX_KNOTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Knot {
    pub x: f32,
    pub y: f32,
    pub coefficient: f32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Interpolation {
    Linear,
    Cubic,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Curve {
    x_range: [f32; 2],
    y_range: [f32; 2],
    input_scale: f32,
    output_scale: f32,
    interpolation: Interpolation,
    knots: Vec<Knot>,
}

impl<'de> serde::Deserialize<'de> for Curve {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            x_range: [f32; 2],
            y_range: [f32; 2],
            input_scale: f32,
            output_scale: f32,
            interpolation: Interpolation,
            knots: Vec<Knot>,
        }
        let f = Fields::deserialize(deserializer)?;
        // Keep authored knot order; `new` intentionally has a stronger contract.
        Self::from_authored(
            f.x_range,
            f.y_range,
            f.input_scale,
            f.output_scale,
            f.interpolation,
            f.knots,
        )
        .map_err(|_| serde::de::Error::custom("invalid prepared curve"))
    }
}
impl Curve {
    pub fn new(
        x_range: [f32; 2],
        y_range: [f32; 2],
        input_scale: f32,
        output_scale: f32,
        interpolation: Interpolation,
        knots: Vec<Knot>,
    ) -> Result<Self, Error> {
        let curve = Self::from_authored(
            x_range,
            y_range,
            input_scale,
            output_scale,
            interpolation,
            knots,
        )?;
        if curve.knots.windows(2).any(|pair| pair[0].x >= pair[1].x) {
            return Err(Error::InvalidCurve);
        }
        Ok(curve)
    }

    /// Preserve installed knot order, including duplicate or decreasing entries.
    /// The native evaluator bisects that exact array; sorting it changes results.
    /// Counts, scalar bounds and finite values are still checked.
    pub fn from_authored(
        x_range: [f32; 2],
        y_range: [f32; 2],
        input_scale: f32,
        output_scale: f32,
        interpolation: Interpolation,
        knots: Vec<Knot>,
    ) -> Result<Self, Error> {
        if !(2..=MAX_KNOTS).contains(&knots.len()) {
            return Err(Error::Bound);
        }
        for value in x_range
            .into_iter()
            .chain(y_range)
            .chain([input_scale, output_scale])
        {
            finite(value)?;
        }
        if x_range[0] >= x_range[1] || y_range[0] > y_range[1] {
            return Err(Error::InvalidCurve);
        }
        finite(x_range[1] - x_range[0])?;
        finite(y_range[1] - y_range[0])?;
        for knot in &knots {
            finite(knot.x)?;
            finite(knot.y)?;
            finite(knot.coefficient)?;
            if !(0.0..=1.0).contains(&knot.x) || !(0.0..=1.0).contains(&knot.y) {
                return Err(Error::InvalidCurve);
            }
        }
        Ok(Self {
            x_range,
            y_range,
            input_scale,
            output_scale,
            interpolation,
            knots,
        })
    }

    /// The default installed trait: scale, clamp and normalize the input,
    /// interpolate, clamp normalized output, restore its range and scale.
    /// Preserve f32 operation order; do not fuse multiply/add operations.
    pub fn evaluate(&self, input: f32) -> Result<f32, Error> {
        let input = finite(finite(input)? * self.input_scale)?;
        let x = (input.clamp(self.x_range[0], self.x_range[1]) - self.x_range[0])
            / (self.x_range[1] - self.x_range[0]);
        // Knots need not cover the complete normalized domain. The outer
        // segments extrapolate to the header bounds before output clamping.
        let (mut left, mut right) = (0, self.knots.len() - 1);
        while right - left > 1 {
            let middle = (left + right) / 2;
            if x < self.knots[middle].x {
                right = middle;
            } else {
                left = middle;
            }
        }
        let (left, right) = (self.knots[left], self.knots[right]);
        let width = right.x - left.x;
        // The native function returns zero before range restoration when the
        // selected segment has no positive width.
        if width <= 0. {
            return Ok(0.);
        }
        let value = match self.interpolation {
            Interpolation::Linear => {
                let slope = (right.y - left.y) / width;
                (left.y - left.x * slope) + slope * x
            }
            Interpolation::Cubic => {
                let reciprocal = 1.0 / width;
                let a = (right.x - x) * reciprocal;
                let b = (x - left.x) * reciprocal;
                let correction = ((a * a * a - a) * left.coefficient
                    + (b * b * b - b) * right.coefficient)
                    * (width * width);
                correction * f32::from_bits(0x3e2a_aaab) + (a * left.y + b * right.y)
            }
        };
        let scaled =
            finite(value)?.clamp(0.0, 1.0) * (self.y_range[1] - self.y_range[0]) + self.y_range[0];
        finite(finite(scaled)? * self.output_scale)
    }
}

#[cfg(test)]
mod tests;
