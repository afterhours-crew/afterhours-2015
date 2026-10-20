// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Player-chosen amounts of ambient traffic and roaming rivals. The defaults
//! reproduce the original game; any other value is explicit host policy.

use nfs_world::replication::players::ai::MAX_AI;

/// Most traffic accepted, as a percentage of the original amount (3x). The
/// nearby target grows from the original 2-10 cars per player to at most 30;
/// traffic never takes the AI cars reserved for rivals. Not more: the client
/// gives every car four entries in one global manager whose linear allocator
/// is fixed at 64 pages of 4 KiB (constructor `03801246`, allocator
/// `0341f7e0`, which breaks into the debugger when full), and a 5x test
/// crashed there with about 45-50 AI cars around the player.
pub const MAX_TRAFFIC_PERCENT: u16 = 300;
/// Test builds accepted up to this; a saved amount above
/// [`MAX_TRAFFIC_PERCENT`] and up to this reads as [`MAX_TRAFFIC_PERCENT`].
const OLDER_MAX_TRAFFIC_PERCENT: u64 = 500;
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
    /// Not the launcher's `settings.json` shape: a JSON object with only
    /// `traffic_percent` (integer) and `rival_limit` (integer or null).
    File,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TrafficPercent => write!(f, "traffic must be 0 to {MAX_TRAFFIC_PERCENT} percent"),
            Self::RivalLimit => write!(f, "rivals must be 0 to {MAX_RIVALS}"),
            Self::File => write!(
                f,
                "settings must be an object with traffic_percent and rival_limit"
            ),
        }
    }
}

/// Builds before the rival count accepted a cap up to this; a saved value
/// above [`MAX_RIVALS`] and up to this reads as [`MAX_RIVALS`], as the
/// launcher reads it.
const OLDER_MAX_RIVALS: u64 = 128;

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

    /// Parse the launcher's `settings.json`: missing fields are the
    /// originals, unknown fields and out-of-range values are refused.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Error::File)?;
        let object = value.as_object().ok_or(Error::File)?;
        if object
            .keys()
            .any(|key| key != "traffic_percent" && key != "rival_limit")
        {
            return Err(Error::File);
        }
        let traffic_percent = match object.get("traffic_percent") {
            None => 100,
            Some(value) => match value.as_u64().ok_or(Error::File)? {
                percent
                    if percent > u64::from(MAX_TRAFFIC_PERCENT)
                        && percent <= OLDER_MAX_TRAFFIC_PERCENT =>
                {
                    MAX_TRAFFIC_PERCENT
                }
                percent => u16::try_from(percent).map_err(|_| Error::TrafficPercent)?,
            },
        };
        let rival_limit = match object.get("rival_limit") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => match value.as_u64().ok_or(Error::File)? {
                limit if limit > u64::from(MAX_RIVALS) && limit <= OLDER_MAX_RIVALS => {
                    Some(MAX_RIVALS)
                }
                limit => Some(u16::try_from(limit).map_err(|_| Error::RivalLimit)?),
            },
        };
        Self::new(traffic_percent, rival_limit)
    }
}

/// A running server's current settings. Every world session reads it each
/// tick, so a change the player saves applies to sessions already running.
#[derive(Clone, Debug)]
pub struct Live(tokio::sync::watch::Receiver<AiSettings>);

impl Live {
    /// Settings that never change, for a server without a settings file.
    pub fn fixed(settings: AiSettings) -> Self {
        Self(tokio::sync::watch::channel(settings).1)
    }

    /// Settings the returned sender can change.
    pub fn channel(settings: AiSettings) -> (tokio::sync::watch::Sender<AiSettings>, Self) {
        let (sender, receiver) = tokio::sync::watch::channel(settings);
        (sender, Self(receiver))
    }

    pub fn current(&self) -> AiSettings {
        *self.0.borrow()
    }

    /// The settings, when they changed since this handle last looked.
    pub fn changed(&mut self) -> Option<AiSettings> {
        self.0
            .has_changed()
            .unwrap_or(false)
            .then(|| *self.0.borrow_and_update())
    }
}

impl Default for Live {
    fn default() -> Self {
        Self::fixed(AiSettings::default())
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
        // 3x: the original 2-10 nearby target becomes 6-30.
        assert_eq!((scale(2, 300), scale(10, 300)), (6, 30));
        assert_eq!(scale(u32::MAX, MAX_TRAFFIC_PERCENT), u32::MAX);
    }

    #[test]
    fn the_settings_file_reads_like_the_launcher_reads_it() {
        let read = |text: &str| AiSettings::from_json(text.as_bytes());
        assert_eq!(
            read(r#"{"traffic_percent": 300, "rival_limit": 15}"#),
            AiSettings::new(300, Some(15))
        );
        // Test builds allowed up to 500 %; such a file reads as 300 %.
        assert_eq!(
            read(r#"{"traffic_percent": 500, "rival_limit": 15}"#),
            AiSettings::new(300, Some(15))
        );
        assert_eq!(read(r#"{"rival_limit": null}"#), Ok(AiSettings::default()));
        assert_eq!(read("{}"), Ok(AiSettings::default()));
        assert_eq!(read(r#"{"rival_limit": 0}"#), AiSettings::new(100, Some(0)));
        // A count saved by an older build (up to 128) reads as 15.
        assert_eq!(
            read(r#"{"rival_limit": 128}"#),
            AiSettings::new(100, Some(15))
        );
        for (text, error) in [
            (r#"{"traffic_percent": 501}"#, Error::TrafficPercent),
            (r#"{"traffic_percent": 301.5}"#, Error::File),
            (r#"{"traffic_percent": 70000}"#, Error::TrafficPercent),
            (r#"{"rival_limit": 129}"#, Error::RivalLimit),
            (r#"{"traffic_percent": -5}"#, Error::File),
            (r#"{"traffic_percent": "lots"}"#, Error::File),
            (r#"{"traffic": 50}"#, Error::File),
            ("[]", Error::File),
            ("not json", Error::File),
            ("", Error::File),
        ] {
            assert_eq!(read(text), Err(error), "{text}");
        }
    }

    #[test]
    fn live_settings_report_each_change_once_to_every_handle() {
        let (sender, mut first) = Live::channel(AiSettings::default());
        let mut second = first.clone();
        assert_eq!((first.changed(), second.changed()), (None, None));
        let busy = AiSettings::new(300, Some(15)).unwrap();
        sender.send_replace(busy);
        assert_eq!((first.changed(), first.changed()), (Some(busy), None));
        assert_eq!((second.changed(), second.current()), (Some(busy), busy));
        let mut fixed = Live::fixed(busy);
        assert_eq!((fixed.changed(), fixed.current()), (None, busy));
        assert_eq!(Live::default().current(), AiSettings::default());
    }
}
