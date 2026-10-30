// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned vehicle figures for `VehicleStatsProvider` (E916). The port names
//! were not recovered; their roles are inferred from the installed consumers:
//! the continuous max-speed prefab casts two integer outputs, drives its
//! threshold curve (0..300) from one and forms `(a - b) / (240 - b) * 2` from
//! both, and the acceleration prefab forms `(a - b) / (2 - b) * 2` from two
//! float outputs while guarding `a < 1`. Both read as a current figure against
//! the stock figure of the same car: top speed in mph and the 0-60 time in
//! seconds, which are the dyno figures the client stores on the car item and
//! the item definition's defaults. Three further outputs (two floats whose
//! difference and one value reach the parent interface only) are declared but
//! never published here. Current and stock MaxHP are also available: the
//! provider reads the owned car's saved dyno value and its definition baseline.
use super::{Error, finite};

/// Current MaxHP from the owned car's dyno cache.
pub const MAX_HP: u32 = 2_836_079_500;
/// Stock MaxHP from that car's installed definition.
pub const STOCK_MAX_HP: u32 = 1_473_716_677;
/// Current top speed, mph (integer output).
pub const TOP_SPEED: u32 = 4_065_231_830;
/// Stock top speed, mph (integer output).
pub const STOCK_TOP_SPEED: u32 = 3_742_822_015;
/// Current 0-60 mph time, seconds (float output).
pub const ZERO_TO_SIXTY: u32 = 3_755_008_883;
/// Stock 0-60 mph time, seconds (float output).
pub const STOCK_ZERO_TO_SIXTY: u32 = 3_312_738_714;
/// Float outputs read only by the build category's interface (unpublished).
pub const UNPUBLISHED_FLOATS: [u32; 2] = [3_493_158_927, 3_337_027_526];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub max_hp: u32,
    pub stock_max_hp: u32,
    pub top_speed_mph: u32,
    pub stock_top_speed_mph: u32,
    pub zero_to_sixty: f32,
    pub stock_zero_to_sixty: f32,
}
impl Stats {
    pub fn validate(self) -> Result<(), Error> {
        if self.max_hp > 1500 || self.stock_max_hp > 1500 {
            return Err(Error::Bound);
        }
        for value in [self.zero_to_sixty, self.stock_zero_to_sixty] {
            if finite(value)? < 0.0 {
                return Err(Error::Bound);
            }
        }
        if self.top_speed_mph > 1000 || self.stock_top_speed_mph > 1000 {
            return Err(Error::Bound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn figures_are_finite_and_bounded() {
        let ok = Stats {
            max_hp: 340,
            stock_max_hp: 205,
            top_speed_mph: 180,
            stock_top_speed_mph: 155,
            zero_to_sixty: 4.1,
            stock_zero_to_sixty: 4.79,
        };
        assert!(ok.validate().is_ok());
        for bad in [
            Stats { max_hp: 1501, ..ok },
            Stats {
                stock_max_hp: 1501,
                ..ok
            },
        ] {
            assert_eq!(bad.validate(), Err(Error::Bound));
        }
        assert!(
            Stats {
                zero_to_sixty: f32::NAN,
                ..ok
            }
            .validate()
            .is_err()
        );
        assert!(
            Stats {
                stock_zero_to_sixty: -1.0,
                ..ok
            }
            .validate()
            .is_err()
        );
        assert!(
            Stats {
                top_speed_mph: 1001,
                ..ok
            }
            .validate()
            .is_err()
        );
    }
}
