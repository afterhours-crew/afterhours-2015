// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned nitrous fuel for one car (E920). The client asks the host to engage
//! and release (`vehicle_nitrous::receive`); the host answers the mode and
//! owns the fuel: it burns while engaged, stops the burn when empty and
//! refills while released. The replicated level (0–255) is what the gauge
//! follows.
//!
//! Figures: the three nitrous tiers (`items/NOS_2/3/4`, buy prices 2,125 /
//! 4,750 / 7,500) hold 5, 10 and 15 "lb". A full 5 lb time-refill tank burns
//! for 2.8 s (observed, E921: the client released on empty after 2.80 s
//! three times) and was full again within 6 s; the other tiers scale by lb
//! (hypothesis), and a released tank refills from empty in 6 s (skill tanks
//! only while the car drives with style). The fuel is a scoring input only: the
//! client predicts its own gauge from the mode acknowledgement. The installed
//! nitrous configurations (`RaceVehicleNOSConfigData`, E924) hold the native
//! figures: capacities 5, 8 and 12 units, a flow curve, a constant 0.35 units
//! per second refill for the time variants and action boosts for the others.
//! They are not adopted here yet because the flow curve's units are not
//! confirmed against the client; the observed 2.8 s burn of the small tank
//! stays the calibration until then, and both variants refill alike.
use std::time::Duration;

/// Refill from empty to full while released.
pub const REFILL: Duration = Duration::from_secs(6);
/// Replicated level steps (8-bit).
pub const LEVEL_STEPS: f32 = 255.0;

