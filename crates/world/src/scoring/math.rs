// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Validated typed subset of installed Math assemblies. Evaluation uses fresh
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
    Vec3,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Register {
    Integer(u8),
    Float(u8),
    Boolean(u8),
    Vec3(u8),
}
impl Register {
    fn parts(self) -> (usize, usize) {
        match self {
            Self::Integer(i) => (0, usize::from(i)),
            Self::Float(i) => (1, usize::from(i)),
            Self::Boolean(i) => (2, usize::from(i)),
            Self::Vec3(i) => (3, usize::from(i)),
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
    Min,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerOp {
    Subtract,
    Min,
    Max,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    Greater,
    GreaterEqual,
    LessEqual,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operation {
    Boolean(bool),
    Integer(i32),
    Float(f32),
    InputInteger(u32),
    InputFloat(u32),
    InputVec3(u32),
    AddInteger(u8, u8),
    BinaryInteger(IntegerOp, u8, u8),
    ClampInteger {
        value: u8,
        min: u8,
        max: u8,
    },
    BinaryFloat(FloatOp, u8, u8),
    LessFloat(u8, u8),
    CompareFloat(Comparison, u8, u8),
    EqualInteger(u8, u8),
    AndBoolean(u8, u8),
    ArcCos(u8),
    /// `normv3` returns the vector's length, not a normalized vector.
    NormVec3(u8),
    DistanceVec3(u8, u8),
    FloatFromInteger(u8),
    SelectFloat {
        condition: u8,
        yes: u8,
        no: u8,
    },
    SelectBoolean {
        condition: u8,
        yes: u8,
        no: u8,
    },
    RoundFloat(u8),
    /// Native `intf` (callback `0x3765420`, E995): `cvttss2si` from a float
    /// register into an integer register, truncating toward zero.
    IntegerFromFloat(u8),
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
            Operation::InputVec3(port) => Some((port, Type::Vec3)),
            _ => None,
        })
    }

    /// The validated final return selects the output's type, not its value.
    pub fn output_type(&self) -> Type {
        match self.instructions.last().map(|i| i.operation) {
            Some(Operation::Return(Register::Integer(_))) => Type::Integer,
            Some(Operation::Return(Register::Float(_))) => Type::Float,
            Some(Operation::Return(Register::Boolean(_))) => Type::Boolean,
            Some(Operation::Return(Register::Vec3(_))) => Type::Vec3,
            _ => unreachable!("validated final return"),
        }
    }

    /// Reject unsupported shapes and reads of undefined typed registers before
    /// accepting a program. Return must be its final instruction.
    pub fn new(instructions: Vec<Instruction>) -> Result<Self, Error> {
        if instructions.is_empty() || instructions.len() > MAX_INSTRUCTIONS {
            return Err(Error::Bound);
        }
        let mut defined = [[false; REGISTERS]; 4];
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
                Operation::Boolean(_) => Type::Boolean,
                Operation::Integer(_) | Operation::InputInteger(_) => Type::Integer,
                Operation::Float(value) => {
                    finite(value)?;
                    Type::Float
                }
                Operation::InputFloat(_) => Type::Float,
                Operation::InputVec3(_) => Type::Vec3,
                Operation::AddInteger(a, b) | Operation::BinaryInteger(_, a, b) => {
                    read(Register::Integer(a))?;
                    read(Register::Integer(b))?;
                    Type::Integer
                }
                Operation::BinaryFloat(_, a, b) => {
                    read(Register::Float(a))?;
                    read(Register::Float(b))?;
                    Type::Float
                }
                Operation::LessFloat(a, b) | Operation::CompareFloat(_, a, b) => {
                    read(Register::Float(a))?;
                    read(Register::Float(b))?;
                    Type::Boolean
                }
                Operation::ClampInteger { value, min, max } => {
                    for r in [value, min, max] {
                        read(Register::Integer(r))?;
                    }
                    Type::Integer
                }
                Operation::AndBoolean(a, b) => {
                    read(Register::Boolean(a))?;
                    read(Register::Boolean(b))?;
                    Type::Boolean
                }
                Operation::EqualInteger(a, b) => {
                    read(Register::Integer(a))?;
                    read(Register::Integer(b))?;
                    Type::Boolean
                }
                Operation::SelectBoolean { condition, yes, no } => {
                    for r in [condition, yes, no] {
                        read(Register::Boolean(r))?;
                    }
                    Type::Boolean
                }
                Operation::ArcCos(a) => {
                    read(Register::Float(a))?;
                    Type::Float
                }
                Operation::NormVec3(a) => {
                    read(Register::Vec3(a))?;
                    Type::Float
                }
                Operation::DistanceVec3(a, b) => {
                    read(Register::Vec3(a))?;
                    read(Register::Vec3(b))?;
                    Type::Float
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
                Operation::IntegerFromFloat(a) => {
                    read(Register::Float(a))?;
                    Type::Integer
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
                Type::Vec3 => 3,
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
        let mut vectors = [[0.; 3]; REGISTERS];
        for instruction in &self.instructions {
            let result = usize::from(instruction.result);
            match instruction.operation {
                Operation::Boolean(value) => booleans[result] = value,
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
                Operation::InputVec3(port) => {
                    let Some(Value::Vec3(value)) = input(port, Type::Vec3) else {
                        return Err(Error::MissingInput);
                    };
                    for v in value {
                        finite(v)?;
                    }
                    vectors[result] = value;
                }
                Operation::AddInteger(a, b) => {
                    integers[result] =
                        integers[usize::from(a)].wrapping_add(integers[usize::from(b)])
                }
                Operation::BinaryInteger(op, a, b) => {
                    let (a, b) = (integers[usize::from(a)], integers[usize::from(b)]);
                    integers[result] = match op {
                        IntegerOp::Subtract => a.wrapping_sub(b),
                        IntegerOp::Min => a.min(b),
                        IntegerOp::Max => a.max(b),
                    };
                }
                Operation::ClampInteger { value, min, max } => {
                    // Native max(value,min), then min(result,max), including
                    // reversed bounds. Rust's clamp would panic in that case.
                    integers[result] = integers[usize::from(value)]
                        .max(integers[usize::from(min)])
                        .min(integers[usize::from(max)]);
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
                        FloatOp::Min => {
                            if a < b {
                                a
                            } else {
                                b
                            }
                        }
                    })?;
                }
                Operation::LessFloat(a, b) => {
                    booleans[result] = floats[usize::from(a)] < floats[usize::from(b)]
                }
                Operation::CompareFloat(op, a, b) => {
                    let (a, b) = (floats[usize::from(a)], floats[usize::from(b)]);
                    booleans[result] = match op {
                        Comparison::Greater => a > b,
                        Comparison::GreaterEqual => a >= b,
                        Comparison::LessEqual => a <= b,
                    };
                }
                Operation::AndBoolean(a, b) => {
                    booleans[result] = booleans[usize::from(a)] && booleans[usize::from(b)];
                }
                Operation::EqualInteger(a, b) => {
                    booleans[result] = integers[usize::from(a)] == integers[usize::from(b)];
                }
                Operation::SelectBoolean { condition, yes, no } => {
                    booleans[result] = booleans[usize::from(if booleans[usize::from(condition)] {
                        yes
                    } else {
                        no
                    })];
                }
                Operation::ArcCos(a) => floats[result] = finite(floats[usize::from(a)].acos())?,
                Operation::NormVec3(a) => floats[result] = norm(vectors[usize::from(a)])?,
                Operation::DistanceVec3(a, b) => {
                    let (a, b) = (vectors[usize::from(a)], vectors[usize::from(b)]);
                    floats[result] = norm([a[0] - b[0], a[1] - b[1], a[2] - b[2]])?;
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
                Operation::IntegerFromFloat(a) => {
                    let value = finite(floats[usize::from(a)])?;
                    // cvttss2si; outside its range the native result is the
                    // integer indefinite value, which this model refuses.
                    if !(-2_147_483_648.0..2_147_483_648.0).contains(&value) {
                        return Err(Error::ConversionRange);
                    }
                    integers[result] = value as i32;
                }
                Operation::Return(register) => {
                    return Ok(Some(match register {
                        Register::Integer(index) => Value::Integer(integers[usize::from(index)]),
                        Register::Float(index) => Value::Float(floats[usize::from(index)]),
                        Register::Boolean(index) => Value::Boolean(booleans[usize::from(index)]),
                        Register::Vec3(index) => Value::Vec3(vectors[usize::from(index)]),
                    }));
                }
            }
        }
        unreachable!("validated final return")
    }
}

fn norm(v: [f32; 3]) -> Result<f32, Error> {
    // SSE reduction is (y*y + x*x) + z*z, without fused multiply/add.
    finite(((v[1] * v[1] + v[0] * v[0]) + v[2] * v[2]).sqrt())
}

#[cfg(test)]
mod tests;
