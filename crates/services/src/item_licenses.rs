// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Item-system inventory assurance (`2052/19`, `ensurePlayerInventory`): the
//! client declares its item system and the licenses it holds; the service
//! acknowledges an exact, configured declaration with an empty response (no
//! new awards). A declaration of the configured item system without any
//! license (a new account holding only the base game) is acknowledged the same
//! way: it names nothing to award. Any other item system or license set is
//! unsupported (no reply): awarding items for licenses is inventory policy,
//! not this route.
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::items::{COMPONENT, ENSURE_PLAYER_INVENTORY, EnsurePlayerInventoryRequest};
use serde_json::Value as Json;
use std::{fs::File, io::Read, path::Path};

pub const MAX_CONTENT_BYTES: usize = 16 * 1024;
pub const MAX_LICENSES: usize = 32;
pub const MAX_SYSTEM_BYTES: usize = 256;
pub const MAX_LICENSE_BYTES: usize = 16;
pub const MAX_SOURCE_BYTES: usize = 32;
pub const MAX_BODY_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a complete, metadata-free category-0 `2052/19` request.
    Ineligible,
    /// Another item system or license declaration than the configured one.
    Unsupported,
    Encode,
}

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_BODY_BYTES,
        max_depth: 2,
        max_values: 4 + 3 * MAX_LICENSES,
        max_collection: MAX_LICENSES,
        max_byte_string: MAX_SYSTEM_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_BODY_BYTES + nfs_fire2::HEADER_LEN, 0, MAX_BODY_BYTES)
        .expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    (component, command) == (COMPONENT, ENSURE_PLAYER_INVENTORY)
}

/// One license and the source that granted it, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct License {
    pub license: String,
    pub source: String,
}

/// The configured item system and the exact license declaration it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    item_system: String,
    licenses: Vec<License>,
}

fn fields(v: &Json, names: &[&str]) -> Result<(), ContentError> {
    let object = v.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(())
}
fn text(v: &Json, max: usize) -> Result<String, ContentError> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= max && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(ContentError::Invalid)
}

impl Content {
    pub fn new(item_system: String, licenses: Vec<License>) -> Result<Self, ContentError> {
        if item_system.is_empty()
            || item_system.len() > MAX_SYSTEM_BYTES
            || item_system.contains('\0')
            || licenses.len() > MAX_LICENSES
            || licenses.iter().any(|l| {
                l.license.is_empty()
                    || l.license.len() > MAX_LICENSE_BYTES
                    || l.license.contains('\0')
                    || l.source.is_empty()
                    || l.source.len() > MAX_SOURCE_BYTES
                    || l.source.contains('\0')
            })
        {
            return Err(ContentError::Invalid);
        }
        Ok(Self {
            item_system,
            licenses,
        })
    }
    /// File I/O must run on a blocking worker at an async edge.
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take((MAX_CONTENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > MAX_CONTENT_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    pub fn from_json(v: &Json) -> Result<Self, ContentError> {
        fields(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "item_system",
                "licenses",
            ],
        )?;
        if v["format"] != "nfs-item-licenses"
            || v["version"] != 1
            || v["build_sha256"] != SUPPORTED_BUILD_SHA256
        {
            return Err(ContentError::Invalid);
        }
        let licenses = v["licenses"]
            .as_array()
            .filter(|a| a.len() <= MAX_LICENSES)
            .ok_or(ContentError::Invalid)?
            .iter()
            .map(|l| {
                fields(l, &["license", "source"])?;
                Ok(License {
                    license: text(&l["license"], MAX_LICENSE_BYTES)?,
                    source: text(&l["source"], MAX_SOURCE_BYTES)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(text(&v["item_system"], MAX_SYSTEM_BYTES)?, licenses)
    }
    pub fn item_system(&self) -> &str {
        &self.item_system
    }
    pub fn licenses(&self) -> &[License] {
        &self.licenses
    }

    /// Acknowledge the configured declaration with the observed empty
    /// response. `Unsupported` is "not mine": the caller must not fall back.
    pub fn reply(&self, wire: &[u8]) -> Result<Vec<u8>, Error> {
        let d = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = d.frame;
        if d.consumed != wire.len()
            || !owns(f.fields.routing_a, f.fields.routing_b)
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
        {
            return Err(Error::Ineligible);
        }
        let limits = body_limits();
        let q =
            EnsurePlayerInventoryRequest::decode(f.body, limits).map_err(|_| Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
        {
            return Err(Error::Ineligible);
        }
        if q.item_system_name != Some(self.item_system.as_bytes()) {
            return Err(Error::Unsupported);
        }
        // A declaration without licenses (list absent or empty) names nothing a
        // license could award, so it is acknowledged like the configured set.
        let declared = q.available_licenses.as_ref().map_or(&[][..], |d| &d.0[..]);
        let same = declared.is_empty()
            || (declared.len() == self.licenses.len()
                && declared.iter().zip(&self.licenses).all(|(row, l)| {
                    row.license == Some(l.license.as_bytes())
                        && row.source == Some(l.source.as_bytes())
                }));
        if !same {
            return Err(Error::Unsupported);
        }
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    category: 1,
                    ..f.fields
                },
                metadata: &[],
                body: &[],
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
}
