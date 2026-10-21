// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded recovery history for an owned car. A crash freezes the destination;
//! rolling wreck positions must never replace the earlier driving trail.
use crate::replication::{
    Error,
    vehicle::{chassis::Motion, orientation::basis_from_quaternion},
};
use std::collections::VecDeque;

const CAPACITY: usize = 128;
// Conservative server sampling/eligibility policy. A complete native trail
// implementation would additionally need its road and collision predicates.
const INTERVAL: f64 = 0.25;
const MARGIN: f64 = 2.0;
const HEADING_DOT: f32 = 0.939_692_6; // cos(20 degrees)

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Destination {
    pub position: [f32; 3],
    pub basis: [[f32; 3]; 3],
}

#[derive(Clone, Copy, Debug)]
struct Point {
    seconds: f64,
    destination: Destination,
    valid: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Trail {
    points: VecDeque<Point>,
    last_seconds: Option<f64>,
}

impl Trail {
    pub fn observe(&mut self, motion: &Motion, grounded: bool, crashed: bool) -> Result<(), Error> {
        let seconds = f64::from_bits(motion.identity);
        if !seconds.is_finite() || seconds < 0.0 || motion.position.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        let basis = basis_from_quaternion(motion.quaternion)?;
        if self.last_seconds.is_some_and(|last| seconds <= last) {
            return Ok(());
        }
        self.last_seconds = Some(seconds);
        let valid = grounded && !crashed && basis[1][1] >= HEADING_DOT;
        // Keep invalid observations too: a short airborne/crash pulse must
        // move the two-second exclusion boundary even between sampled points.
        if let Some(last) = self.points.back_mut()
            && seconds - last.seconds < INTERVAL
        {
            if !valid {
                last.valid = false;
                last.seconds = seconds;
            }
            return Ok(());
        }
        if self.points.len() == CAPACITY {
            self.points.pop_front();
        }
        self.points.push_back(Point {
            seconds,
            destination: Destination {
                position: motion.position,
                basis,
            },
            valid,
        });
        Ok(())
    }

    pub fn destination(&self, seconds: f64) -> Option<Destination> {
        if !seconds.is_finite() {
            return None;
        }
        let mut before = seconds - MARGIN;
        let mut stable = 0;
        let mut previous: Option<[f32; 3]> = None;
        for point in self.points.iter().rev() {
            if !point.valid {
                before = before.min(point.seconds - MARGIN);
                stable = 0;
                previous = None;
                continue;
            }
            if point.seconds > before {
                continue;
            }
            let forward = point.destination.basis[2];
            if previous.is_some_and(|p| {
                p.into_iter().zip(forward).map(|(a, b)| a * b).sum::<f32>() < HEADING_DOT
            }) {
                stable = 0;
            }
            stable += 1;
            previous = Some(forward);
            if stable == 4 {
                return Some(point.destination);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn motion(time: f64) -> Motion {
        Motion {
            identity: time.to_bits(),
            revision: (time * 100.0) as u32,
            position: [time as f32, 10.0, 0.0],
            quaternion: [0.0, 0.0, 0.0, 1.0],
            velocity: [1.0, 0.0, 0.0],
            angular_velocity: [0.0; 3],
        }
    }
    #[test]
    fn recovery_precedes_the_incident_and_requires_a_stable_grounded_run() {
        let mut trail = Trail::default();
        for i in 0..40 {
            trail
                .observe(&motion(f64::from(i) / 4.0), true, false)
                .unwrap();
        }
        assert_eq!(trail.destination(10.0).unwrap().position[0], 7.25);
        trail.observe(&motion(10.0), false, true).unwrap();
        for i in 41..60 {
            trail
                .observe(&motion(f64::from(i) / 4.0), false, true)
                .unwrap();
        }
        assert_eq!(trail.destination(15.0).unwrap().position[0], 7.25);
    }
    #[test]
    fn inverted_and_airborne_samples_do_not_become_destinations() {
        let mut trail = Trail::default();
        for i in 0..40 {
            let mut sample = motion(f64::from(i) / 4.0);
            sample.quaternion = [1.0, 0.0, 0.0, 0.0];
            trail.observe(&sample, true, false).unwrap();
        }
        assert!(trail.destination(15.0).is_none());
    }
    #[test]
    fn invalid_pulses_and_stale_clocks_cannot_replace_good_history() {
        let mut trail = Trail::default();
        for i in 0..40 {
            trail
                .observe(&motion(f64::from(i) / 4.0), true, false)
                .unwrap();
        }
        trail.observe(&motion(9.8), false, false).unwrap();
        let expected = trail.destination(15.0).unwrap();
        assert!(expected.position[0] <= 7.05);
        trail.observe(&motion(5.0), true, false).unwrap();
        assert_eq!(trail.destination(15.0), Some(expected));
        assert!(trail.observe(&motion(f64::NAN), true, false).is_err());
        assert!(trail.destination(f64::NAN).is_none());
    }
    #[test]
    fn history_is_bounded_and_sharp_turns_break_the_stable_run() {
        let mut trail = Trail::default();
        for i in 0..1000 {
            let mut sample = motion(f64::from(i));
            if i % 2 == 0 {
                sample.quaternion = [0.0, 1.0, 0.0, 0.0];
            }
            trail.observe(&sample, true, false).unwrap();
        }
        assert_eq!(trail.points.len(), CAPACITY);
        assert!(trail.destination(1003.0).is_none());
    }
}
