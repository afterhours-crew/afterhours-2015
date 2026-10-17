// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-car nitrous fuel, driven by installed curves and owned graph inputs.
//! Flow uses trapezoidal integration over elapsed engagement time. Released
//! tanks recharge from speed, continuous actions and discrete events.
use crate::scoring::{Error, finite};
use std::sync::Arc;

/// Vehicle curves expand the normalized asset knots into physical coordinates
/// once, then linearly interpolate those points. They do not use the scoring
/// graph's normalized/cubic evaluator; output endpoints may descend.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve(Vec<[f32; 2]>);
impl Curve {
    pub fn new(points: Vec<[f32; 2]>) -> Result<Self, Error> {
        if !(2..=256).contains(&points.len()) {
            return Err(Error::Bound);
        }
        for p in &points {
            finite(p[0])?;
            finite(p[1])?;
        }
        if points.windows(2).any(|w| w[0][0] >= w[1][0]) {
            return Err(Error::InvalidCurve);
        }
        for w in points.windows(2) {
            finite(w[1][0] - w[0][0])?;
            finite(w[1][1] - w[0][1])?;
        }
        Ok(Self(points))
    }
    pub fn evaluate(&self, input: f32) -> Result<f32, Error> {
        finite(input)?;
        let first = self.0[0];
        let last = self.0[self.0.len() - 1];
        if input < first[0] {
            return Ok(first[1]);
        }
        if input >= last[0] {
            return Ok(last[1]);
        }
        let right = self.0.partition_point(|p| p[0] <= input);
        let a = self.0[right - 1];
        let b = self.0[right];
        finite((b[1] - a[1]) / (b[0] - a[0]) * (input - a[0]) + a[1])
    }
}

