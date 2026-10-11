// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    ChassisUpdate, Customization, Error, Physics, PhysicsControls, Quaternion14, SparseVector,
    Update, Vector,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub identity: u64,
    pub revision: u32,
    pub position: [f32; 3],
    pub quaternion: [f32; 4],
    pub velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    pub vehicle_mode: u8,
    pub mode_time: f32,
    pub auxiliary_time: Option<f32>,
    pub flag: bool,
    pub counters: [u8; 2],
    pub scaled_value: f32,
    pub selector: u8,
    pub small_selector: u8,
    pub factor: f32,
    pub control_flag: bool,
    pub axes: [f32; 3],
    pub factor2: f32,
    pub flags: [bool; 4],
    pub wheels: [[f32; 2]; 4],
    pub driving_mode: u32,
}
impl Controls {
    pub fn new(vehicle_mode: u8) -> Result<Self, Error> {
        if vehicle_mode > 7 {
            return Err(Error::Bound);
        }
        Ok(Self {
            vehicle_mode,
            mode_time: 0.,
            auxiliary_time: None,
            flag: true,
            counters: [0; 2],
            scaled_value: 0.,
            selector: 2,
            small_selector: 0,
            factor: 0.,
            control_flag: false,
            axes: [0.; 3],
            factor2: 0.,
            flags: [false; 4],
            wheels: [[0.; 2]; 4],
            driving_mode: 0,
        })
    }
    pub fn encode(&self) -> Result<PhysicsControls, Error> {
        if self.vehicle_mode > 7
            || self.counters.iter().any(|v| *v > 15)
            || self.selector > 15
            || self.small_selector > 3
            || [self.mode_time, self.scaled_value, self.factor, self.factor2]
                .iter()
                .chain(self.auxiliary_time.iter())
                .chain(self.axes.iter())
                .chain(self.wheels.iter().flatten())
                .any(|v| !v.is_finite())
        {
            return Err(Error::Bound);
        }
        Ok(PhysicsControls {
            value3: self.vehicle_mode,
            value6: unsigned(self.mode_time / 3., 6) as u8,
            optional6: self.auxiliary_time.map(|v| unsigned(v / 5., 6) as u8),
            flag: self.flag,
            values: [
                u16::from(self.counters[0]),
                u16::from(self.counters[1]),
                unsigned(self.scaled_value / 12500., 10),
                u16::from(self.selector),
                u16::from(self.small_selector),
                unsigned(self.factor, 4),
                u16::from(self.control_flag),
                signed(self.axes[0], 6),
                signed(self.axes[1], 4),
                signed(self.axes[2], 4),
                unsigned(self.factor2, 1),
            ],
            flags: self.flags,
            wheel_pairs: (self.driving_mode != 5).then(|| {
                self.wheels.map(|v| {
                    [
                        signed(v[0] / 5., 8) as u8,
                        signed(v[1] / std::f32::consts::PI, 8) as u8,
                    ]
                })
            }),
        })
    }
}