/// Refill kind of the installed item (`items/NOS_n_Time` refill on a clock,
/// `items/NOS_n` refill while the car drives with style).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Time,
    Skill,
}
impl Kind {
    /// The kind of a nitrous item definition (guids of the six installed
    /// `NosTuningItemData` assets, E920).
    pub fn from_definition(guid: &[u8; 16]) -> Option<Self> {
        const TIME: [[u8; 4]; 3] = [
            [0x24, 0xec, 0x4a, 0xb7],
            [0xb3, 0x59, 0x12, 0x22],
            [0x51, 0x44, 0x3d, 0xbf],
        ];
        const SKILL: [[u8; 4]; 3] = [
            [0xa3, 0x87, 0xe8, 0xd8],
            [0x30, 0xde, 0xe5, 0x7e],
            [0x75, 0x66, 0x82, 0x81],
        ];
        let head = [guid[0], guid[1], guid[2], guid[3]];
        if TIME.contains(&head) {
            Some(Self::Time)
        } else if SKILL.contains(&head) {
            Some(Self::Skill)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier {
    Two,
    Three,
    Four,
}
impl Tier {
    /// The tier of an installed nitrous item from its buy price (the only
    /// tier-bearing field the item content carries).
    pub fn from_buy_price(price: u32) -> Option<Self> {
        match price {
            2_125 => Some(Self::Two),
            4_750 => Some(Self::Three),
            7_500 => Some(Self::Four),
            _ => None,
        }
    }
    /// Capacity in the scoring query's unit ("lb").
    pub fn capacity(self) -> f32 {
        match self {
            Self::Two => 5.0,
            Self::Three => 10.0,
            Self::Four => 15.0,
        }
    }
    /// Burn time from full, seconds (0.56 s per lb).
    pub fn burn(self) -> f32 {
        match self {
            Self::Two => 2.8,
            Self::Three => 5.6,
            Self::Four => 8.4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    /// Empty: the host ends the burn.
    Stopped,
    /// The replicated level changed.
    Level(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fuel {
    tier: Tier,
    kind: Kind,
    /// Remaining burn time, seconds.
    remaining: f32,
    engaged: bool,
    /// Last replicated level.
    level: u8,
}
impl Fuel {
    pub fn full(tier: Tier, kind: Kind) -> Self {
        Self {
            tier,
            kind,
            remaining: tier.burn(),
            engaged: false,
            level: LEVEL_STEPS as u8,
        }
    }
    pub fn tier(self) -> Tier {
        self.tier
    }
    pub fn kind(self) -> Kind {
        self.kind
    }
    pub fn engaged(self) -> bool {
        self.engaged
    }
    /// Remaining share of a full tank.
    pub fn fraction(self) -> f32 {
        (self.remaining / self.tier.burn()).clamp(0.0, 1.0)
    }
    pub fn level(self) -> u8 {
        (self.fraction() * LEVEL_STEPS + 0.5) as u8
    }
    /// The driver's request. Engaging an empty tank is refused (false).
    pub fn request(&mut self, engaged: bool) -> bool {
        if engaged && self.remaining <= 0.0 {
            self.engaged = false;
            return false;
        }
        self.engaged = engaged;
        true
    }
    /// Advance by `elapsed`; `building` says whether the car is driving with
    /// style this tick (skill-refill tanks refill only then). The changes,
    /// in order.
    pub fn tick(&mut self, elapsed: Duration, building: bool) -> Vec<Change> {
        let seconds = elapsed.as_secs_f32();
        let mut changes = Vec::new();
        if self.engaged {
            self.remaining = (self.remaining - seconds).max(0.0);
            if self.remaining <= 0.0 {
                self.engaged = false;
                changes.push(Change::Stopped);
            }
        } else if self.remaining < self.tier.burn() && (self.kind == Kind::Time || building) {
            let rate = self.tier.burn() / REFILL.as_secs_f32();
            self.remaining = (self.remaining + seconds * rate).min(self.tier.burn());
        }
        let level = self.level();
        if level != self.level {
            self.level = level;
            changes.push(Change::Level(level));
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn burns_while_engaged_stops_when_empty_and_refills_released() {
        let mut fuel = Fuel::full(Tier::Two, Kind::Time);
        assert_eq!(fuel.level(), 255);
        assert!(fuel.request(true));
        let mut stopped_at = None;
        let mut levels = 0;
        for step in 1..=200 {
            for change in fuel.tick(Duration::from_millis(33), false) {
                match change {
                    Change::Stopped => stopped_at = Some(step),
                    Change::Level(_) => levels += 1,
                }
            }
            if stopped_at.is_some() {
                break;
            }
        }
        // 2.8 s of burn at 33 ms steps: stops after about 85 steps.
        let stopped = stopped_at.unwrap();
        assert!((82..=88).contains(&stopped), "{stopped}");
        assert!(!fuel.engaged());
        assert_eq!(fuel.level(), 0);
        assert!(levels >= 60);
        // Empty: engaging is refused until some fuel is back.
        assert!(!fuel.request(true));
        for _ in 0..6 {
            fuel.tick(Duration::from_millis(100), false);
        }
        // 0.6 s of 6 s refill: a tenth of the tank.
        assert!(
            (0.09..=0.11).contains(&fuel.fraction()),
            "{}",
            fuel.fraction()
        );
        assert!(fuel.request(true));
        assert!(fuel.engaged());
        assert!(fuel.request(false));
        assert!(!fuel.engaged());
    }
    #[test]
    fn tiers_come_from_the_buy_price() {
        assert_eq!(Tier::from_buy_price(2_125), Some(Tier::Two));
        assert_eq!(Tier::from_buy_price(4_750), Some(Tier::Three));
        assert_eq!(Tier::from_buy_price(7_500), Some(Tier::Four));
        assert_eq!(Tier::from_buy_price(1), None);
        assert_eq!(Tier::Four.capacity(), 15.0);
        assert!(Fuel::full(Tier::Two, Kind::Skill).fraction() == 1.0);
        assert_eq!(
            Kind::from_definition(&[0x24, 0xec, 0x4a, 0xb7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(Kind::Time)
        );
        assert_eq!(
            Kind::from_definition(&[0x30, 0xde, 0xe5, 0x7e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(Kind::Skill)
        );
        assert_eq!(Kind::from_definition(&[0; 16]), None);
        // A skill tank refills only while building.
        let mut skill = Fuel::full(Tier::Two, Kind::Skill);
        skill.request(true);
        // 3.3 s of ticks empty the 2.8 s tank.
        for _ in 0..100 {
            skill.tick(Duration::from_millis(33), false);
        }
        assert_eq!(skill.fraction(), 0.0);
        skill.tick(Duration::from_secs(3), false);
        assert_eq!(skill.fraction(), 0.0);
        skill.tick(Duration::from_secs(3), true);
        assert!(
            (0.49..=0.51).contains(&skill.fraction()),
            "{}",
            skill.fraction()
        );
    }
}
