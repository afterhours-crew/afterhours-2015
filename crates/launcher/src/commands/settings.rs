// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `settings`: show or change the saved traffic and rival amounts.

use super::Context;
use crate::{
    cli::SettingsArgs,
    contract::SettingsReport,
    exit::Result,
    paths,
    settings::{self, Rivals},
};
use std::path::Path;

/// Print the saved amounts, or save new ones when an option is given.
pub fn settings(context: &Context, args: &SettingsArgs) -> Result<()> {
    let (values, report) = apply(&context.data_dir, args.traffic, args.rivals)?;
    context.emit(&report, || {
        let mut lines = Vec::new();
        if report.saved {
            lines.push(
                "Saved. A running session applies them within about a second; later sessions \
                 start with them."
                    .to_owned(),
            );
        }
        lines.push(format!("Traffic and rivals: {}", values.describe()));
        lines.push(format!("Settings file: {}", report.file.display()));
        lines.join("\n")
    })
}

/// Load the settings, or apply and save the given changes. Saving needs no
/// session lock: the file is replaced atomically, a session that is starting
/// reads either version, and a running server applies the new one when it
/// sees the file change.
fn apply(
    data: &Path,
    traffic: Option<u16>,
    rivals: Option<Rivals>,
) -> Result<(settings::Settings, SettingsReport)> {
    let data = paths::checked_data_path(data, None)?;
    let change = traffic.is_some() || rivals.is_some();
    let values = if change {
        std::fs::create_dir_all(&data)?;
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
    use crate::session::Lock;

    #[test]
    fn reading_changes_nothing_and_saving_works_while_a_session_runs() {
        let data = crate::testing::scratch("settings-command");
        let (values, report) = apply(&data, None, None).unwrap();
        assert_eq!(values, settings::Settings::default());
        assert!(!report.saved && !report.file.exists());
        assert_eq!(
            (
                report.max_traffic_percent,
                report.max_rivals,
                report.rivals_above_original,
                report.live
            ),
            (300, 15, true, true)
        );
        crate::contract::testing::validate("settings", &report);

        let (values, report) = apply(&data, Some(200), Some(Rivals::Count(0))).unwrap();
        assert_eq!((values.traffic_percent, values.rival_limit), (200, Some(0)));
        assert!(report.saved && report.file.exists());
        assert_eq!((report.traffic_percent, report.rival_limit), (200, Some(0)));
        crate::contract::testing::validate("settings", &report);
        let (_, report) = apply(&data, None, None).unwrap();
        assert_eq!((report.traffic_percent, report.rival_limit), (200, Some(0)));
        assert!(!report.saved);

        // A session holding the data directory picks changes up from the
        // file, so saving goes ahead (up, off and back) and leaves no
        // temporary file behind.
        let session = Lock::acquire(&data).unwrap();
        for (traffic, rivals, expected) in [
            (Some(300), Some(Rivals::Count(15)), (300, Some(15))),
            (Some(0), Some(Rivals::Count(0)), (0, Some(0))),
            (Some(100), Some(Rivals::Original), (100, None)),
        ] {
            let (_, report) = apply(&data, traffic, rivals).unwrap();
            assert!(report.saved);
            assert_eq!((report.traffic_percent, report.rival_limit), expected);
            let saved = settings::load(&data).unwrap();
            assert_eq!((saved.traffic_percent, saved.rival_limit), expected);
        }
        let leftovers = std::fs::read_dir(&data)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        drop(session);
        // A later session starts with the last saved values.
        let (_, report) = apply(&data, None, None).unwrap();
        assert_eq!((report.traffic_percent, report.rival_limit), (100, None));

        // A damaged file is reported, not replaced.
        std::fs::write(data.join(settings::FILE), b"{\"traffic_percent\": 900}").unwrap();
        assert!(apply(&data, None, None).is_err());
        assert!(apply(&data, Some(100), None).is_err());
        std::fs::remove_dir_all(data).unwrap();
    }
}
