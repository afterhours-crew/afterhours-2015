// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! UTC timestamps for status records and console lines, without a date
//! crate.

use crate::marker::UtcDate;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn split(time: SystemTime) -> (UtcDate, u64) {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    (UtcDate::from_system_time(time), seconds % 86_400)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for an instant.
pub fn iso_utc(time: SystemTime) -> String {
    let (date, day_seconds) = split(time);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date.iso(),
        day_seconds / 3600,
        day_seconds % 3600 / 60,
        day_seconds % 60
    )
}

/// `HH:MM:SS` for an instant, used as the console line prefix.
pub fn hms_utc(time: SystemTime) -> String {
    let (_, day_seconds) = split(time);
    format!(
        "{:02}:{:02}:{:02}",
        day_seconds / 3600,
        day_seconds % 3600 / 60,
        day_seconds % 60
    )
}

/// Current time as ISO UTC.
pub fn now_iso() -> String {
    iso_utc(SystemTime::now())
}

/// Current time as `HH:MM:SS` UTC.
pub fn now_hms() -> String {
    hms_utc(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_format_as_iso_utc() {
        let t = UNIX_EPOCH + Duration::from_secs(20_739 * 86_400 + 23 * 3_600 + 5 * 60 + 9);
        assert_eq!(iso_utc(t), "2026-10-13T23:05:09Z");
        assert_eq!(hms_utc(t), "23:05:09");
        assert_eq!(iso_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }
}
