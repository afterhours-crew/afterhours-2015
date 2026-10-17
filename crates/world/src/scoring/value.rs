// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed graph values. Scalar Math registers still accept scalar values only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Integer(i32),
    Float(f32),
    Boolean(bool),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    /// Right, up, forward and translation vectors, in that order. No padding.
    Transform([[f32; 3]; 4]),
}
