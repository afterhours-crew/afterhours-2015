// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The date-derived launch marker the game expects in its environment.
//!
//! The installed client reads `EARtPLaunchCode` at startup and compares it
//! with a value it derives from the current UTC date. The value is launch
//! context, not a credential: anyone with a calendar can compute it. It must
//! use the UTC date, not the local wall-clock date, which differ near midnight.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Civil UTC date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDate {
    pub year: i64,
    pub month: i64,
    pub day: i64,
}

impl UtcDate {
    /// Current UTC date from the system clock.
    pub fn today() -> Self {
        Self::from_system_time(SystemTime::now())
    }

    /// UTC date of an instant. Instants before 1970 clamp to the epoch.
    pub fn from_system_time(time: SystemTime) -> Self {
        let seconds = time
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self::from_days(i64::try_from(seconds / 86_400).unwrap_or(i64::MAX))
    }

    /// Days since 1970-01-01 to a proleptic Gregorian date.
    /// This is the public-domain algorithm from Howard Hinnant's
    /// "chrono-compatible low-level date algorithms".
    pub fn from_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        Self {
            year: if month <= 2 { year + 1 } else { year },
            month,
            day,
        }
    }

    /// `YYYY-MM-DD`.
    pub fn iso(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// The marker for a UTC date, as the decimal string the game compares.
///
/// Mixes day, year and month with fixed multipliers, folds the result with
/// its own 16-bit shifts and keeps the low 32 bits. The constants come from
/// the client's own check and are reproduced here, not derived.
pub fn launch_marker(date: UtcDate) -> u32 {
    let mixed = (date.day * 0x558a9) ^ (date.year * 0x19919) ^ (date.month * 0x36de1);
    let folded = mixed ^ (mixed >> 16) ^ (mixed << 16);
    (folded & 0xffff_ffff) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_matches_the_recorded_vectors() {
        // Vectors recorded from the PowerShell launcher's own tests.
        let d = |year, month, day| UtcDate { year, month, day };
        assert_eq!(launch_marker(d(2026, 10, 13)), 1_497_848_135);
        assert_eq!(launch_marker(d(2026, 10, 14)), 14_876_899);
    }

    #[test]
    fn dates_come_from_utc_not_local_wall_clock() {
        // 2026-10-14T01:00:00+02:00 is 2026-10-13T23:00:00Z.
        let seconds = 20_739 * 86_400 + 23 * 3_600;
        let date = UtcDate::from_system_time(UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(
            date,
            UtcDate {
                year: 2026,
                month: 10,
                day: 13
            }
        );
        assert_eq!(launch_marker(date), 1_497_848_135);
    }

    #[test]
    fn civil_date_conversion_handles_boundaries() {
        assert_eq!(UtcDate::from_days(0).iso(), "1970-01-01");
        assert_eq!(UtcDate::from_days(-1).iso(), "1969-12-31");
        assert_eq!(UtcDate::from_days(11_016).iso(), "2000-02-29");
        assert_eq!(UtcDate::from_days(20_739).iso(), "2026-10-13");
    }
}
