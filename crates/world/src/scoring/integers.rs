// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Integer graph operations used by event award selection. RankIntegers uses
//! descending, stable insertion order for up to 28 inputs in the native client.
//! Larger native sorts have a different tie contract and are not modeled here.
use super::{Error, nodes::OUT, value::Value};

pub const MAX_RANK_INPUTS: u16 = 28;
pub const SORT: u32 = 0x3cc0_d267;
pub const RANK: u32 = (-892_384_772i32) as u32;
pub const COUNT: u32 = 208_911_203;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Add,
    Subtract,
    Divide,
}

/// MathIntOp folds left to right. Unready inputs use zero. Division by zero
/// resets the accumulator to zero and continues; signed division truncates.
pub fn fold(
    operations: &[Operation],
    mut read: impl FnMut(u32) -> Result<Option<Value>, Error>,
) -> Result<(u32, Value), Error> {
    if operations.len() > super::arithmetic::MAX_OPERATIONS {
        return Err(Error::Bound);
    }
    let mut input = |slot| match read(port("In", slot))? {
        Some(Value::Integer(v)) => Ok(v),
        None => Ok(0),
        _ => Err(Error::InvalidGraph),
    };
    let mut value = input(0)?;
    for (slot, op) in operations.iter().enumerate() {
        let right = input(slot as u16 + 1)?;
        value = match op {
            Operation::Add => value.wrapping_add(right),
            Operation::Subtract => value.wrapping_sub(right),
            Operation::Divide if right == 0 => 0,
            // The native idiv traps on MIN/-1. Refuse it transactionally.
            Operation::Divide => value.checked_div(right).ok_or(Error::ConversionRange)?,
        };
    }
    Ok((OUT, Value::Integer(value)))
}

pub fn port(prefix: &str, index: u16) -> u32 {
    format!("{prefix}{index}")
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b))
}

pub fn rank(values: &[i32]) -> Result<Vec<i32>, Error> {
    if values.is_empty() || values.len() > usize::from(MAX_RANK_INPUTS) {
        return Err(Error::Bound);
    }
    let mut order: Vec<_> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[b].cmp(&values[a]));
    let mut ranks = vec![0; values.len()];
    for (rank, index) in order.into_iter().enumerate() {
        ranks[index] = rank as i32 + 1;
    }
    Ok(ranks)
}

/// Signed 32-bit addition. The native MathIntOp getter falls back to zero
/// for unbound/unready inputs; addition wraps as a 32-bit machine operation.
pub fn sum(
    count: u16,
    mut read: impl FnMut(u32) -> Result<Option<Value>, Error>,
) -> Result<(u32, Value), Error> {
    if count == 0 || usize::from(count) > super::arithmetic::MAX_OPERATIONS + 1 {
        return Err(Error::Bound);
    }
    let mut value = 0i32;
    for slot in 0..count {
        value = value.wrapping_add(match read(port("In", slot))? {
            Some(Value::Integer(v)) => v,
            None => 0,
            _ => return Err(Error::InvalidGraph),
        });
    }
    Ok((OUT, Value::Integer(value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_integer_operations_zero_divisors_and_overflow() {
        let run = |ops: &[Operation], values: &[i32]| {
            fold(ops, |p| {
                Ok(values
                    .iter()
                    .enumerate()
                    .find(|(i, _)| port("In", *i as u16) == p)
                    .map(|(_, v)| Value::Integer(*v)))
            })
        };
        assert_eq!(
            run(&[Operation::Subtract, Operation::Divide], &[17, 4, 3])
                .unwrap()
                .1,
            Value::Integer(4)
        );
        assert_eq!(
            run(&[Operation::Divide, Operation::Add], &[-17, 3, 1])
                .unwrap()
                .1,
            Value::Integer(-4)
        );
        assert_eq!(
            run(&[Operation::Divide, Operation::Add], &[17, 0, 8])
                .unwrap()
                .1,
            Value::Integer(8)
        );
        assert_eq!(
            run(&[Operation::Subtract], &[i32::MIN, 1]).unwrap().1,
            Value::Integer(i32::MAX)
        );
        assert_eq!(
            run(&[Operation::Divide], &[i32::MIN, -1]),
            Err(Error::ConversionRange)
        );
        assert_eq!(
            run(&[Operation::Divide], &[17]).unwrap().1,
            Value::Integer(0)
        );
        assert!(
            run(
                &vec![Operation::Add; super::super::arithmetic::MAX_OPERATIONS + 1],
                &[]
            )
            .is_err()
        );
    }
    #[test]
    fn rank_is_descending_one_based_and_stable_at_ties() {
        assert_eq!(rank(&[7, 9, 9, -1]).unwrap(), [3, 1, 2, 4]);
        assert_eq!(rank(&[0; 19]).unwrap(), (1..=19).collect::<Vec<_>>());
        assert!(rank(&[]).is_err());
        assert!(rank(&[0; 29]).is_err());
    }
    #[test]
    fn sum_falls_back_to_zero_and_wraps_without_accepting_other_types() {
        assert_eq!(
            sum(3, |p| Ok(if p == port("In", 0) {
                Some(Value::Integer(i32::MAX))
            } else if p == port("In", 2) {
                Some(Value::Integer(1))
            } else {
                None
            }))
            .unwrap()
            .1,
            Value::Integer(i32::MIN)
        );
        assert!(sum(2, |_| Ok(Some(Value::Boolean(true)))).is_err());
    }
}