fn unsigned(value: f32, width: u8) -> u16 {
    (value.clamp(0., 1.) * ((1u32 << width) - 1) as f32 + 0.5) as u16
}
fn signed(value: f32, width: u8) -> u16 {
    let value = value.clamp(-1., 1.);
    ((value.is_sign_negative() as u16) << (width - 1)) | unsigned(value.abs(), width - 1)
}
impl Motion {
    pub fn encode(&self, origin: [f32; 3], controls: &Controls) -> Result<Physics, Error> {
        Ok(Physics {
            identity_words: [
                (self.identity >> 32) as u32,
                self.identity as u32,
                self.revision,
            ],
            position: Vector::from_position(self.position, origin, 7)?,
            rotation: Quaternion14::from_quaternion(self.quaternion)?,
            velocity: SparseVector::from_vector(self.velocity, 5)?,
            angular_velocity: SparseVector::from_vector(self.angular_velocity, 8)?,
            controls: controls.encode()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    motion: Motion,
    controls: Controls,
    flag: bool,
    custom: Customization,
    pair: (f32, bool),
}
impl State {
    pub fn new(
        position: [f32; 3],
        quaternion: [f32; 4],
        vehicle_mode: u8,
        custom: Customization,
    ) -> Result<Self, Error> {
        let result = Self {
            motion: Motion {
                identity: 0,
                revision: 0,
                position,
                quaternion,
                velocity: [0.; 3],
                angular_velocity: [0.; 3],
            },
            controls: Controls::new(vehicle_mode)?,
            flag: false,
            custom,
            pair: (0., false),
        };
        result.motion.encode([0.; 3], &result.controls)?;
        validate_custom(&result.custom)?;
        Ok(result)
    }
    pub fn motion(&self) -> &Motion {
        &self.motion
    }
    pub fn controls(&self) -> &Controls {
        &self.controls
    }
    pub fn set_motion(&mut self, motion: Motion, controls: Controls) -> Result<(), Error> {
        motion.encode([0.; 3], &controls)?;
        self.motion = motion;
        self.controls = controls;
        Ok(())
    }
    pub fn set_customization(&mut self, custom: Customization) -> Result<(), Error> {
        validate_custom(&custom)?;
        self.custom = custom;
        Ok(())
    }
    pub fn set_flags(&mut self, flag: bool, pair: (f32, bool)) -> Result<(), Error> {
        if !pair.0.is_finite() {
            return Err(Error::Bound);
        }
        self.flag = flag;
        self.pair = pair;
        Ok(())
    }
    pub fn update(&self, origin: [f32; 3], mask: u8) -> Result<Update, Error> {
        if mask & !15 != 0 {
            return Err(Error::Bound);
        }
        Ok(Update::Chassis(Box::new(ChassisUpdate {
            physics: (mask & 1 != 0)
                .then(|| self.motion.encode(origin, &self.controls))
                .transpose()?,
            flag: (mask & 2 != 0).then_some(self.flag),
            custom: (mask & 4 != 0).then(|| self.custom.clone()),
            pair: (mask & 8 != 0).then_some((self.pair.0.to_bits(), self.pair.1)),
        })))
    }
}
fn validate_custom(v: &Customization) -> Result<(), Error> {
    if v.list_a.len() > 255
        || v.list_b.len() > 255
        || v.index.is_some_and(|i| i > 63)
        || v.tuning.iter().any(|v| *v > 15)
    {
        return Err(Error::Bound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::vehicle::{Body, Kind, Profile};

    fn custom() -> Customization {
        Customization {
            value: 19,
            list_a: vec![1, 4],
            list_b: vec![3],
            index: Some(2),
            values: [17, 32],
            flag: true,
            tuning: [7; 32],
        }
    }
    fn state() -> State {
        State::new([9., -2., 3.], [0., 0., 0., 1.], 3, custom()).unwrap()
    }
    fn value(s: &State, mask: u8) -> ChassisUpdate {
        let Update::Chassis(v) = s.update([0.; 3], mask).unwrap() else {
            panic!("Chassis update")
        };
        *v
    }

    #[test]
    fn fresh_defaults_use_static_mode_and_keep_creation_customization_separate() {
        let s = state();
        let v = value(&s, 11);
        assert_eq!(v.flag, Some(false));
        assert_eq!(v.pair, Some((0, false)));
        assert_eq!(v.custom, None);
        let p = v.physics.unwrap();
        assert_eq!(p.identity_words, [0; 3]);
        assert_eq!(p.controls.value3, 3);
        assert!(p.controls.flag);
        assert_eq!(p.controls.values, [0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(p.controls.wheel_pairs, Some([[0; 2]; 4]));
        assert_eq!(value(&s, 4).custom, Some(custom()));
        assert_eq!(Controls::new(8), Err(Error::Bound));
    }

    #[test]
    fn control_quantization_clamps_rounds_and_preserves_sign_first_negative_zero() {
        let mut c = Controls::new(2).unwrap();
        c.mode_time = 1.5;
        c.auxiliary_time = Some(10.);
        c.scaled_value = 6250.;
        c.factor = -1.;
        c.factor2 = 0.5;
        c.axes = [-0., -1., 0.5];
        c.wheels[0] = [-5., std::f32::consts::PI];
        let p = c.encode().unwrap();
        assert_eq!(p.value6, 32);
        assert_eq!(p.optional6, Some(63));
        assert_eq!(p.values[2], 512);
        assert_eq!(p.values[5], 0);
        assert_eq!(&p.values[7..], &[32, 15, 4, 1]);
        assert_eq!(p.wheel_pairs.unwrap()[0], [255, 127]);
        c.driving_mode = 5;
        assert_eq!(c.encode().unwrap().wheel_pairs, None);
        c.driving_mode = 6;
        assert!(c.encode().unwrap().wheel_pairs.is_some());
    }

    #[test]
    fn sparse_velocity_omits_quantized_zero_and_places_shared_mode_at_first_nonzero() {
        for fractional in [5, 8] {
            let step = 1. / (1u32 << fractional) as f32;
            assert_eq!(
                SparseVector::from_vector([step * 0.99, -step * 0.99, -0.], fractional).unwrap(),
                SparseVector::Packed {
                    mode: None,
                    values: [None; 3]
                }
            );
            assert_eq!(
                SparseVector::from_vector([0., -step, step * 1.9], fractional).unwrap(),
                SparseVector::Packed {
                    mode: Some(1),
                    values: [None, Some(-1), Some(1)]
                }
            );
            assert_eq!(
                SparseVector::from_vector([0., 0., 4.], fractional).unwrap(),
                SparseVector::Packed {
                    mode: Some(3),
                    values: [None, None, Some(4 << fractional)]
                }
            );
            let large = [0., -8192., -0.];
            assert_eq!(
                SparseVector::from_vector(large, fractional).unwrap(),
                SparseVector::FloatBits(large.map(f32::to_bits))
            );
        }
        assert!(SparseVector::from_vector([f32::NAN, 0., 0.], 5).is_err());
        assert!(SparseVector::from_vector([0.; 3], 17).is_err());
    }

    #[test]
    fn changing_motion_uses_session_origin_identity_order_and_isolates_vehicles() {
        let mut a = state();
        let b = state();
        let mut motion = a.motion().clone();
        motion.identity = 0x1122334455667788;
        motion.revision = 0x99aabbcc;
        motion.position = [100., 200., -300.];
        motion.velocity = [0., 1., 0.];
        motion.angular_velocity = [0., 0., -0.5];
        a.set_motion(motion, Controls::new(1).unwrap()).unwrap();
        let Update::Chassis(v) = a.update([100., 200., -300.], 1).unwrap() else {
            panic!()
        };
        let p = v.physics.unwrap();
        assert_eq!(p.identity_words, [0x11223344, 0x55667788, 0x99aabbcc]);
        assert_eq!(
            p.position,
            Vector::Packed {
                mode: 1,
                values: [0; 3]
            }
        );
        assert_eq!(
            p.velocity,
            SparseVector::Packed {
                mode: Some(2),
                values: [None, Some(32), None]
            }
        );
        assert_eq!(
            p.angular_velocity,
            SparseVector::Packed {
                mode: Some(1),
                values: [None, None, Some(-128)]
            }
        );
        assert_eq!(b, state());
    }

    #[test]
    fn invalid_late_fields_do_not_partially_replace_current_state() {
        let mut s = state();
        let before = s.clone();
        let mut c = s.controls().clone();
        c.flags = [true; 4];
        c.wheels[3][1] = f32::NAN;
        assert!(s.set_motion(s.motion().clone(), c).is_err());
        let mut m = s.motion().clone();
        m.angular_velocity[2] = f32::INFINITY;
        assert!(s.set_motion(m, Controls::new(0).unwrap()).is_err());
        let mut c = custom();
        c.tuning[31] = 16;
        assert!(s.set_customization(c).is_err());
        assert!(s.set_flags(true, (f32::NAN, true)).is_err());
        assert!(s.update([0.; 3], 16).is_err());
        assert_eq!(s, before);
    }

    #[test]
    fn independent_dirty_fields_repeat_and_round_trip_with_nonzero_current_controls() {
        let mut s = state();
        let mut c = s.controls().clone();
        c.auxiliary_time = Some(2.);
        c.flags = [true, false, true, false];
        c.counters = [3, 9];
        c.wheels[2] = [1.5, -0.75];
        s.set_motion(s.motion().clone(), c).unwrap();
        s.set_flags(true, (-0., true)).unwrap();
        let profile = Profile::new(vec![
            Kind::Root {
                property_owner: false,
            },
            Kind::Chassis,
        ])
        .unwrap();
        for mask in 0..16 {
            let v = value(&s, mask);
            assert_eq!(v.physics.is_some(), mask & 1 != 0);
            assert_eq!(v.flag.is_some(), mask & 2 != 0);
            assert_eq!(v.custom.is_some(), mask & 4 != 0);
            assert_eq!(v.pair.is_some(), mask & 8 != 0);
            let body = Body {
                creation: None,
                updates: vec![None, Some(Update::Chassis(Box::new(v)))],
            };
            let wire = body.encode(&profile).unwrap();
            assert_eq!(
                Body::decode(wire.span(), &profile, false).unwrap().body,
                body
            );
            assert_eq!(wire, body.encode(&profile).unwrap());
        }
    }
}
