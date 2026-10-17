// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned doughnut detection for `RaceVehicleDonutQuery` (E916). The authored
//! thresholds are installed (`MinLinearVelocity` 5, `MaxLinearVelocity` 35,
//! `DonutScaleThreshold` 0.7, `TimeInDonutThreshold` 0, `AllowDonutsInReverse`
//! false); the native measure behind "donut scale" is not established. The
//! speed window is read in mph like the road query's limits (E918: the
//! owner's doughnuts ran at 2–4 m/s, 5–9 mph, which the window must admit;
//! as m/s it would only admit slides above 11 mph). This
//! model (hypothesis) takes it as the sine of the slip angle, the lateral
//! share of the planar velocity: a car sliding sideways while rotating. While
//! the speed is inside the window, the car moves forward (unless reverse is
//! allowed) and the scale reaches the threshold for at least the time
//! threshold, yaw rotation accumulates; every full turn fires
//! `OnDonutCompleted`. Leaving the window resets the accumulation.
use super::{Error, finite};

/// `OnDonutCompleted` (hash of the name).
pub const ON_DONUT_COMPLETED: u32 = 0x79b8_a6ed;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configuration {
    pub min_linear_velocity: f32,
    pub max_linear_velocity: f32,
    pub donut_scale_threshold: f32,
    pub time_in_donut_threshold: f32,
    pub allow_donuts_in_reverse: bool,
}
impl Configuration {
    pub fn validate(self) -> Result<(), Error> {
        for value in [
            self.min_linear_velocity,
            self.max_linear_velocity,
            self.donut_scale_threshold,
            self.time_in_donut_threshold,
        ] {
            if finite(value)? < 0.0 {
                return Err(Error::InvalidGraph);
            }
        }
        if self.max_linear_velocity <= self.min_linear_velocity {
            return Err(Error::InvalidGraph);
        }
        Ok(())
    }
}

/// One owned measurement in the car frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Right, up, forward basis and the world velocity and angular velocity.
    pub basis: [[f32; 3]; 3],
    pub velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
}

