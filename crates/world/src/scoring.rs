// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded calculations used by installed driving-score graphs.
//! These primitives do not accept awards from clients or persist progression.
pub mod arithmetic;
pub mod boolean_fold;
pub mod collision;
pub mod curve;
pub mod delay;
pub mod draft;
pub mod events;
pub mod interpreter;
pub mod math;
pub mod nodes;
pub mod program;
pub mod properties;
pub mod property_cast;
pub mod property_gate;
pub mod reports;
pub mod road;
pub mod score;
pub mod stopwatch;
pub mod synced;
pub mod value;
pub mod vector_length;
pub mod vehicle_query;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Bound,
    InvalidCurve,
    InvalidProgram,
    UninitializedRegister,
    MissingInput,
    NonFinite,
    ConversionRange,
    InvalidGraph,
    UnknownNode,
    UnsupportedEvent,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "scoring calculation: {self:?}")
    }
}
impl std::error::Error for Error {}

fn finite(value: f32) -> Result<f32, Error> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::NonFinite)
    }
}