/// Buildup nodes merge by OR/maximum, never by summing their event counts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Buildup {
    pub drifting: bool,
    pub drafting: bool,
    pub oncoming: bool,
    pub airborne: bool,
    pub speed: f32,
    pub near_misses: u32,
    pub cop_slams: u32,
    pub pacing: f32,
}
impl Buildup {
    pub fn merge(&mut self, other: Self) -> Result<(), Error> {
        finite(other.speed)?;
        finite(other.pacing)?;
        self.drifting |= other.drifting;
        self.drafting |= other.drafting;
        self.oncoming |= other.oncoming;
        self.airborne |= other.airborne;
        self.speed = self.speed.max(other.speed);
        self.pacing = self.pacing.max(other.pacing);
        self.near_misses = self.near_misses.max(other.near_misses);
        self.cop_slams = self.cop_slams.max(other.cop_slams);
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Boosts {
    pub drift: f32,
    pub draft: f32,
    pub near_miss: f32,
    pub oncoming: f32,
    pub airborne: f32,
    pub cop_slam: f32,
    pub draft_speed: f32,
    pub oncoming_speed: f32,
    pub airborne_speed: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Configuration {
    capacity: f32,
    flow: Curve,
    recharge: Curve,
    boosts: Boosts,
}
impl Configuration {
    pub fn new(capacity: f32, flow: Curve, recharge: Curve, boosts: Boosts) -> Result<Self, Error> {
        if finite(capacity)? <= 0.0 {
            return Err(Error::Bound);
        }
        for v in [
            boosts.drift,
            boosts.draft,
            boosts.near_miss,
            boosts.oncoming,
            boosts.airborne,
            boosts.cop_slam,
            boosts.draft_speed,
            boosts.oncoming_speed,
            boosts.airborne_speed,
        ] {
            if finite(v)? < 0.0 {
                return Err(Error::Bound);
            }
        }
        Ok(Self {
            capacity,
            flow,
            recharge,
            boosts,
        })
    }
    pub fn capacity(&self) -> f32 {
        self.capacity
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Fuel {
    config: Arc<Configuration>,
    amount: f32,
    elapsed: f32,
    engaged: bool,
    setting: u8,
}
impl Fuel {
    pub fn full(config: Arc<Configuration>, setting: u8) -> Result<Self, Error> {
        if setting > 10 {
            return Err(Error::Bound);
        }
        Ok(Self {
            amount: config.capacity,
            config,
            elapsed: 0.0,
            engaged: false,
            setting,
        })
    }
    pub fn capacity(&self) -> f32 {
        self.config.capacity
    }
    pub fn fraction(&self) -> f32 {
        (self.amount / self.capacity()).clamp(0.0, 1.0)
    }
    pub fn engaged(&self) -> bool {
        self.engaged
    }
    /// Empty stays engaged until the driver releases; a synthetic host release
    /// would replace the actual query event and its sampled amount.
    pub fn request(&mut self, engaged: bool) {
        if self.engaged != engaged {
            self.engaged = engaged;
            self.elapsed = 0.0;
        }
    }
    /// One bounded engine step. Failed evaluation leaves all state unchanged.
    /// This does not publish network corrections or choose scoring thresholds.
    pub fn tick(&mut self, seconds: f32, input: Buildup) -> Result<(), Error> {
        if !(0.0..=0.5).contains(&finite(seconds)?) {
            return Err(Error::Bound);
        }
        let mut checked = Buildup::default();
        checked.merge(input)?;
        let (amount, elapsed) = if self.engaged {
            let end = finite(self.elapsed + seconds)?;
            let start_rate = self.config.flow.evaluate(self.elapsed)?;
            let end_rate = self.config.flow.evaluate(end)?;
            let scale = (10.0 - f32::from(self.setting)) * 0.1 + 0.5;
            let consumed = finite((start_rate + end_rate) * 0.5 * seconds * scale)?;
            ((self.amount - consumed.max(0.0)).max(0.0), end)
        } else {
            let b = self.config.boosts;
            let mut rate = self.config.recharge.evaluate(checked.speed)? + checked.pacing;
            if checked.drifting {
                rate += b.drift;
            }
            if checked.drafting && checked.speed > b.draft_speed {
                rate += b.draft;
            }
            if checked.oncoming && checked.speed > b.oncoming_speed {
                rate += b.oncoming;
            }
            if checked.airborne && checked.speed > b.airborne_speed {
                rate += b.airborne;
            }
            let gain = finite(
                checked.near_misses as f32 * b.near_miss
                    + rate * seconds
                    + checked.cop_slams as f32 * b.cop_slam,
            )?;
            (finite(self.amount + gain)?.clamp(0.0, self.capacity()), 0.0)
        };
        self.amount = amount;
        self.elapsed = elapsed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn curve(end: f32) -> Curve {
        Curve::new(vec![[0.0, 0.0], [2.0, end]]).unwrap()
    }
    fn config() -> Arc<Configuration> {
        Arc::new(
            Configuration::new(
                8.0,
                curve(4.0),
                curve(0.4),
                Boosts {
                    drift: 1.0,
                    draft: 2.0,
                    near_miss: 0.5,
                    oncoming: 3.0,
                    airborne: 4.0,
                    cop_slam: 1.0,
                    draft_speed: 2.0,
                    oncoming_speed: 2.0,
                    airborne_speed: 2.0,
                },
            )
            .unwrap(),
        )
    }
    #[test]
    fn ramp_integrates_across_ticks_and_setting_changes_consumption() {
        let mut fuel = Fuel::full(config(), 5).unwrap();
        fuel.request(true);
        for _ in 0..4 {
            fuel.tick(0.5, Buildup::default()).unwrap();
        }
        assert_eq!(fuel.amount, 4.0);
        let mut fast = Fuel::full(config(), 0).unwrap();
        fast.request(true);
        for _ in 0..4 {
            fast.tick(0.5, Buildup::default()).unwrap();
        }
        assert_eq!(fast.amount, 2.0);
        fuel.request(false);
        fuel.request(true);
        fuel.tick(0.5, Buildup::default()).unwrap();
        assert_eq!(fuel.amount, 3.75, "release resets the flow ramp");
    }
    #[test]
    fn empty_waits_for_release_and_refill_counts_events_once() {
        let mut fuel = Fuel::full(config(), 5).unwrap();
        fuel.request(true);
        for _ in 0..10 {
            fuel.tick(0.5, Buildup::default()).unwrap();
        }
        assert_eq!(fuel.amount, 0.0);
        assert!(fuel.engaged());
        fuel.request(false);
        let input = Buildup {
            speed: 2.0,
            drifting: true,
            drafting: true,
            oncoming: true,
            airborne: true,
            near_misses: 2,
            cop_slams: 1,
            pacing: 0.2,
        };
        fuel.tick(0.5, input).unwrap();
        assert!((fuel.amount - 2.8).abs() < 1e-6, "strict speed gates");
        fuel.tick(
            0.5,
            Buildup {
                speed: 3.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!((fuel.amount - 3.0).abs() < 1e-6);
        let before = fuel.clone();
        assert!(fuel.tick(f32::NAN, input).is_err());
        assert_eq!(fuel, before);
        assert!(Fuel::full(config(), 11).is_err());
    }
    #[test]
    fn buildup_merges_maxima_without_double_counting() {
        let a = Buildup {
            drifting: true,
            speed: 10.0,
            near_misses: 2,
            ..Default::default()
        };
        let mut merged = Buildup::default();
        merged.merge(a).unwrap();
        merged.merge(a).unwrap();
        merged
            .merge(Buildup {
                airborne: true,
                speed: 5.0,
                cop_slams: 3,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(merged.near_misses, 2);
        assert_eq!(merged.speed, 10.0);
        assert!(merged.drifting && merged.airborne);
        let before = merged;
        assert!(
            merged
                .merge(Buildup {
                    speed: f32::INFINITY,
                    ..Default::default()
                })
                .is_err()
        );
        assert_eq!(merged, before);
    }
}
