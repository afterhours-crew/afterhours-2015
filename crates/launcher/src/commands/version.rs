// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `version` and `schema`: what this build provides and the JSON contract.

use crate::{
    contract::{self, Build, VersionReport},
    exit::{Failure, Result},
};

/// `version`: program identity, record versions and capabilities.
pub fn version(json: bool, build: &Build) -> Result<()> {
    let report = VersionReport::new(build);
    if json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    let records: Vec<String> = report
        .records
        .iter()
        .map(|(name, version)| format!("{name} {version}"))
        .collect();
    let capabilities: Vec<String> = report
        .capabilities
        .iter()
        .filter_map(|c| serde_json::to_value(c).ok())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    println!(
        "{} {} ({} / {})\nlauncher {} on {}{}\nJSON records: {}\ncapabilities: {}",
        report.program,
        report.program_version,
        report.release,
        report.revision.as_deref().unwrap_or("local build"),
        report.launcher_version,
        report.os,
        if report.game_launch {
            ", game launch supported"
        } else {
            ", server only"
        },
        records.join(", "),
        capabilities.join(", ")
    );
    Ok(())
}

/// `schema`: one record's JSON Schema, exactly as committed and released.
pub fn schema(name: &str) -> Result<()> {
    let text = contract::schema_text(name)
        .ok_or_else(|| Failure::refused(format!("No JSON record is named {name}.")))?;
    print!("{text}");
    Ok(())
}
