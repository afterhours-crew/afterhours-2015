// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Player-chosen amounts of ambient traffic and roaming rivals. The defaults
//! reproduce the original game; any other value is explicit host policy.

use nfs_world::replication::players::ai::MAX_AI;

/// Most traffic accepted, as a percentage of the original amount (5x). The
/// nearby target grows from the original 2-10 cars per player to at most 50;
/// traffic never takes the AI cars reserved for rivals.
pub const MAX_TRAFFIC_PERCENT: u16 = 500;
/// Largest roaming-rival count accepted. Above the game's own, region-driven
/// number the host activates more of the installed racers, each at most once.
pub const MAX_RIVALS: u16 = 15;
/// AI cars ambient traffic leaves free, so every chosen rival can get a car
/// however much traffic there is.
pub const RIVAL_CAR_RESERVE: usize = MAX_RIVALS as usize;
const _: () = assert!(RIVAL_CAR_RESERVE < MAX_AI);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AiSettings {
    traffic_percent: u16,
    rival_limit: Option<u16>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            traffic_percent: 100,
            rival_limit: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    TrafficPercent,
    RivalLimit,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TrafficPercent => write!(f, "traffic must be 0 to {MAX_TRAFFIC_PERCENT} percent"),
            Self::RivalLimit => write!(f, "rivals must be 0 to {MAX_RIVALS}"),
        }
    }
}

impl std::error::Error for Error {}

impl AiSettings {
    /// `traffic_percent` scales the original traffic (0 turns it off);
    /// `rival_limit` is the number of roaming rivals kept active (0 turns
    /// them off; `None` keeps the original, region-driven number). The
    /// game's own racers are capped at it, and other installed racers near
    /// the player are added up to it.
    pub fn new(traffic_percent: u16, rival_limit: Option<u16>) -> Result<Self, Error> {
        if traffic_percent > MAX_TRAFFIC_PERCENT {
            return Err(Error::TrafficPercent);
        }
        if rival_limit.is_some_and(|limit| limit > MAX_RIVALS) {
            return Err(Error::RivalLimit);
        }
        Ok(Self {
            traffic_percent,
            rival_limit,
        })
    }

    pub fn traffic_percent(&self) -> u16 {
        self.traffic_percent
    }

    pub fn rival_limit(&self) -> Option<u16> {
        self.rival_limit
    }
}

/// `count` scaled by `percent`, rounded to the nearest whole number.
pub fn scale(count: u32, percent: u16) -> u32 {
    let scaled = (u64::from(count) * u64::from(percent) + 50) / 100;
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_original_amounts_and_bounds_are_enforced() {
        let original = AiSettings::default();
        assert_eq!(
            (original.traffic_percent(), original.rival_limit()),
            (100, None)
        );
        assert_eq!(AiSettings::new(100, None), Ok(original));
        assert!(AiSettings::new(0, Some(0)).is_ok());
        assert!(AiSettings::new(MAX_TRAFFIC_PERCENT, Some(MAX_RIVALS)).is_ok());
        assert_eq!(
            AiSettings::new(MAX_TRAFFIC_PERCENT + 1, None),
            Err(Error::TrafficPercent)
        );
        assert_eq!(
            AiSettings::new(100, Some(MAX_RIVALS + 1)),
            Err(Error::RivalLimit)
        );
    }

    #[test]
    fn scaling_rounds_and_keeps_the_original_at_100_percent() {
        for count in 0..=40 {
            assert_eq!(scale(count, 100), count);
            assert_eq!(scale(count, 0), 0);
        }
        assert_eq!((scale(10, 50), scale(3, 50), scale(1, 50)), (5, 2, 1));
        assert_eq!((scale(10, 300), scale(7, 150)), (30, 11));
        // 5x: the original 2-10 nearby target becomes 10-50.
        assert_eq!((scale(2, 500), scale(10, 500)), (10, 50));
        assert_eq!(scale(u32::MAX, MAX_TRAFFIC_PERCENT), u32::MAX);
    }
}
