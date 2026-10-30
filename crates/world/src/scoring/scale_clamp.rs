// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native ScaleClamp (045176f0). Fresh binding leaves Out unready; only a
//! property callback evaluates and publishes it (0451a620 / 0451ca60).
use super::{Error, value::Value};

pub const INPUT: u32 = 0x0059_7302;
pub const IN_MIN: u32 = 0x0cc8_c528;
pub const IN_MAX: u32 = 0x0cc8_c636;
pub const OUT_MIN: u32 = 0xac99_9fc1;
pub const OUT_MAX: u32 = 0xac99_9edf;
pub const OUT: u32 = 0x0b87_df4b;
pub const INPUTS: [u32; 5] = [INPUT, IN_MIN, IN_MAX, OUT_MIN, OUT_MAX];
const EPSILON: f32 = f32::from_bits(0x3586_37bd);

pub fn evaluate(mut read: impl FnMut(u32) -> Result<Option<Value>, Error>) -> Result<f32, Error> {
    let mut values = [0.; 5];
    for (field, value) in INPUTS.into_iter().zip(&mut values) {
        *value = match read(field)? {
            Some(Value::Float(value)) => value,
            None => return Err(Error::MissingInput),
            _ => return Err(Error::InvalidGraph),
        };
        super::finite(*value)?;
    }
    let [input, min, max, low, high] = values;
    let width = max - min;
    // A tiny negative interval uses positive epsilon too. The two ordered
    // comparisons do not reorder reversed bounds (unlike a sorted clamp).
    let denominator = if width.abs() < EPSILON {
        EPSILON
    } else {
        width
    };
    let raised = if min > input { min } else { input };
    let clipped = if max < raised { max } else { raised };
    let fraction = (clipped - min) / denominator;
    let scaled = fraction * (high - low);
    let result = scaled + low;
    super::finite(result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn eval(values: [f32; 5]) -> Result<f32, Error> {
        evaluate(|port| {
            Ok(Some(Value::Float(
                values[INPUTS.iter().position(|&p| p == port).unwrap()],
            )))
        })
    }
    #[test]
    fn clamp_scale_and_native_degenerate_range_order() {
        for (input, expected) in [(-1., 10.), (0., 10.), (5., 15.), (10., 20.), (11., 20.)] {
            assert_eq!(eval([input, 0., 10., 10., 20.]), Ok(expected));
        }
        assert_eq!(eval([10., 3., 3., 7., 9.]), Ok(7.));
        assert_eq!(eval([5., 10., 0., 10., 20.]), Ok(20.));
        assert_eq!(eval([0., EPSILON / 2., 0., 0., 1.]), Ok(-0.5));
        assert_eq!(eval([1., 0., EPSILON / 2., 0., 1.]), Ok(0.5));
        assert!(eval([f32::NAN, 0., 1., 0., 1.]).is_err());
        assert!(eval([1., 0., 1., -f32::MAX, f32::MAX]).is_err());
        assert_eq!(evaluate(|_| Ok(None)), Err(Error::MissingInput));
    }
}
