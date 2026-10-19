// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Traffic composition without player customization or inventory requirements.
use super::{ParseError, defaults, extract, fields::bad, parse::*};
use crate::{Error, assets::AssetIndex};
use nfs_world::replication::vehicle::{Kind, Profile, health, placement, root};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Definition {
    pub blueprint: String,
    pub bundle: String,
    pub asset_type: u16,
    pub asset_index: u16,
    pub catalog: Vec<(u16, u16)>,
    pub profile: Profile,
    pub root: root::Blueprint,
    pub resting: placement::RestingConfig,
    pub team: u8,
    pub mode: u8,
    pub parts: BTreeMap<usize, bool>,
    pub health: health::State,
}

impl Definition {
    pub fn from_json(v: &Value) -> Result<Self, ParseError> {
        keys(
            v,
            &[
                "blueprint",
                "bundle",
                "asset",
                "catalog",
                "profile",
                "parts",
                "buses",
                "resting",
                "team",
                "mode",
                "health",
            ],
            &[],
        )?;
        let profile = profile(&v["profile"])?;
        // Fail closed for other vehicle families; a missing component must not
        // silently change the subsequent component bit boundaries.
        for kind in [
            Kind::Chassis,
            Kind::Bool,
            Kind::Index,
            Kind::Rpc,
            Kind::GuidFloat,
            Kind::FourBit,
        ] {
            if profile.kinds().iter().filter(|k| **k == kind).count() != 1 {
                return Err(ParseError::Invalid);
            }
        }
        if profile
            .kinds()
            .iter()
            .filter(|k| **k == Kind::Wheel)
            .count()
            != 4
            || profile.kinds().iter().any(|k| {
                !matches!(
                    k,
                    Kind::Root { .. }
                        | Kind::Chassis
                        | Kind::Part { .. }
                        | Kind::Bool
                        | Kind::Wheel
                        | Kind::Index
                        | Kind::FourBit
                        | Kind::Rpc
                        | Kind::GuidFloat
                )
            })
        {
            return Err(ParseError::Invalid);
        }
        let parts = parts(v, &profile)?;
        let root = buses(v)?;
        let resting = resting(&v["resting"])?;
        model(resting.position_at([0.; 3]))?;
        let team = uint(&v["team"])?;
        let mode = uint(&v["mode"])?;
        if team > 16 || mode > 7 {
            return Err(ParseError::Invalid);
        }
        let Asset {
            asset_type,
            asset_index,
            catalog,
        } = asset(v)?;
        Ok(Self {
            blueprint: text(&v["blueprint"])?.to_owned(),
            bundle: text(&v["bundle"])?.to_owned(),
            asset_type,
            asset_index,
            catalog,
            profile,
            root,
            resting,
            team,
            mode,
            parts,
            health: health(&v["health"])?,
        })
    }
}

/// Resolve a requested traffic bundle through the installed blueprint and
/// registry. The caller chooses which registered traffic bundle to instantiate;
/// this function neither spawns vehicles nor chooses simulation ownership.
pub fn extract(index: &mut AssetIndex, bundle: &str) -> Result<Value, Error> {
    if bundle.is_empty() || bundle.len() > 512 || bundle.contains('\0') {
        return Err(bad("traffic bundle name"));
    }
    let rules = index.identity.vehicles.clone();
    let (root, blueprint, nodes) = extract::composition(index, bundle, &rules)?;
    let fields = extract::fields(index, bundle, &root, &blueprint, &nodes, &[], &rules)?;
    let mut row = defaults::common(&mut index.assets, &nodes)?;
    row.as_object_mut()
        .expect("defaults object")
        .extend(fields.as_object().expect("composition object").clone());
    Definition::from_json(&row)
        .map_err(|e| bad(&format!("invalid traffic profile {bundle}: {e:?}")))?;
    Ok(row)
}
