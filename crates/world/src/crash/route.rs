// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored event recovery, sampled before a crash and frozen for its lifetime.
use super::Destination;
use crate::replication::Error;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub time_before_corner: f32,
    pub sharp_corner_degrees: f32,
    pub distance_before_checkpoint: f32,
    pub time_before_checkpoint: f32,
    pub min_speed: f32,
    pub max_speed_percent: f32,
    pub max_delta_time: f32,
}
impl Config {
    pub fn validate(self) -> Result<Self, Error> {
        if ![
            self.time_before_corner,
            self.time_before_checkpoint,
            self.max_delta_time,
        ]
        .into_iter()
        .all(|v| v.is_finite() && (0.001..=60.).contains(&v))
            || !self.sharp_corner_degrees.is_finite()
            || !(0.0..=180.).contains(&self.sharp_corner_degrees)
            || !self.distance_before_checkpoint.is_finite()
            || !(0.0..=10_000.).contains(&self.distance_before_checkpoint)
            || !self.min_speed.is_finite()
            || !(0.0..=500.).contains(&self.min_speed)
            || !self.max_speed_percent.is_finite()
            || !(0.0..=1.).contains(&self.max_speed_percent)
        {
            return Err(Error::Bound);
        }
        Ok(self)
    }

    /// The minimum's conversion applies to the instantaneous limit; the native
    /// elapsed-time blend uses the authored value directly. Preserve that order.
    pub fn speed(
        self,
        current: f32,
        elapsed: f32,
        distance: f32,
        corner: Option<f32>,
    ) -> Result<Option<f32>, Error> {
        self.validate()?;
        if [current, elapsed, distance]
            .into_iter()
            .chain(corner)
            .any(|v| !v.is_finite() || v < 0.)
            || current > 1_000.
        {
            return Err(Error::Bound);
        }
        let limit = (current * self.max_speed_percent).max(self.min_speed * 0.447_04);
        let t = elapsed.min(self.max_delta_time) / self.max_delta_time;
        let floor = (1. - t) * limit + t * self.min_speed;
        let mut allowed = distance / self.time_before_checkpoint;
        if let Some(corner) = corner {
            allowed = allowed.min(corner / self.time_before_corner);
        }
        let speed = allowed.max(floor).min(floor.max(limit));
        Ok(
            (distance > self.distance_before_checkpoint && speed - allowed < 0.000_001)
                .then_some(speed),
        )
    }
}

/// A game-mode lifetime and its current route segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    pub game_mode: u16,
    pub next: u16,
    pub lap: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recovery {
    pub destination: Destination,
    pub speed: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub destination: Destination,
    pub distance: f32,
    pub corner: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub key: Key,
    pub config: Config,
    pub geometry: Option<Geometry>,
}

