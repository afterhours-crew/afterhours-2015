// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned vehicle measurements for physics and drift scoring queries.
//! These are scheduled local queries, not client-reported REP awards.
use super::{Error, events::PropertyHandler, finite, properties::Kind, value::Value};

pub const TRANSFORM: u32 = 0x8752_5469;
pub const VELOCITY: u32 = 0x4aa8_68cf;
pub const AIRBORNE: u32 = 0x4e55_9cf1;
/// Boolean vehicle state; its gameplay meaning is not yet established.
pub const STATE_3C230E35: u32 = 0x3c23_0e35;
/// Boolean vehicle state; its gameplay meaning is not yet established.
pub const STATE_CD5AE2F0: u32 = 0xcd5a_e2f0;
pub const MAX_RPM: u32 = 0x9d86_40be;
pub const RPM: u32 = 0x0b88_09ea;
pub const DRIFT_ANGLE: u32 = 0xd547_c1b9;

pub const IDENTITY: [[f32; 3]; 4] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Query {
    Physics,
    Drift,
    Draft,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub field: u32,
    pub kind: Kind,
    /// Constructor-supplied value, not permission to overwrite a shared cell.
    /// None leaves the binding uninitialized and preserves existing cell state.
    pub initial: Option<Value>,
}
const PHYSICS: [Output; 7] = [
    Output {
        field: TRANSFORM,
        kind: Kind::Transform,
        initial: Some(Value::Transform(IDENTITY)),
    },
    Output {
        field: VELOCITY,
        kind: Kind::Vec3,
        initial: Some(Value::Vec3([0.; 3])),
    },
    Output {
        field: AIRBORNE,
        kind: Kind::Boolean,
        initial: Some(Value::Boolean(false)),
    },
    Output {
        field: STATE_3C230E35,
        kind: Kind::Boolean,
        initial: Some(Value::Boolean(false)),
    },
    Output {
        field: STATE_CD5AE2F0,
        kind: Kind::Boolean,
        initial: Some(Value::Boolean(false)),
    },
    Output {
        field: MAX_RPM,
        kind: Kind::Float,
        initial: None,
    },
    Output {
        field: RPM,
        kind: Kind::Float,
        initial: None,
    },
];
const DRIFT: [Output; 2] = [
    Output {
        field: VELOCITY,
        kind: Kind::Vec3,
        initial: Some(Value::Vec3([0.; 3])),
    },
    Output {
        field: DRIFT_ANGLE,
        kind: Kind::Float,
        initial: Some(Value::Float(0.)),
    },
];
const DRAFT: [Output; 3] = [
    Output {
        field: super::draft::STRENGTH,
        kind: Kind::Float,
        initial: Some(Value::Float(0.)),
    },
    Output {
        field: super::draft::DELTA_SPEED,
        kind: Kind::Float,
        initial: Some(Value::Float(0.)),
    },
    Output {
        field: super::draft::ACTIVE,
        kind: Kind::Boolean,
        initial: Some(Value::Boolean(false)),
    },
];
impl Query {
    /// Constructor binding order. Physics update order starts with velocity,
    /// then transform, followed by the remaining outputs in this order.
    pub fn outputs(self) -> &'static [Output] {
        match self {
            Self::Physics => &PHYSICS,
            Self::Drift => &DRIFT,
            Self::Draft => &DRAFT,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bindings {
    pub(crate) query: Query,
    // None = unbound; Some(bool) = installed equality-suppression flag.
    outputs: [Option<bool>; 7],
}
impl Bindings {
    /// Supply every connected output, with its actual binding flag. Unknown or
    /// duplicate ports fail; never filter them to make a query appear supported.
    pub fn new(query: Query, outputs: &[(u32, bool)]) -> Result<Self, Error> {
        if outputs.len() > query.outputs().len() {
            return Err(Error::Bound);
        }
        let mut result = Self {
            query,
            outputs: [None; 7],
        };
        for &(field, suppress_equal) in outputs {
            let index = query
                .outputs()
                .iter()
                .position(|o| o.field == field)
                .ok_or(Error::InvalidGraph)?;
            if result.outputs[index].replace(suppress_equal).is_some() {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(result)
    }
    pub(crate) fn suppression(self, field: u32) -> Option<bool> {
        self.query
            .outputs()
            .iter()
            .position(|o| o.field == field)
            .and_then(|i| self.outputs[i])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriftSample {
    pub transform: [[f32; 3]; 4],
    pub velocity: [f32; 3],
}
impl DriftSample {
    pub(crate) fn validate(self) -> Result<(), Error> {
        Kind::Transform.validate(Value::Transform(self.transform))?;
        Kind::Vec3.validate(Value::Vec3(self.velocity))
    }
    /// Signed radians: atan(lateral / abs(forward)), except that a forward
    /// magnitude below f32::EPSILON uses the lateral value directly. This is
    /// deliberately not atan2. Dot products round each multiply/add to f32.
    /// Platform atan implementations need not be bit-identical to the client CRT.
    pub fn angle(self) -> Result<f32, Error> {
        self.validate()?;
        let dot = |axis: [f32; 3]| -> Result<f32, Error> {
            let x = finite(axis[0] * self.velocity[0])?;
            let y = finite(axis[1] * self.velocity[1])?;
            let z = finite(axis[2] * self.velocity[2])?;
            finite(finite(y + x)? + z)
        };
        let forward = dot(self.transform[2])?.abs();
        let lateral = dot(self.transform[0])?;
        let ratio = if forward >= f32::EPSILON {
            finite(lateral / forward)?
        } else {
            lateral
        };
        finite(ratio.atan())
    }
}

pub trait Handler: PropertyHandler {
    /// A stable handle to this query owner's current vehicle/physics state.
    /// It must remain valid for this transaction, including nested callbacks.
    type Vehicle: Copy;
    fn vehicle_query(&self, node: u16) -> Result<Bindings, Error>;
    /// Resolve once per update. Missing participant, vehicle or physics data
    /// returns None and preserves all query outputs. Validate ownership here.
    fn query_vehicle(&mut self, node: u16) -> Result<Option<Self::Vehicle>, Error>;
    /// Read each physics field in update order, after earlier callbacks finish.
    /// Unknown state flags must come from established owned measurements; this
    /// API does not infer them from acceleration, contacts or input keys.
    fn physics_measurement(&mut self, vehicle: Self::Vehicle, field: u32) -> Result<Value, Error>;
    /// Drift snapshots transform and velocity together before any publication.
    fn drift_measurement(&mut self, vehicle: Self::Vehicle) -> Result<DriftSample, Error>;
    /// Evaluate the current owned participant set with draft::Configuration.
    /// Resolve its five input properties before falling back to authored values.
    fn draft_measurement(
        &mut self,
        node: u16,
        vehicle: Self::Vehicle,
    ) -> Result<super::draft::Measurement, Error>;
}
