// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Validated static progression defaults and entity shapes.
pub(crate) use nfs_services::progression::MAX_EXPRESSIONS;
pub use nfs_services::progression::{
    Branch, Catalog, Definition, Expr, ObjectiveRow, ParseError, Settings,
};
use serde_json::Value as Json;
use std::{fs::File, io::Read, path::Path};
pub mod delays;
pub mod entities;
mod extract;
use crate::graph;
mod rules;
#[cfg(test)]
mod tests;
pub(crate) use extract::build;
pub use rules::Rules;
const BUILD: &str = nfs_services::SUPPORTED_BUILD_SHA256;
pub const FORMAT: &str = "nfs-progression-entities";
pub const VERSION: u8 = 1;
const MAX_BYTES: u64 = 512 * 1024;
const MAX_SOURCES: usize = 16;
fn fields(v: &Json, names: &[&str]) -> Result<(), ParseError> {
    let o = v.as_object().ok_or(ParseError::Invalid)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(ParseError::Invalid);
    }
    Ok(())
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Content {
    pub catalog: Catalog,
    pub construction: entities::Construction,
    pub settings: Settings,
    pub speed_list_bypass: bool,
}
impl Content {
    pub fn load(path: &Path) -> Result<Self, ParseError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ParseError::Io)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ParseError::Io)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(ParseError::Limit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ParseError::Invalid)?)
    }
    pub fn from_json(v: &Json) -> Result<Self, ParseError> {
        let mut names = vec![
            "format",
            "version",
            "build_sha256",
            "sources",
            "restoration",
            "entities",
            "settings",
        ];
        if v.get("story_editor").is_some() {
            names.push("story_editor");
        }
        fields(v, &names)?;
        if v["format"] != FORMAT || v["version"] != VERSION || v["build_sha256"] != BUILD {
            return Err(ParseError::Invalid);
        }
        let sources = v["sources"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= MAX_SOURCES)
            .ok_or(ParseError::Invalid)?;
        for source in sources {
            fields(source, &["path", "sha256"])?;
            if !source["path"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 512)
                || !source["sha256"]
                    .as_str()
                    .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(ParseError::Invalid);
            }
        }
        fields(&v["settings"], &["consumer_beta", "enable_speed_lists"])?;
        let setting = |name: &str| v["settings"][name].as_bool().ok_or(ParseError::Invalid);
        let catalog = Catalog::from_json(&v["restoration"])?;
        nfs_services::story::editing::Options::parse(
            v,
            &catalog,
            Settings {
                consumer_beta: setting("consumer_beta")?,
            },
        )
        .map_err(|_| ParseError::Invalid)?;
        let construction = entities::Construction::from_json(&v["entities"])?;
        for branch in [Branch::Main, Branch::Intro] {
            construction.check_objectives(branch, &catalog.objective_ids(branch))?;
        }
        Ok(Self {
            catalog,
            construction,
            settings: Settings {
                consumer_beta: setting("consumer_beta")?,
            },
            speed_list_bypass: setting("enable_speed_lists")?,
        })
    }
}
