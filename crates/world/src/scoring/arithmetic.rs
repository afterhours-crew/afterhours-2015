// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ordered MathOp folds. This is distinct from the register-based Math program.
use super::{
    Error, finite,
    nodes::{Action, Effect, OUT},
    value::Value,
};
use std::collections::BTreeSet;

pub const MAX_OPERATIONS: usize = 256;
pub const IN0: u32 = 193450866;
pub const DIVISOR_EPSILON: f32 = f32::from_bits(0x358637bd);
pub const SMALL_DIVISOR_MULTIPLIER: f32 = f32::from_bits(0x49742400);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Min,
    Max,
}
impl Operation {
    pub fn evaluate(self, left: f32, right: f32) -> Result<f32, Error> {
        finite(left)?;
        finite(right)?;
        finite(match self {
            Self::Add => left + right,
            Self::Subtract => left - right,
            Self::Multiply => left * right,
            Self::Divide if right.abs() < DIVISOR_EPSILON => left * SMALL_DIVISOR_MULTIPLIER,
            Self::Divide => left / right,
            // Equal operands select the right-hand bits, including signed zero.
            Self::Min if left < right => left,
            Self::Max if left > right => left,
            Self::Min | Self::Max => right,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    operations: Vec<Operation>,
    inputs: Vec<Option<u32>>,
    active: bool,
}
impl Node {
    /// Connected slots range from In0 through InN for N operations. Unbound
    /// inputs use zero; a connected but unready input suppresses publication.
    /// Bind Out with no initial value. Saved-state restoration is not modeled.
    pub fn new(operations: Vec<Operation>, connected: &[u16]) -> Result<Self, Error> {
        if operations.len() > MAX_OPERATIONS || connected.len() > MAX_OPERATIONS + 1 {
            return Err(Error::Bound);
        }
        let mut slots = BTreeSet::new();
        for &slot in connected {
            if slot as usize > operations.len() || !slots.insert(slot) {
                return Err(Error::InvalidGraph);
            }
        }
        let inputs = (0..=operations.len())
            .map(|slot| {
                if !slots.contains(&(slot as u16)) {
                    Ok(None)
                } else if slot == 0 {
                    Ok(Some(IN0))
                } else {
                    super::boolean_fold::input_port(slot as u16).map(Some)
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            operations,
            inputs,
            active: false,
        })
    }
    /// Retain node state before publishing the returned effect, within the
    /// graph transaction. Equal values still publish; events do not evaluate.
    pub fn apply(
        &mut self,
        action: Action,
        mut read: impl FnMut(u32) -> Result<Option<Value>, Error>,
    ) -> Result<Option<Effect>, Error> {
        let mut candidate = self.clone();
        match action {
            Action::Activate => candidate.active = true,
            Action::PropertyChanged(_) if candidate.active => {}
            _ => return Ok(None),
        }
        let output = candidate.evaluate(&mut read)?;
        *self = candidate;
        Ok(output.map(|v| Effect::Publish(OUT, Value::Float(v))))
    }
    fn evaluate(
        &self,
        read: &mut impl FnMut(u32) -> Result<Option<Value>, Error>,
    ) -> Result<Option<f32>, Error> {
        let mut left = 0.0;
        for (slot, port) in self.inputs.iter().enumerate() {
            let right = match port {
                None => 0.0,
                Some(port) => match read(*port)? {
                    Some(Value::Float(v)) => finite(v)?,
                    None => return Ok(None),
                    _ => return Err(Error::InvalidGraph),
                },
            };
            left = if slot == 0 {
                right
            } else {
                self.operations[slot - 1].evaluate(left, right)?
            };
        }
        Ok(Some(left))
    }
}

#[cfg(test)]
mod tests;