fn dot(a: [f32; 3], b: [f32; 3]) -> Result<f32, Error> {
    finite(a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
}

/// Ineligible time tolerated before the accumulation resets: a doughnut at
/// walking pace dips below the 5 mph floor between samples (E918, E919).
pub const GRACE_SECONDS: f32 = 0.5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Node {
    /// Accumulated yaw while eligible, radians (unsigned).
    turned: f32,
    /// Time spent eligible, seconds.
    eligible: f32,
    /// Time spent ineligible since the last eligible sample, seconds.
    lapsed: f32,
}
impl Node {
    /// Advance by `elapsed` seconds; true when a full turn completed.
    pub fn step(
        &mut self,
        configuration: Configuration,
        sample: Sample,
        elapsed: f32,
    ) -> Result<bool, Error> {
        configuration.validate()?;
        if finite(elapsed)? < 0.0 {
            return Err(Error::Bound);
        }
        let [right, up, forward] = sample.basis;
        let lateral = dot(sample.velocity, right)?;
        let along = dot(sample.velocity, forward)?;
        let planar = finite((lateral * lateral + along * along).sqrt())?;
        let speed = finite(dot(sample.velocity, sample.velocity)?.sqrt())?;
        let speed_mph = finite(speed * super::road::METRES_PER_SECOND_TO_MPH)?;
        let scale = if planar > f32::EPSILON {
            finite(lateral.abs() / planar)?
        } else {
            0.0
        };
        let direction_ok = configuration.allow_donuts_in_reverse || along > 0.0;
        let eligible = speed_mph >= configuration.min_linear_velocity
            && speed_mph <= configuration.max_linear_velocity
            && direction_ok
            && scale >= configuration.donut_scale_threshold;
        if !eligible {
            self.lapsed = finite(self.lapsed + elapsed)?;
            if self.lapsed > GRACE_SECONDS {
                *self = Self::default();
            }
            return Ok(false);
        }
        self.lapsed = 0.0;
        self.eligible = finite(self.eligible + elapsed)?;
        if self.eligible < configuration.time_in_donut_threshold {
            return Ok(false);
        }
        let yaw_rate = dot(sample.angular_velocity, up)?.abs();
        self.turned = finite(self.turned + yaw_rate * elapsed)?;
        if self.turned >= std::f32::consts::TAU {
            self.turned -= std::f32::consts::TAU;
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CONFIGURATION: Configuration = Configuration {
        min_linear_velocity: 5.0,
        max_linear_velocity: 35.0,
        donut_scale_threshold: 0.7,
        time_in_donut_threshold: 0.0,
        allow_donuts_in_reverse: false,
    };
    const BASIS: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    fn sliding(lateral: f32, along: f32, yaw: f32) -> Sample {
        Sample {
            basis: BASIS,
            velocity: [lateral, 0.0, along],
            angular_velocity: [0.0, yaw, 0.0],
        }
    }
    #[test]
    fn a_full_sideways_turn_inside_the_speed_window_completes_once() {
        let mut node = Node::default();
        // 8 m/s sideways, 3 m/s forward: scale 0.94, speed 8.5 m/s = 19 mph; yaw 2 rad/s.
        let mut completed = 0;
        for _ in 0..120 {
            if node
                .step(CONFIGURATION, sliding(8.0, 3.0, 2.0), 1.0 / 30.0)
                .unwrap()
            {
                completed += 1;
            }
        }
        // 4 s at 2 rad/s = 8 rad: one full turn, 1.72 rad carried.
        assert_eq!(completed, 1);
        assert!((node.turned - (8.0 - std::f32::consts::TAU)).abs() < 1e-3);
    }
    #[test]
    fn leaving_the_window_resets_and_reverse_is_rejected_unless_allowed() {
        let mut node = Node::default();
        assert!(
            !node
                .step(CONFIGURATION, sliding(8.0, 3.0, 2.0), 1.0)
                .unwrap()
        );
        assert!(node.turned > 0.0);
        // Too fast.
        assert!(
            !node
                .step(CONFIGURATION, sliding(30.0, 20.0, 2.0), 1.0)
                .unwrap()
        );
        assert_eq!(node, Node::default());
        // Straight driving: scale 0.
        assert!(
            !node
                .step(CONFIGURATION, sliding(0.0, 10.0, 2.0), 1.0)
                .unwrap()
        );
        assert_eq!(node, Node::default());
        // Reverse slide is ignored by default and accepted when allowed.
        assert!(
            !node
                .step(CONFIGURATION, sliding(8.0, -3.0, 2.0), 1.0)
                .unwrap()
        );
        assert_eq!(node, Node::default());
        let reverse = Configuration {
            allow_donuts_in_reverse: true,
            ..CONFIGURATION
        };
        assert!(!node.step(reverse, sliding(8.0, -3.0, 2.0), 1.0).unwrap());
        assert!(node.turned > 0.0);
    }
    #[test]
    fn time_threshold_delays_accumulation_and_inputs_are_validated() {
        let timed = Configuration {
            time_in_donut_threshold: 0.5,
            ..CONFIGURATION
        };
        let mut node = Node::default();
        assert!(!node.step(timed, sliding(8.0, 3.0, 2.0), 0.4).unwrap());
        assert_eq!(node.turned, 0.0);
        assert!(!node.step(timed, sliding(8.0, 3.0, 2.0), 0.2).unwrap());
        assert!(node.turned > 0.0);
        assert!(
            node.step(CONFIGURATION, sliding(8.0, 3.0, 2.0), -1.0)
                .is_err()
        );
        assert!(
            node.step(CONFIGURATION, sliding(f32::NAN, 3.0, 2.0), 0.1)
                .is_err()
        );
        assert!(
            Configuration {
                max_linear_velocity: 5.0,
                ..CONFIGURATION
            }
            .validate()
            .is_err()
        );
    }
}
