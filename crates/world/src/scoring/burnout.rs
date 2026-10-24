// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Server burnout query: native 04736170, outputs 04735d70.
use super::{Error, finite, value::Value};
pub const RESET: u32 = 0x0db4_b330;
pub const ACTIVE: u32 = 0x4082_15ac;
pub const COUNT: u32 = 0x74b6_ee0e;
pub const TIME: u32 = 0x536c_f0dc;
const MPH_TO_MPS: f32 = f32::from_bits(0x3ee4_e26d);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configuration {
    pub max_linear_mph: f32,
    pub max_angular: f32,
    pub min_throttle: f32,
    pub min_time: f32,
    pub min_slip_front: f32,
    pub min_slip_rear: f32,
}
impl Configuration {
    pub fn validate(self) -> Result<(), Error> {
        for v in [
            self.max_linear_mph,
            self.max_angular,
            self.min_throttle,
            self.min_time,
            self.min_slip_front,
            self.min_slip_rear,
        ] {
            finite(v)?;
        }
        finite(self.max_linear_mph * MPH_TO_MPS)?;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls {
    pub throttle: f32,
    /// Front left/right, rear left/right. Both tires of an axle must qualify.
    pub slip: [f32; 4],
}
impl Controls {
    /// Native 041c8b70/041cadd0 copy throttle to control value 8 and slip
    /// ratios to the first member of each wheel pair. Missing wheel data
    /// retains the previous ratios in the owning vehicle's control state.
    pub fn from_wire(
        wire: &crate::replication::vehicle::PhysicsControls,
        previous: Option<Self>,
    ) -> Result<Option<Self>, Error> {
        fn signed(value: u16, bits: u32) -> Result<f32, Error> {
            if u32::from(value) >= 1 << bits {
                return Err(Error::ConversionRange);
            }
            let sign = 1 << (bits - 1);
            let magnitude = f32::from(value & (sign - 1)) / f32::from(sign - 1);
            Ok(if value & sign == 0 {
                magnitude
            } else {
                -magnitude
            })
        }
        let throttle = signed(wire.values[8], 4)?;
        let slip = if let Some(pairs) = wire.wheel_pairs {
            let mut values = [0.; 4];
            for (target, pair) in values.iter_mut().zip(pairs) {
                *target = signed(u16::from(pair[0]), 8)? * 5.;
            }
            values
        } else if let Some(previous) = previous {
            previous.slip
        } else {
            return Ok(None);
        };
        Ok(Some(Self { throttle, slip }))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    active: bool,
    count: i32,
    time: f32,
}
impl State {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn outputs(self) -> [(u32, Value); 3] {
        [
            (ACTIVE, Value::Boolean(self.active)),
            (COUNT, Value::Integer(self.count)),
            (TIME, Value::Float(self.time)),
        ]
    }
    pub fn sample(
        &mut self,
        config: Configuration,
        velocity: [f32; 3],
        angular_velocity: [f32; 3],
        controls: Controls,
        elapsed: f32,
    ) -> Result<(), Error> {
        config.validate()?;
        for v in velocity
            .into_iter()
            .chain(angular_velocity)
            .chain(controls.slip)
            .chain([controls.throttle, elapsed])
        {
            finite(v)?;
        }
        if elapsed < 0. {
            return Err(Error::ConversionRange);
        }
        let magnitude = |v: [f32; 3]| finite(((v[1] * v[1] + v[0] * v[0]) + v[2] * v[2]).sqrt());
        let linear = magnitude(velocity)?;
        let angular = magnitude(angular_velocity)?;
        let active = linear < config.max_linear_mph * MPH_TO_MPS
            && angular < config.max_angular
            && (controls.slip[0].min(controls.slip[1]) > config.min_slip_front
                || controls.slip[2].min(controls.slip[3]) > config.min_slip_rear)
            && controls.throttle > config.min_throttle;
        if active {
            self.time = finite(self.time + elapsed)?;
        } else {
            if self.active && self.time > config.min_time {
                self.count = self.count.wrapping_add(1);
            }
            self.time = 0.;
        }
        self.active = active;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Configuration {
        Configuration {
            max_linear_mph: 20.,
            max_angular: 1.,
            min_throttle: 0.7,
            min_time: 0.5,
            min_slip_front: 1.,
            min_slip_rear: 1.,
        }
    }
    fn controls() -> Controls {
        Controls {
            throttle: 1.,
            slip: [0., 0., 2., 2.],
        }
    }
    #[test]
    fn counts_only_when_a_sufficiently_long_burnout_ends_and_uses_strict_thresholds() {
        let mut s = State::default();
        s.sample(config(), [0.; 3], [0.; 3], controls(), 0.5)
            .unwrap();
        assert_eq!((s.active, s.count, s.time), (true, 0, 0.5));
        let mut off = controls();
        off.throttle = 0.7;
        s.sample(config(), [0.; 3], [0.; 3], off, 0.1).unwrap();
        assert_eq!((s.active, s.count, s.time), (false, 0, 0.));
        s.sample(config(), [0.; 3], [0.; 3], controls(), 0.6)
            .unwrap();
        s.sample(config(), [0.; 3], [0., 1., 0.], controls(), 0.1)
            .unwrap();
        assert_eq!((s.active, s.count, s.time), (false, 1, 0.));
        s.sample(
            config(),
            [20. * MPH_TO_MPS, 0., 0.],
            [0.; 3],
            controls(),
            1.,
        )
        .unwrap();
        assert!(!s.active);
        let mut mixed = controls();
        mixed.slip = [2., 0., 0., 2.];
        s.sample(config(), [0.; 3], [0.; 3], mixed, 1.).unwrap();
        assert!(!s.active);
        mixed.slip = [2., 2., 0., 0.];
        s.sample(config(), [0.; 3], [0.; 3], mixed, 1.).unwrap();
        assert!(s.active);
        let before = s;
        assert!(
            s.sample(config(), [f32::MAX; 3], [0.; 3], controls(), 1.)
                .is_err()
        );
        assert_eq!(s, before);
        s.reset();
        assert_eq!(s, State::default());
    }
    #[test]
    fn chassis_wire_decodes_throttle_and_signed_slips_and_retains_omitted_wheels() {
        let mut c = crate::replication::vehicle::chassis::Controls::new(3).unwrap();
        c.axes[1] = 1.;
        c.wheels = [[5., 0.], [-5., 0.], [0., 0.], [0., 0.]];
        let mut wire = c.encode().unwrap();
        let decoded = Controls::from_wire(&wire, None).unwrap().unwrap();
        assert_eq!(
            decoded,
            Controls {
                throttle: 1.,
                slip: [5., -5., 0., 0.]
            }
        );
        wire.wheel_pairs = None;
        assert_eq!(Controls::from_wire(&wire, None).unwrap(), None);
        assert_eq!(
            Controls::from_wire(&wire, Some(decoded)).unwrap(),
            Some(decoded)
        );
        wire.values[8] = 16;
        assert!(Controls::from_wire(&wire, Some(decoded)).is_err());
    }
}
