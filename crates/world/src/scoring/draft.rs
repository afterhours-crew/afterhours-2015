// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Drafting from owned participant vehicle snapshots, without an award policy.
use super::{Error, finite, properties::Kind, value::Value};

pub const STRENGTH: u32 = 0x7b88_b10c;
pub const DELTA_SPEED: u32 = 0xad59_4576;
pub const ACTIVE: u32 = 0xd520_d6da;
pub const MAX_VEHICLES: usize = 4096;
pub const INPUTS: [u32; 5] = [
    0x0dd8_920c,
    0x5877_4f4e,
    0x6bee_0f7a,
    0x5c66_d87f,
    0x5855_1a21,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configuration {
    pub min_other_speed: f32,
    pub max_delta_speed: f32,
    pub cone_scale: f32,
    pub min_draft_time: f32,
    pub max_draft_time: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vehicle {
    /// Stable physics identity within the current session, used to exclude self.
    pub identity: u32,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
}
impl Vehicle {
    fn validate(self) -> Result<(), Error> {
        Kind::Vec3.validate(&Value::Vec3(self.position))?;
        Kind::Vec3.validate(&Value::Vec3(self.velocity))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measurement {
    strength: f32,
    delta_speed: f32,
}
impl Measurement {
    pub fn strength(self) -> f32 {
        self.strength
    }
    pub fn delta_speed(self) -> f32 {
        self.delta_speed
    }
    pub fn active(self) -> bool {
        self.strength != 0.
    }
}
fn dot(a: [f32; 3], b: [f32; 3]) -> Result<f32, Error> {
    let x = finite(a[0] * b[0])?;
    let y = finite(a[1] * b[1])?;
    let z = finite(a[2] * b[2])?;
    finite(finite(y + x)? + z)
}
fn difference(a: [f32; 3], b: [f32; 3]) -> Result<[f32; 3], Error> {
    Ok([
        finite(a[0] - b[0])?,
        finite(a[1] - b[1])?,
        finite(a[2] - b[2])?,
    ])
}
impl Configuration {
    pub fn validate(self) -> Result<(), Error> {
        for v in [
            self.min_other_speed,
            self.max_delta_speed,
            self.cone_scale,
            self.min_draft_time,
            self.max_draft_time,
        ] {
            if finite(v)? < 0. {
                return Err(Error::InvalidGraph);
            }
        }
        if self.max_delta_speed == 0.
            || self.cone_scale == 0.
            || self.max_draft_time <= self.min_draft_time
        {
            return Err(Error::InvalidGraph);
        }
        Ok(())
    }
    /// `others` is the resolved participant-set snapshot; omit missing vehicles.
    /// Missing own vehicle is handled by the dispatcher, not a synthetic zero.
    /// Strength and positive delta speed take independent maxima, so they may
    /// originate from different vehicles. Empty sets produce zero/false outputs.
    pub fn evaluate(self, own: Vehicle, others: &[Vehicle]) -> Result<Measurement, Error> {
        self.validate()?;
        own.validate()?;
        if others.len() > MAX_VEHICLES {
            return Err(Error::Bound);
        }
        let mut result = Measurement::default();
        for &other in others {
            if other.identity == own.identity {
                continue;
            }
            other.validate()?;
            if let Some(pair) = self.pair(own, other)? {
                if pair.strength > result.strength {
                    result.strength = pair.strength;
                }
                if pair.delta_speed > result.delta_speed {
                    result.delta_speed = pair.delta_speed;
                }
            }
        }
        Ok(result)
    }
    fn pair(self, own: Vehicle, other: Vehicle) -> Result<Option<Measurement>, Error> {
        let delta = difference(other.position, own.position)?;
        // The broad-phase boundary includes exactly 50 metres, in 3D.
        if dot(delta, delta)? > 2500.
            || dot(own.velocity, delta)? <= 0.
            || dot(other.velocity, delta)? <= 0.
        {
            return Ok(None);
        }
        let reciprocal = finite(1. / dot(own.velocity, own.velocity)?.sqrt())?;
        let direction = [
            finite(own.velocity[0] * reciprocal)?,
            finite(own.velocity[1] * reciprocal)?,
            finite(own.velocity[2] * reciprocal)?,
        ];
        let distance = dot(delta, direction)?;
        let other_speed = dot(other.velocity, direction)?;
        let own_speed = dot(own.velocity, direction)?;
        let delta_speed = finite(own_speed - other_speed)?;
        if other_speed <= self.min_other_speed || delta_speed.abs() >= self.max_delta_speed {
            return Ok(None);
        }
        let time = finite(distance / other_speed)?;
        if time >= self.max_draft_time {
            return Ok(None);
        }
        let weight = if time < self.min_draft_time {
            1.
        } else {
            finite(
                1. - finite(
                    finite(time - self.min_draft_time)?
                        / finite(self.max_draft_time - self.min_draft_time)?,
                )?,
            )?
        };
        // Project the other car backwards along its own velocity. Vertical
        // displacement is discarded only here, after the 3D broad-phase checks.
        let projected = [
            finite(other.position[0] - finite(other.velocity[0] * time)?)?,
            own.position[1],
            finite(other.position[2] - finite(other.velocity[2] * time)?)?,
        ];
        let lateral = difference(projected, own.position)?;
        let width = finite(finite(finite(1. - weight)? * self.cone_scale)? + weight)?;
        let across = finite(1. - finite(dot(lateral, lateral)?.sqrt() / width)?)?;
        let strength = finite(across.clamp(0., 1.) * weight)?;
        Ok((strength > 0.).then_some(Measurement {
            strength,
            delta_speed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Configuration {
        Configuration {
            min_other_speed: 5.,
            max_delta_speed: 25.,
            cone_scale: 3.,
            min_draft_time: 0.1,
            max_draft_time: 1.,
        }
    }
    fn vehicle(identity: u32, x: f32, z: f32, speed: f32) -> Vehicle {
        Vehicle {
            identity,
            position: [x, 0., z],
            velocity: [0., 0., speed],
        }
    }
    #[test]
    fn perpendicular_crossing_vehicle_is_not_a_draft_or_a_numeric_fault() {
        let own = vehicle(1, 0., 0., 20.);
        let crossing = Vehicle {
            identity: 2,
            position: [3., 0., 5.],
            velocity: [10., 0., 0.],
        };
        // Both broad-phase dot products are positive, but forward speed is
        // zero. Reject it before division instead of disabling scoring.
        assert_eq!(
            config().evaluate(own, &[crossing]),
            Ok(Measurement::default())
        );
        let valid = vehicle(3, 0., 2., 20.);
        assert_eq!(
            config().evaluate(own, &[crossing, valid]),
            config().evaluate(own, &[valid])
        );
    }

    #[test]
    fn straight_draft_has_time_falloff_and_independent_maxima() {
        let own = vehicle(1, 0., 0., 20.);
        let near = vehicle(2, 0., 2., 20.);
        let slower = vehicle(3, 0., 3., 10.);
        let first = config().evaluate(own, &[near]).unwrap();
        assert_eq!(first.strength(), 1.);
        assert_eq!(first.delta_speed(), 0.);
        assert!(first.active());
        let both = config().evaluate(own, &[near, slower]).unwrap();
        assert_eq!(both.strength(), 1.);
        assert_eq!(both.delta_speed(), 10.);
        assert_eq!(both, config().evaluate(own, &[slower, near, near]).unwrap());
        let weaker = config().evaluate(own, &[slower]).unwrap();
        assert!((weaker.strength() - 7. / 9.).abs() < 1e-6);
        let faster = vehicle(4, 0., 3., 30.);
        assert_eq!(config().evaluate(own, &[faster]).unwrap().delta_speed(), 0.);
    }
    #[test]
    fn strict_speed_time_cone_and_inclusive_distance_boundaries() {
        let own = vehicle(1, 0., 0., 20.);
        for other in [
            vehicle(2, 0., -2., 20.),
            vehicle(2, 0., 2., -20.),
            vehicle(2, 0., 2., 5.),
            vehicle(2, 0., 2., 45.),
            vehicle(2, 0., 20., 20.),
            vehicle(2, 1., 1., 20.),
        ] {
            assert_eq!(
                config().evaluate(own, &[other]).unwrap(),
                Measurement::default(),
                "{other:?}"
            );
        }
        let mut c = config();
        c.max_draft_time = 10.;
        assert!(
            c.evaluate(own, &[vehicle(2, 0., 50., 20.)])
                .unwrap()
                .active()
        );
        assert!(
            !c.evaluate(
                own,
                &[vehicle(2, 0., f32::from_bits(50_f32.to_bits() + 1), 20.)]
            )
            .unwrap()
            .active()
        );
        let mut high = vehicle(2, 0., 1., 20.);
        high.position[1] = 10.;
        assert_eq!(config().evaluate(own, &[high]).unwrap().strength(), 1.);
    }
    #[test]
    fn own_identity_missing_set_and_stationary_cars_do_not_award() {
        let own = vehicle(1, 0., 0., 20.);
        assert_eq!(config().evaluate(own, &[]).unwrap(), Measurement::default());
        assert_eq!(
            config().evaluate(own, &[vehicle(1, 0., 2., 20.)]).unwrap(),
            Measurement::default()
        );
        assert_eq!(
            config()
                .evaluate(vehicle(1, 0., 0., 0.), &[vehicle(2, 0., 2., 20.)])
                .unwrap(),
            Measurement::default()
        );
    }
    #[test]
    fn malformed_config_measurements_and_excessive_sets_fail() {
        let own = vehicle(1, 0., 0., 20.);
        for c in [
            Configuration {
                max_draft_time: 0.1,
                ..config()
            },
            Configuration {
                cone_scale: 0.,
                ..config()
            },
            Configuration {
                max_delta_speed: f32::NAN,
                ..config()
            },
        ] {
            assert!(c.evaluate(own, &[]).is_err());
        }
        assert_eq!(
            config().evaluate(own, &vec![own; MAX_VEHICLES + 1]),
            Err(Error::Bound)
        );
        let mut bad = vehicle(2, f32::INFINITY, 2., 20.);
        assert_eq!(config().evaluate(own, &[bad]), Err(Error::NonFinite));
        bad.position = [f32::MAX; 3];
        assert_eq!(config().evaluate(own, &[bad]), Err(Error::NonFinite));
    }
}
