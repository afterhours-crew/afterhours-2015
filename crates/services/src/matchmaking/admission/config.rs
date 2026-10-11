// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{AttributeRule, Config, Error};
use serde_json::Value;
impl Config {
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 16 * 1024 {
            return Err(Error::Config);
        }
        let v: Value = serde_json::from_slice(bytes).map_err(|_| Error::Config)?;
        let keys = [
            "version",
            "protocol_version",
            "game_rules",
            "ued_rule",
            "ued_threshold",
            "player_attributes",
            "default_role",
            "max_players",
            "player_count_threshold",
            "utilization_threshold",
            "ping_site_threshold",
            "desired_percent_full",
            "creation_settings",
            "duration_micros",
            "starting_decay_ages",
        ];
        let o = v.as_object().ok_or(Error::Config)?;
        if o.len() != keys.len() || !keys.iter().all(|k| o.contains_key(*k)) || v["version"] != 1 {
            return Err(Error::Config);
        }
        let text = |v: &Value| {
            v.as_str()
                .map(|s| s.as_bytes().to_vec())
                .ok_or(Error::Config)
        };
        let mut rules = Vec::new();
        for row in v["game_rules"].as_array().ok_or(Error::Config)? {
            let o = row.as_object().ok_or(Error::Config)?;
            if o.len() != 3
                || !["name", "threshold", "values"]
                    .iter()
                    .all(|k| o.contains_key(*k))
            {
                return Err(Error::Config);
            }
            rules.push(AttributeRule {
                name: text(&row["name"])?,
                threshold: text(&row["threshold"])?,
                values: row["values"]
                    .as_array()
                    .ok_or(Error::Config)?
                    .iter()
                    .map(text)
                    .collect::<Result<_, _>>()?,
            });
        }
        let mut attrs = Vec::new();
        for row in v["player_attributes"].as_array().ok_or(Error::Config)? {
            let pair = row.as_array().ok_or(Error::Config)?;
            if pair.len() != 2 {
                return Err(Error::Config);
            }
            attrs.push((text(&pair[0])?, text(&pair[1])?));
        }
        let unsigned = |key: &str| v[key].as_u64().ok_or(Error::Config);
        let config = Self {
            protocol_version: text(&v["protocol_version"])?,
            game_rules: rules,
            ued_rule: text(&v["ued_rule"])?,
            ued_threshold: text(&v["ued_threshold"])?,
            player_attributes: attrs,
            default_role: text(&v["default_role"])?,
            max_players: u16::try_from(unsigned("max_players")?).map_err(|_| Error::Config)?,
            player_count_threshold: text(&v["player_count_threshold"])?,
            utilization_threshold: text(&v["utilization_threshold"])?,
            ping_site_threshold: text(&v["ping_site_threshold"])?,
            desired_percent_full: u8::try_from(unsigned("desired_percent_full")?)
                .map_err(|_| Error::Config)?,
            creation_settings: u32::try_from(unsigned("creation_settings")?)
                .map_err(|_| Error::Config)?,
            duration_micros: v["duration_micros"].as_i64().ok_or(Error::Config)?,
            starting_decay_ages: v["starting_decay_ages"]
                .as_array()
                .ok_or(Error::Config)?
                .iter()
                .map(|v| v.as_i64().ok_or(Error::Config))
                .collect::<Result<_, _>>()?,
        };
        config.validate()?;
        Ok(config)
    }
    pub fn load(path: &std::path::Path) -> Result<Self, Error> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| Error::Config)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Config)?;
        Self::from_json(&bytes)
    }
}
