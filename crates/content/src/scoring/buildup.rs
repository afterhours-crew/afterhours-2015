// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Authored fallbacks for the six nitrous buildup inputs.
use super::{boolean, events::Identity, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::{buildup, value::Value};
pub const CLASS: &str = "RaceVehicleNosBuildupEntityData";
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub defaults: Vec<(u32, Value)>,
}
pub fn compile(object: &Object) -> Result<Vec<(u32, Value)>, Error> {
    if object.class != CLASS {
        return Err(bad("nitrous buildup class"));
    }
    let f = &object.fields;
    Ok(vec![
        (buildup::DRIFTING, Value::Boolean(boolean(f, "Drifting")?)),
        (
            buildup::DRAFTING,
            Value::Boolean(boolean(f, "Slipstreaming")?),
        ),
        (
            buildup::ONCOMING,
            Value::Boolean(boolean(f, "OncomingLane")?),
        ),
        (buildup::AIRBORNE, Value::Boolean(boolean(f, "Airborne")?)),
        (buildup::SPEED, Value::Float(scalar(f, "Speed")?)),
        (buildup::PACING, Value::Float(scalar(f, "Pacing")?)),
    ])
}
impl AssetIndex {
    pub fn scoring_buildups(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|r| r.object.class == CLASS)
            .map(|r| {
                Ok(Node {
                    identity: Identity::from(&r.site),
                    defaults: compile(&r.object)?,
                })
            })
            .collect()
    }
}
