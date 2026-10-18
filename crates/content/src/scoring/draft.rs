// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Authored draft-query fallbacks. Connected ready properties override these.
use super::{events::Identity, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::draft::Configuration;
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    if object.class != "RaceVehicleDraftQueryEntityData" {
        return Err(bad("unsupported draft query class"));
    }
    let f = &object.fields;
    let c = Configuration {
        min_other_speed: scalar(f, "MinOtherSpeed")?,
        max_delta_speed: scalar(f, "MaxDeltaSpeed")?,
        cone_scale: scalar(f, "ConeScale")?,
        min_draft_time: scalar(f, "MinDraftTime")?,
        max_draft_time: scalar(f, "MaxDraftTime")?,
    };
    c.validate()
        .map_err(|_| bad("invalid draft query thresholds"))?;
    Ok(c)
}
impl AssetIndex {
    pub fn scoring_drafts(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|r| r.object.class == "RaceVehicleDraftQueryEntityData")
            .map(|r| {
                Ok(Node {
                    identity: Identity::from(&r.site),
                    configuration: compile(&r.object)?,
                })
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use nfs_frostbite::ebx::{Fields, Value};
    #[test]
    fn requires_typed_finite_thresholds_and_declares_actual_class() {
        let mut o = Object {
            class: "RaceVehicleDraftQueryEntityData".into(),
            guid: None,
            fields: Fields(
                [
                    ("MinOtherSpeed", 5.),
                    ("MaxDeltaSpeed", 25.),
                    ("ConeScale", 3.),
                    ("MinDraftTime", 0.1),
                    ("MaxDraftTime", 1.),
                ]
                .into_iter()
                .map(|(n, v)| (n.into(), Value::Float32(v)))
                .collect(),
            ),
        };
        assert_eq!(compile(&o).unwrap().cone_scale, 3.);
        let declarations = crate::scoring::property_types::compile(&o)
            .unwrap()
            .unwrap();
        assert_eq!(declarations.inputs.len(), 5);
        assert_eq!(declarations.outputs.len(), 3);
        for i in 0..5 {
            let saved = o.fields.0[i].1.clone();
            for bad in [
                Value::Bool(false),
                Value::Float32(f32::INFINITY),
                Value::Float32(-1.),
            ] {
                o.fields.0[i].1 = bad;
                assert!(compile(&o).is_err());
            }
            o.fields.0[i].1 = saved;
        }
        o.fields.0.pop();
        assert!(compile(&o).is_err());
    }
}
