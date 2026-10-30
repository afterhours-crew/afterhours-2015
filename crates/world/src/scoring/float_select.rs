// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! FloatSelectEntity: binding evaluates both inputs, then selects by sign.
//! Native 03767130 / 037562a0; any property callback republishes (0376fda0).
use super::{Error, value::Value};

pub const POSITIVE: u32 = 0x2c94_b751;
pub const NEGATIVE: u32 = 0x0cc8_d26e;
pub const SELECT: u32 = 0xc6f6_466d;
pub const OUT: u32 = 0x0b87_df4b;

pub fn evaluate(mut read: impl FnMut(u32) -> Result<Option<Value>, Error>) -> Result<f32, Error> {
    let mut float = |field| match read(field)? {
        Some(Value::Float(value)) => {
            super::finite(value)?;
            Ok(value)
        }
        None => Err(Error::MissingInput),
        _ => Err(Error::InvalidGraph),
    };
    // Native eagerly resolves both branches, including the unselected one.
    let positive = float(POSITIVE)?;
    let negative = float(NEGATIVE)?;
    let select = float(SELECT)?;
    Ok(if select < 0.0 { negative } else { positive })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_is_positive_and_both_branches_are_resolved_in_native_order() {
        for select in [-1.0, -f32::MIN_POSITIVE, -0.0, 0.0, 1.0] {
            let mut reads = vec![];
            let value = evaluate(|field| {
                reads.push(field);
                Ok(Some(Value::Float(match field {
                    POSITIVE => 2.,
                    NEGATIVE => 3.,
                    SELECT => select,
                    _ => unreachable!(),
                })))
            })
            .unwrap();
            assert_eq!(reads, [POSITIVE, NEGATIVE, SELECT]);
            assert_eq!(value, if select < 0. { 3. } else { 2. });
        }
        assert_eq!(
            evaluate(|field| Ok(if field == NEGATIVE {
                None
            } else {
                Some(Value::Float(1.))
            })),
            Err(Error::MissingInput)
        );
        assert!(evaluate(|_| Ok(Some(Value::Float(f32::NAN)))).is_err());
        assert!(evaluate(|_| Ok(Some(Value::Integer(1)))).is_err());
    }
}
