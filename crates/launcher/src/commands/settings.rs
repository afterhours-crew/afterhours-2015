// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `settings`: show or change the saved traffic and rival amounts.

use super::Context;
use crate::{
    cli::SettingsArgs,
    contract::SettingsReport,
    exit::{Exit, Failure, Result},
    paths,
    session::Lock,
    settings::{self, Rivals},
};
use std::path::Path;

/// Print the saved amounts, or save new ones when an option is given.
pub fn settings(context: &Context, args: &SettingsArgs) -> Result<()> {
    let (values, report) = apply(&context.data_dir, args.traffic, args.rivals)?;
    context.emit(&report, || {
        let mut lines = Vec::new();
        if report.saved {
            lines.push("Saved. Later sessions use these values.".to_owned());
        }
        lines.push(format!("Traffic and rivals: {}", values.describe()));
        lines.push(format!("Settings file: {}", report.file.display()));
        lines.join("\n")
    })
}

/// Load the settings, or apply and save the given changes.
fn apply(
    data: &Path,
    traffic: Option<u16>,
    rivals: Option<Rivals>,
) -> Result<(settings::Settings, SettingsReport)> {
    let data = paths::checked_data_path(data, None)?;
    let change = traffic.is_some() || rivals.is_some();
    let values = if change {
        std::fs::create_dir_all(&data)?;
        // A running session keeps the values it started with, and the lock
        // keeps this write from interleaving with a session that is starting.
        let _lock = Lock::acquire(&data).map_err(|failure| {
            if failure.exit == Exit::Refused {
                Failure::refused(
                    "A session is running in this data directory and keeps the traffic and \
                     rivals it started with. Change them after it ends.",
                )
            } else {
                failure
            }
        })?;
        settings::resolve(&data, traffic, rivals)?
    } else {
        settings::load(&data)?
    };
    let report = SettingsReport::new(&values, data.join(settings::FILE), change);
    Ok((values, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_changes_nothing_and_saving_waits_for_the_session() {
        let data = crate::testing::scratch("settings-command");
        let (values, report) = apply(&data, None, None).unwrap();
        assert_eq!(values, settings::Settings::default());
        assert!(!report.saved && !report.file.exists());
        assert_eq!(
            (report.max_traffic_percent, report.max_rivals),
            (settings::MAX_TRAFFIC_PERCENT, settings::MAX_RIVALS)
        );
        crate::contract::testing::validate("settings", &report);

        let (values, report) = apply(&data, Some(200), Some(Rivals::AtMost(0))).unwrap();
        assert_eq!((values.traffic_percent, values.rival_limit), (200, Some(0)));
        assert!(report.saved && report.file.exists());
        assert_eq!((report.traffic_percent, report.rival_limit), (200, Some(0)));
        crate::contract::testing::validate("settings", &report);
        let (_, report) = apply(&data, None, None).unwrap();
        assert_eq!((report.traffic_percent, report.rival_limit), (200, Some(0)));
        assert!(!report.saved);

        // A session holding the data directory keeps its values.
        let session = Lock::acquire(&data).unwrap();
        let refused = apply(&data, Some(100), None).unwrap_err();
        assert_eq!(refused.exit, Exit::Refused);
        assert!(
            refused.message.contains("after it ends"),
            "{}",
            refused.message
        );
        let (_, report) = apply(&data, None, None).unwrap();
        assert_eq!(report.traffic_percent, 200, "reading still works");
        drop(session);
        let (_, report) = apply(&data, None, Some(Rivals::Original)).unwrap();
        assert_eq!((report.traffic_percent, report.rival_limit), (200, None));

        // A damaged file is reported, not replaced.
        std::fs::write(data.join(settings::FILE), b"{\"traffic_percent\": 900}").unwrap();
        assert!(apply(&data, None, None).is_err());
        assert!(apply(&data, Some(100), None).is_err());
        std::fs::remove_dir_all(data).unwrap();
    }
}
