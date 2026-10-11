// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use serde_json::Value as Json;
use std::{fs::File, io::Read, path::Path};

const MAX_BYTES: usize = 128 * 1024;
#[derive(Clone, Debug)]
pub struct Award {
    pub id: u64,
    pub item_guid: String,
    pub kind: u32,
    pub unlock_value: u32,
}
#[derive(Clone, Debug)]
pub struct Rank {
    pub id: u32,
    pub unlock: u32,
}
#[derive(Clone, Debug)]
pub struct Definition {
    pub id: u32,
    pub car_id: u32,
    pub count_one: u32,
    pub count_two: u32,
    pub day: String,
    pub event_id: u32,
    pub kind: u32,
    pub weekly: bool,
    pub type_override_id: u32,
}
pub struct Catalog {
    pub(super) day_id: u32,
    pub(super) monthly_start: i64,
    pub(super) awards: Vec<(u32, Vec<Award>)>,
    pub(super) ranks: Vec<Rank>,
    pub(super) challenges: Vec<Definition>,
}
impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChallengeCatalog")
            .field("definitions", &self.challenges.len())
            .finish_non_exhaustive()
    }
}
fn fields<'a>(v: &'a Json, names: &[&str]) -> Result<&'a Json, ContentError> {
    let object = v.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(v)
}
fn unsigned(v: &Json) -> Result<u32, ContentError> {
    v.as_u64()
        .and_then(|n| n.try_into().ok())
        .ok_or(ContentError::Invalid)
}
fn string(v: &Json) -> Result<String, ContentError> {
    v.as_str()
        .filter(|s| s.len() < 128 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(ContentError::Invalid)
}
fn array(v: &Json) -> Result<&[Json], ContentError> {
    v.as_array()
        .filter(|v| v.len() <= 64)
        .map(Vec::as_slice)
        .ok_or(ContentError::Invalid)
}
impl Catalog {
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        let file = File::open(path).map_err(|_| ContentError::Io)?;
        if file.metadata().map_err(|_| ContentError::Io)?.len() > MAX_BYTES as u64 {
            return Err(ContentError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > MAX_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?)
    }
    /// Explicit local period plus definitions, never persona/progress/obtained flags
    /// or encoded request/response material. Current account data is joined later.
    pub fn from_json(value: &Json) -> Result<Self, ContentError> {
        let v = fields(
            value,
            &[
                "version",
                "build_sha256",
                "day_id",
                "monthly_start",
                "awards",
                "ranks",
                "challenges",
            ],
        )?;
        if v["version"] != 1 || v["build_sha256"] != SUPPORTED_BUILD_SHA256 {
            return Err(ContentError::Invalid);
        }
        let day_id = unsigned(&v["day_id"])?;
        let monthly_start = v["monthly_start"].as_i64().ok_or(ContentError::Invalid)?;
        let mut seen = BTreeSet::new();
        let mut awards = Vec::new();
        for entry in array(&v["awards"])? {
            fields(entry, &["challenge_id", "items"])?;
            let id = unsigned(&entry["challenge_id"])?;
            if !seen.insert(id) {
                return Err(ContentError::Invalid);
            }
            let mut ids = BTreeSet::new();
            let mut items = Vec::new();
            for item in array(&entry["items"])? {
                fields(item, &["id", "item_guid", "kind", "unlock_value"])?;
                let id = item["id"].as_u64().ok_or(ContentError::Invalid)?;
                if !ids.insert(id) {
                    return Err(ContentError::Invalid);
                }
                items.push(Award {
                    id,
                    item_guid: string(&item["item_guid"])?,
                    kind: unsigned(&item["kind"])?,
                    unlock_value: unsigned(&item["unlock_value"])?,
                });
            }
            awards.push((id, items));
        }
        seen.clear();
        let mut ranks = Vec::new();
        for r in array(&v["ranks"])? {
            fields(r, &["id", "unlock"])?;
            let id = unsigned(&r["id"])?;
            if !seen.insert(id) {
                return Err(ContentError::Invalid);
            }
            ranks.push(Rank {
                id,
                unlock: unsigned(&r["unlock"])?,
            });
        }
        seen.clear();
        let mut challenges = Vec::new();
        for c in array(&v["challenges"])? {
            fields(
                c,
                &[
                    "id",
                    "car_id",
                    "count_one",
                    "count_two",
                    "day",
                    "event_id",
                    "kind",
                    "weekly",
                    "type_override_id",
                ],
            )?;
            let id = unsigned(&c["id"])?;
            if !seen.insert(id) {
                return Err(ContentError::Invalid);
            }
            challenges.push(Definition {
                id,
                car_id: unsigned(&c["car_id"])?,
                count_one: unsigned(&c["count_one"])?,
                count_two: unsigned(&c["count_two"])?,
                day: string(&c["day"])?,
                event_id: unsigned(&c["event_id"])?,
                kind: unsigned(&c["kind"])?,
                weekly: c["weekly"].as_bool().ok_or(ContentError::Invalid)?,
                type_override_id: unsigned(&c["type_override_id"])?,
            });
        }
        let catalog = Self {
            day_id,
            monthly_start,
            awards,
            ranks,
            challenges,
        };
        catalog
            .body(&State::default(), 1)
            .map_err(|_| ContentError::Invalid)?;
        Ok(catalog)
    }
}
