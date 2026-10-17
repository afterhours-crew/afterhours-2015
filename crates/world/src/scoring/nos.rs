// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned nitrous query for `RaceVehicleNosQuery` (E916, inferred from the
//! installed wiring): `Capacity` is read continuously, `EngagedLevel` is
//! cached when `Engaged` fires and `DisengagedLevel` is read when
//! `Disengaged` fires; the prefab scores `(engaged - disengaged) * capacity *
//! 20` against 100. The measurement (whether nitrous is engaged, the current
//! level and the capacity) is an owned server input; the node turns its
//! transitions into the query's publications and events.
use super::{Error, finite};

/// `EngagedLevel`, `DisengagedLevel`, `Capacity` (hashes of the names).
pub const ENGAGED_LEVEL: u32 = 1_491_554_552;
pub const DISENGAGED_LEVEL: u32 = 2_868_135_622;
pub const CAPACITY: u32 = 3_607_789_937;
/// `Engaged` and `Disengaged` events (hashes of the names).
pub const ENGAGED: u32 = 0xff75_a0ee;
pub const DISENGAGED: u32 = 0x970b_3e90;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub engaged: bool,
    /// Current nitrous level in the capacity's unit.
    pub level: f32,
    pub capacity: f32,
}
impl Sample {
    pub fn validate(self) -> Result<(), Error> {
        for value in [self.level, self.capacity] {
            if finite(value)? < 0.0 {
                return Err(Error::Bound);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Publish(u32, f32),
    Emit(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Node {
    engaged: bool,
    capacity: Option<f32>,
}
impl Node {
    /// Effects of one measurement, in publication order: a changed capacity,
    /// then the level at an engage or disengage transition followed by its event.
    pub fn step(&mut self, sample: Sample) -> Result<Vec<Effect>, Error> {
        sample.validate()?;
        let mut effects = Vec::new();
        if self.capacity != Some(sample.capacity) {
            self.capacity = Some(sample.capacity);
            effects.push(Effect::Publish(CAPACITY, sample.capacity));
        }
        if sample.engaged != self.engaged {
            self.engaged = sample.engaged;
            if sample.engaged {
                effects.push(Effect::Publish(ENGAGED_LEVEL, sample.level));
                effects.push(Effect::Emit(ENGAGED));
            } else {
                effects.push(Effect::Publish(DISENGAGED_LEVEL, sample.level));
                effects.push(Effect::Emit(DISENGAGED));
            }
        }
        Ok(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transitions_publish_levels_and_fire_in_order() {
        let mut node = Node::default();
        let idle = Sample {
            engaged: false,
            level: 1.0,
            capacity: 5.0,
        };
        assert_eq!(
            node.step(idle).unwrap(),
            vec![Effect::Publish(CAPACITY, 5.0)]
        );
        assert_eq!(node.step(idle).unwrap(), vec![]);
        let on = Sample {
            engaged: true,
            ..idle
        };
        assert_eq!(
            node.step(on).unwrap(),
            vec![Effect::Publish(ENGAGED_LEVEL, 1.0), Effect::Emit(ENGAGED)]
        );
        assert_eq!(node.step(on).unwrap(), vec![]);
        let off = Sample {
            engaged: false,
            level: 0.4,
            capacity: 5.0,
        };
        assert_eq!(
            node.step(off).unwrap(),
            vec![
                Effect::Publish(DISENGAGED_LEVEL, 0.4),
                Effect::Emit(DISENGAGED)
            ]
        );
        assert!(
            node.step(Sample {
                level: f32::INFINITY,
                ..off
            })
            .is_err()
        );
        assert!(
            node.step(Sample {
                capacity: -1.0,
                ..off
            })
            .is_err()
        );
    }
}