#[derive(Clone, Debug, Default)]
pub struct Cache {
    key: Option<Key>,
    sample: Option<Recovery>,
    closest: Option<f32>,
    updated: f64,
    observed: Option<f64>,
}
impl Cache {
    pub fn observe(
        &mut self,
        key: Key,
        config: Config,
        geometry: Option<Geometry>,
        speed: f32,
        seconds: f64,
    ) -> Result<(), Error> {
        if !seconds.is_finite() || seconds < 0. {
            return Err(Error::Shape);
        }
        config.validate()?;
        if !speed.is_finite() || !(0.0..=1_000.).contains(&speed) {
            return Err(Error::Bound);
        }
        if let Some(g) = geometry {
            crate::replication::vehicle::orientation::quaternion_from_basis(g.destination.basis)?;
            if g.destination.position.iter().any(|v| !v.is_finite()) {
                return Err(Error::Shape);
            }
            config.speed(speed, 0., g.distance, g.corner)?;
        }
        if self.observed.is_some_and(|last| seconds <= last) {
            return Ok(());
        }
        if self
            .key
            .is_none_or(|previous| previous.game_mode != key.game_mode)
        {
            self.sample = None;
            self.updated = seconds;
        }
        if self.key != Some(key) {
            self.key = Some(key);
            // The native waypoint callback clears only the closest-distance
            // flag. Keep the last safe point until this segment yields one.
            self.closest = None;
        }
        self.observed = Some(seconds);
        let Some(g) = geometry else {
            return Ok(());
        };
        let Some(speed) =
            config.speed(speed, (seconds - self.updated) as f32, g.distance, g.corner)?
        else {
            return Ok(());
        };
        if self
            .closest
            .is_some_and(|remaining| g.distance >= remaining)
        {
            return Ok(());
        }
        self.sample = Some(Recovery {
            destination: g.destination,
            speed,
        });
        self.closest = Some(g.distance);
        self.updated = seconds;
        Ok(())
    }
    pub fn recovery(&self, key: Key) -> Option<Recovery> {
        self.key
            .filter(|previous| previous.game_mode == key.game_mode)
            .and(self.sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            time_before_corner: 2.,
            sharp_corner_degrees: 60.,
            distance_before_checkpoint: 10.,
            time_before_checkpoint: 0.5,
            min_speed: 20.,
            max_speed_percent: 0.5,
            max_delta_time: 2.,
        }
    }
    fn key() -> Key {
        Key {
            game_mode: 40,
            next: 0,
            lap: 1,
        }
    }
    fn geometry(distance: f32) -> Geometry {
        Geometry {
            destination: Destination {
                position: [100. - distance, 0., 0.],
                basis: [[0., 0., -1.], [0., 1., 0.], [1., 0., 0.]],
            },
            distance,
            corner: None,
        }
    }
    #[test]
    fn authored_fraction_minimum_and_elapsed_blend() {
        let c = config();
        assert_eq!(c.speed(100., 0., 100., None).unwrap(), Some(50.));
        assert_eq!(c.speed(0., 0., 100., None).unwrap(), Some(20. * 0.447_04));
        assert_eq!(c.speed(0., 2., 100., None).unwrap(), Some(20.));
        assert_eq!(c.speed(100., 0., 20., None).unwrap(), None);
        assert_eq!(c.speed(100., 1., 20., None).unwrap(), Some(40.));
        assert_eq!(c.speed(100., 200., 12., None).unwrap(), Some(24.));
    }
    #[test]
    fn checkpoint_and_corner_limits_reject_unsuitable_points() {
        let c = config();
        assert_eq!(c.speed(20., 0., 10., None).unwrap(), None);
        assert_eq!(c.speed(100., 0., 100., Some(99.)).unwrap(), None);
        assert_eq!(c.speed(100., 0., 100., Some(100.)).unwrap(), Some(50.));
        assert_eq!(c.speed(100., 1., 100., Some(80.)).unwrap(), Some(40.));
    }
    #[test]
    fn invalid_configuration_and_samples_are_bounded() {
        for value in [f32::NAN, f32::INFINITY, -1.] {
            assert!(config().speed(value, 0., 100., None).is_err());
            assert!(config().speed(100., value, 100., None).is_err());
            assert!(config().speed(100., 0., value, None).is_err());
            assert!(config().speed(100., 0., 100., Some(value)).is_err());
        }
        assert!(config().speed(1001., 0., 100., None).is_err());
        for change in [
            (|c: &mut Config| c.max_delta_time = 0.) as fn(&mut Config),
            |c| c.time_before_checkpoint = 0.,
            |c| c.time_before_corner = 0.,
            |c| c.max_speed_percent = 1.01,
            |c| c.sharp_corner_degrees = 181.,
            |c| c.min_speed = f32::NAN,
        ] {
            let mut c = config();
            change(&mut c);
            assert!(c.validate().is_err());
        }
    }
    #[test]
    fn keeps_closest_valid_point_and_ignores_duplicate_stale_or_invalid_samples() {
        let mut cache = Cache::default();
        cache
            .observe(key(), config(), Some(geometry(100.)), 100., 1.)
            .unwrap();
        cache
            .observe(key(), config(), Some(geometry(90.)), 100., 2.)
            .unwrap();
        let expected = cache.recovery(key()).unwrap();
        for (distance, seconds) in [(80., 2.), (70., 1.5), (120., 3.), (5., 4.)] {
            cache
                .observe(key(), config(), Some(geometry(distance)), 100., seconds)
                .unwrap();
            assert_eq!(cache.recovery(key()), Some(expected));
        }
        assert!(
            cache
                .observe(key(), config(), Some(geometry(f32::NAN)), 100., 5.)
                .is_err()
        );
        cache
            .observe(key(), config(), Some(geometry(80.)), 100., 5.)
            .unwrap();
        assert_eq!(cache.recovery(key()).unwrap().destination.position[0], 20.);
    }
    #[test]
    fn waypoint_change_keeps_safe_fallback_until_new_segment_sample_but_new_race_does_not() {
        let mut cache = Cache::default();
        cache
            .observe(key(), config(), Some(geometry(90.)), 100., 1.)
            .unwrap();
        let previous = cache.recovery(key());
        let next = Key { next: 1, ..key() };
        cache.observe(next, config(), None, 100., 2.).unwrap();
        assert_eq!(cache.recovery(next), previous);
        cache
            .observe(next, config(), Some(geometry(200.)), 100., 3.)
            .unwrap();
        assert_ne!(cache.recovery(next), previous);
        let lap = Key {
            next: 0,
            lap: 2,
            ..key()
        };
        cache
            .observe(lap, config(), Some(geometry(300.)), 100., 4.)
            .unwrap();
        assert_eq!(cache.recovery(lap).unwrap().destination.position[0], -200.);
        let restarted = Key {
            game_mode: 41,
            ..key()
        };
        assert!(cache.recovery(restarted).is_none());
        cache.observe(restarted, config(), None, 100., 5.).unwrap();
        assert!(cache.recovery(restarted).is_none());
    }
}
