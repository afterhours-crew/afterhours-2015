// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! VectorMathOp Length, including synchronous feedback between its outputs.
use super::{Error, events::PropertyHandler, finite, nodes::Action, value::Value};
pub const INPUT: u32 = 2412549647;
pub const LENGTH: u32 = 2595815478;
pub const NORMAL: u32 = 3768956554;
pub const NORMAL_EPSILON: f32 = f32::from_bits(0x3727c5ac);

pub fn length(vector: [f32; 3]) -> Result<f32, Error> {
    for v in vector {
        finite(v)?;
    }
    let [x, y, z] = vector.map(|v| v * v);
    finite(((y + x) + z).sqrt())
}
/// The vector is reread after Length callbacks, but the original length is
/// retained. A short vector is copied, not replaced with zero.
pub fn normal(vector: [f32; 3], previous_length: f32) -> Result<[f32; 3], Error> {
    finite(previous_length)?;
    let mut result = vector;
    for value in &mut result {
        finite(*value)?;
        if previous_length.abs() > NORMAL_EPSILON {
            *value = finite(*value / previous_length)?;
        }
    }
    Ok(result)
}
pub(crate) fn vector(value: Value) -> Result<[f32; 3], Error> {
    match value {
        Value::Vec3(v) => Ok(v),
        _ => Err(Error::InvalidGraph),
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Node {
    pub(crate) length_bound: bool,
    pub(crate) normal_bound: bool,
    active: bool,
}
impl Node {
    /// Output bindings are fixed for the graph lifetime, initially unready.
    pub fn new(length_bound: bool, normal_bound: bool) -> Self {
        Self {
            length_bound,
            normal_bound,
            active: false,
        }
    }
    pub(crate) fn prepare(&mut self, action: Action) -> bool {
        match action {
            Action::Activate => {
                self.active = true;
                true
            }
            Action::PropertyChanged(_) => self.active,
            Action::Event(_) => false,
        }
    }
}
pub trait Handler: PropertyHandler {
    fn vector_length(&mut self, node: u16) -> Result<&mut Node, Error>;
    /// Read the same resolved vector input cell each time. None means unbound or
    /// unready: both suppress evaluation here. Bindings cannot change mid-call.
    fn vector_length_input(&self, node: u16) -> Result<Option<Value>, Error>;
}
#[cfg(test)]
mod tests;
