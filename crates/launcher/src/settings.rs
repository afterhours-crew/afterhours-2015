// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Game settings the player chooses, kept in `settings.json` in the data
//! directory and applied to every session. Edit the file, or pass
//! `--traffic` and `--rivals` to `start`, `serve` or `setup`; those save
//! their values for later sessions. The defaults are the original game.

use crate::{
    exit::{Failure, Result},
    records,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "settings.json";
/// Same bounds as the server's `--traffic-percent` and `--rival-limit`.
pub const MAX_TRAFFIC_PERCENT: u16 = 300;
pub const MAX_RIVALS: u16 = 128;
const ORIGINAL_TRAFFIC: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Ambient traffic as a percentage of the original amount; 0 turns it off.
    #[serde(default = "original_traffic")]
    pub traffic_percent: u16,
    /// Most roaming rivals active at once; 0 turns them off. Null keeps the
    /// original number.
    #[serde(default)]
    pub rival_limit: Option<u16>,
}

fn original_traffic() -> u16 {
    ORIGINAL_TRAFFIC
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            traffic_percent: ORIGINAL_TRAFFIC,
            rival_limit: None,
        }
    }
}

/// `--rivals`: `original` or the most rivals active at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rivals {
    Original,
    AtMost(u16),
}

/// Parse a `--rivals` value.
pub fn parse_rivals(value: &str) -> std::result::Result<Rivals, String> {
    if value.eq_ignore_ascii_case("original") {
        return Ok(Rivals::Original);
    }
    match value.parse::<u16>() {
        Ok(count) if count <= MAX_RIVALS => Ok(Rivals::AtMost(count)),
        _ => Err(format!("expected `original` or 0 to {MAX_RIVALS}")),
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.traffic_percent > MAX_TRAFFIC_PERCENT
            || self.rival_limit.is_some_and(|limit| limit > MAX_RIVALS)
        {
            return Err(Failure::refused(format!(
                "Invalid {FILE}: traffic_percent must be 0 to {MAX_TRAFFIC_PERCENT} and \
                 rival_limit null or 0 to {MAX_RIVALS}."
            )));
        }
        Ok(())
    }

    /// Server options for these settings; none when they are the original.
    pub fn server_arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if self.traffic_percent != ORIGINAL_TRAFFIC {
            arguments.push("--traffic-percent".into());
            arguments.push(self.traffic_percent.to_string());
        }
        if let Some(limit) = self.rival_limit {
            arguments.push("--rival-limit".into());
            arguments.push(limit.to_string());
        }
        arguments
    }

    /// One line for the terminal.
    pub fn describe(&self) -> String {
        let traffic = match self.traffic_percent {
            0 => "no traffic".to_owned(),
            ORIGINAL_TRAFFIC => "original traffic".to_owned(),
            percent => format!("{percent}% traffic"),
        };
        let rivals = match self.rival_limit {
            None => "original rivals".to_owned(),
            Some(0) => "no rivals".to_owned(),
            Some(1) => "at most 1 rival".to_owned(),
            Some(limit) => format!("at most {limit} rivals"),
        };
        format!("{traffic}, {rivals}")
    }
}

/// The saved settings, or the original ones when there is no file yet.
pub fn load(data: &Path) -> Result<Settings> {
    let path = data.join(FILE);
    if std::fs::symlink_metadata(&path).is_err() {
        return Ok(Settings::default());
    }
    let settings: Settings = records::read_json(&path)?;
    settings.validate()?;
    Ok(settings)
}

/// Apply command-line changes to the saved settings and save the result. The
/// file is created on first use so the player can find and edit it.
pub fn resolve(data: &Path, traffic: Option<u16>, rivals: Option<Rivals>) -> Result<Settings> {
    let path = data.join(FILE);
    let exists = std::fs::symlink_metadata(&path).is_ok();
    let mut settings = load(data)?;
    if let Some(percent) = traffic {
        settings.traffic_percent = percent;
    }
    match rivals {
        Some(Rivals::Original) => settings.rival_limit = None,
        Some(Rivals::AtMost(limit)) => settings.rival_limit = Some(limit),
        None => {}
    }
    settings.validate()?;
    if !exists || traffic.is_some() || rivals.is_some() {
        records::replace_json(&path, &settings)?;
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn originals_need_no_server_options_and_others_are_passed() {
        let original = Settings::default();
        assert!(original.server_arguments().is_empty());
        assert_eq!(original.describe(), "original traffic, original rivals");
        let custom = Settings {
            traffic_percent: 150,
            rival_limit: Some(2),
        };
        assert_eq!(
            custom.server_arguments(),
            ["--traffic-percent", "150", "--rival-limit", "2"]
        );
        assert_eq!(custom.describe(), "150% traffic, at most 2 rivals");
        let off = Settings {
            traffic_percent: 0,
            rival_limit: Some(0),
        };
        assert_eq!(off.describe(), "no traffic, no rivals");
        assert_eq!(
            off.server_arguments(),
            ["--traffic-percent", "0", "--rival-limit", "0"]
        );
    }

    #[test]
    fn rivals_values_are_original_or_a_bounded_count() {
        assert_eq!(parse_rivals("original"), Ok(Rivals::Original));
        assert_eq!(parse_rivals("Original"), Ok(Rivals::Original));
        assert_eq!(parse_rivals("0"), Ok(Rivals::AtMost(0)));
        assert_eq!(parse_rivals("128"), Ok(Rivals::AtMost(128)));
        for bad in ["129", "-1", "", "lots"] {
            assert!(parse_rivals(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_file_is_created_updated_kept_and_validated() {
        let dir = crate::testing::scratch("settings");
        // First use writes the original settings for the player to edit.
        assert_eq!(resolve(&dir, None, None).unwrap(), Settings::default());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({"traffic_percent": 100, "rival_limit": null})
        );
        // Command-line values are saved and later sessions keep them.
        resolve(&dir, Some(40), Some(Rivals::AtMost(3))).unwrap();
        let kept = resolve(&dir, None, None).unwrap();
        assert_eq!((kept.traffic_percent, kept.rival_limit), (40, Some(3)));
        let reset = resolve(&dir, None, Some(Rivals::Original)).unwrap();
        assert_eq!((reset.traffic_percent, reset.rival_limit), (40, None));
        // A hand-edited file with missing fields uses the originals for them.
        std::fs::write(dir.join(FILE), br#"{"rival_limit": 1}"#).unwrap();
        assert_eq!(
            load(&dir).unwrap(),
            Settings {
                traffic_percent: 100,
                rival_limit: Some(1)
            }
        );
        // Out-of-range, unknown or malformed settings are refused, not ignored.
        for bad in [
            &br#"{"traffic_percent": 301}"#[..],
            br#"{"rival_limit": 129}"#,
            br#"{"traffic": 50}"#,
            br#"{"traffic_percent": -5}"#,
            b"not json",
        ] {
            std::fs::write(dir.join(FILE), bad).unwrap();
            assert!(load(&dir).is_err(), "{}", String::from_utf8_lossy(bad));
            assert!(resolve(&dir, Some(50), None).is_err());
        }
        assert!(resolve(&dir.join("missing"), Some(301), None).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
