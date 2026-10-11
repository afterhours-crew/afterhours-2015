// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed item inputs to vehicle construction. These contain definitions,
//! never the player's selected parts, paint, tuning settings or wrap uploads.
use nfs_world::items::Catalog as ItemCatalog;
use nfs_world::replication::vehicle::{
    appearance, customization, mesh, nos, tuning, wheel_customization,
};
use serde_json::Value;
use std::{io::Read, path::Path};

mod extract;
mod parse;
pub(crate) use extract::build;

pub const FORMAT: &str = "nfs-vehicle-parts";
pub const VERSION: u8 = 1;
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    Invalid,
    Limit,
}

/// The shared consuming models, also used to validate builder output.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    classes: ItemCatalog,
    pub customization: customization::Definitions,
    pub nos: nos::Definitions,
    pub wheels: wheel_customization::Definitions,
    pub appearance: appearance::Definitions,
    pub meshes: mesh::Definitions,
}

impl Catalog {
    pub fn load(path: &Path, classes: &ItemCatalog) -> Result<Self, crate::Error> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(crate::Error::Content(
                "vehicle parts exceed byte bound".into(),
            ));
        }
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| crate::Error::Content("vehicle parts JSON".into()))?;
        Self::from_json(&value, classes)
            .map_err(|e| crate::Error::Content(format!("vehicle parts rejected: {e:?}")))
    }

    pub fn from_json(value: &Value, classes: &ItemCatalog) -> Result<Self, ParseError> {
        parse::keys(value, &["format", "version", "build_sha256", "parts"], &[])?;
        if value["format"] != FORMAT
            || value["version"] != VERSION
            || value["build_sha256"] != nfs_services::SUPPORTED_BUILD_SHA256
        {
            return Err(ParseError::Invalid);
        }
        Self::from_rows(&value["parts"], classes)
    }

    /// Accept the embedded section of legacy vehicle-content documents too.
    pub fn from_rows(parts: &Value, classes: &ItemCatalog) -> Result<Self, ParseError> {
        parse::keys(
            parts,
            &["customization", "nos", "wheels", "appearance", "mesh"],
            &[],
        )?;
        let (customization, nos, wheels, appearance, meshes) = parse::parse(parts, classes)?;
        Ok(Self {
            classes: classes.clone(),
            customization,
            nos,
            wheels,
            appearance,
            meshes,
        })
    }
    pub fn matches_classes(&self, classes: &ItemCatalog) -> bool {
        &self.classes == classes
    }
}

#[cfg(test)]
mod tests;
