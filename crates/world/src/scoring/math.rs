// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Validated scalar subset of installed Math assemblies. Evaluation uses fresh
//! typed registers; state changes occur only when the caller accepts an output.
pub use super::value::Value;
use super::{Error, finite};

pub const MAX_INSTRUCTIONS: usize = 256;
pub const REGISTERS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type {
    Integer,
    Float,
    Boolean,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Register {
    Integer(u8),
    Float(u8),
    Boolean(u8),
}
impl Register {
    fn parts(self) -> (usize, usize) {
        match self {
            Self::Integer(i) => (0, usize::from(i)),
            Self::Float(i) => (1, usize::from(i)),
            Self::Boolean(i) => (2, usize::from(i)),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operation {
    Integer(i32),
    Float(f32),
    InputInteger(u32),
    InputFloat(u32),
    AddInteger(u8, u8),
    BinaryFloat(FloatOp, u8, u8),
    LessFloat(u8, u8),
    FloatFromInteger(u8),
    SelectFloat { condition: u8, yes: u8, no: u8 },
    RoundFloat(u8),
    Return(Register),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instruction {
    pub result: u8,
    pub operation: Operation,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    instructions: Vec<Instruction>,
}
impl Program {
    /// Typed input reads in instruction order, including repeated reads. A graph
    /// binder must reject incompatible types for the same bound property.
    pub fn inputs(&self) -> impl Iterator<Item = (u32, Type)> + '_ {
        self.instructions.iter().filter_map(|i| match i.operation {
            Operation::InputInteger(port) => Some((port, Type::Integer)),
            Operation::InputFloat(port) => Some((port, Type::Float)),
            _ => None,
        })
    }

    /// The validated final return selects the output's type, not its value.
    pub fn output_type(&self) -> Type {
        match self.instructions.last().map(|i| i.operation) {
            Some(Operation::Return(Register::Integer(_))) => Type::Integer,
            Some(Operation::Return(Register::Float(_))) => Type::Float,
            Some(Operation::Return(Register::Boolean(_))) => Type::Boolean,
            _ => unreachable!("validated final return"),
        }
    }

    /// Reject unsupported shapes and reads of undefined typed registers before
    /// accepting a program. Return must be its final instruction.
    pub fn new(instructions: Vec<Instruction>) -> Result<Self, Error> {
        if instructions.is_empty() || instructions.len() > MAX_INSTRUCTIONS {
            return Err(Error::Bound);
        }
        let mut defined = [[false; REGISTERS]; 3];
        for (index, instruction) in instructions.iter().enumerate() {
            let read = |register: Register| {
                let (kind, slot) = register.parts();
                match defined[kind].get(slot) {
                    Some(true) => Ok(()),
                    Some(false) => Err(Error::UninitializedRegister),
                    None => Err(Error::Bound),
                }
            };
            let output = match instruction.operation {
                Operation::Integer(_) | Operation::InputInteger(_) => Type::Integer,
                Operation::Float(value) => {
                    finite(value)?;
                    Type::Float
                }
                Operation::InputFloat(_) => Type::Float,
                Operation::AddInteger(a, b) => {
                    read(Register::Integer(a))?;
                    read(Register::Integer(b))?;
                    Type::Integer
                }
                Operation::BinaryFloat(_, a, b) => {
                    read(Register::Float(a))?;
                    read(Register::Float(b))?;
                    Type::Float
                }
                Operation::LessFloat(a, b) => {
                    read(Register::Float(a))?;
                    read(Register::Float(b))?;
                    Type::Boolean
                }
                Operation::FloatFromInteger(a) => {
                    read(Register::Integer(a))?;
                    Type::Float
                }
                Operation::SelectFloat { condition, yes, no } => {
                    read(Register::Boolean(condition))?;
                    read(Register::Float(yes))?;
                    read(Register::Float(no))?;
                    Type::Float
                }
                Operation::RoundFloat(a) => {
                    read(Register::Float(a))?;
                    Type::Float
                }
                Operation::Return(register) => {
                    read(register)?;
                    if index + 1 != instructions.len() || instruction.result != 0 {
                        return Err(Error::InvalidProgram);
                    }
                    continue;
                }
            };
            let kind = match output {
                Type::Integer => 0,
                Type::Float => 1,
                Type::Boolean => 2,
            };
            *defined[kind]
                .get_mut(usize::from(instruction.result))
                .ok_or(Error::Bound)? = true;
        }
        if !matches!(
            instructions.last().map(|i| i.operation),
            Some(Operation::Return(_))
        ) {
            return Err(Error::InvalidProgram);
        }
        Ok(Self { instructions })
    }

    /// `None` means division by zero aborted evaluation before publishing.
    /// The caller must retain its prior output, not replace it with zero.
    /// Nonfinite inputs/results and out-of-range conversions are rejected.
    pub fn evaluate(
        &self,
        mut input: impl FnMut(u32, Type) -> Option<Value>,
    ) -> Result<Option<Value>, Error> {
        let mut integers = [0i32; REGISTERS];
        let mut floats = [0f32; REGISTERS];
        let mut booleans = [false; REGISTERS];
        for instruction in &self.instructions {
            let result = usize::from(instruction.result);
            match instruction.operation {
                Operation::Integer(value) => integers[result] = value,
                Operation::Float(value) => floats[result] = value,
                Operation::InputInteger(port) => {
                    let Some(Value::Integer(value)) = input(port, Type::Integer) else {
                        return Err(Error::MissingInput);
                    };
                    integers[result] = value;
                }
                Operation::InputFloat(port) => {
                    let Some(Value::Float(value)) = input(port, Type::Float) else {
                        return Err(Error::MissingInput);
                    };
                    floats[result] = finite(value)?;
                }
                Operation::AddInteger(a, b) => {
                    integers[result] =
                        integers[usize::from(a)].wrapping_add(integers[usize::from(b)])
                }
                Operation::BinaryFloat(op, a, b) => {
                    let (a, b) = (floats[usize::from(a)], floats[usize::from(b)]);
                    floats[result] = finite(match op {
                        FloatOp::Add => a + b,
                        FloatOp::Subtract => a - b,
                        FloatOp::Multiply => a * b,
                        FloatOp::Divide if b == 0.0 => return Ok(None),
                        FloatOp::Divide => a / b,
                        // Native scalar power calls the platform math library.
                        // Its last-bit precision is platform-dependent.
                        FloatOp::Power => a.powf(b),
                    })?;
                }
                Operation::LessFloat(a, b) => {
                    booleans[result] = floats[usize::from(a)] < floats[usize::from(b)]
                }
                Operation::FloatFromInteger(a) => floats[result] = integers[usize::from(a)] as f32,
                Operation::SelectFloat { condition, yes, no } => {
                    floats[result] = floats[usize::from(if booleans[usize::from(condition)] {
                        yes
                    } else {
                        no
                    })]
                }
                Operation::RoundFloat(a) => {
                    let shifted = finite(floats[usize::from(a)] + 0.5)?;
                    // cvttss2si followed by signed integer-to-float conversion.
                    // Reject values outside that instruction's defined range.
                    if !(-2_147_483_648.0..2_147_483_648.0).contains(&shifted) {
                        return Err(Error::ConversionRange);
                    }
                    floats[result] = (shifted as i32) as f32;
                }
                Operation::Return(register) => {
                    return Ok(Some(match register {
                        Register::Integer(index) => Value::Integer(integers[usize::from(index)]),
                        Register::Float(index) => Value::Float(floats[usize::from(index)]),
                        Register::Boolean(index) => Value::Boolean(booleans[usize::from(index)]),
                    }));
                }
            }
        }
        unreachable!("validated final return")
    }
}

#[cfg(test)]
mod tests;
