// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Installed road-query thresholds, without a road database or startup policy.
use super::{events::Identity, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::road::Configuration;

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    if object.class != "RaceVehicleRoadDrivingQueryEntityData" {
        return Err(bad("unsupported road query class"));
    }
    let f = &object.fields;
    let configuration = Configuration {
        max_distance_to_segment: scalar(f, "MaxDistToSegment")?,
        speeding_min_time: scalar(f, "SpeedingMinTime")?,
        speeding_over_limit: scalar(f, "SpeedingOverLimit")?,
        excessive_speeding_over_limit: scalar(f, "ExcessiveSpeedingOverLimit")?,
        wrong_way_begin_distance: scalar(f, "WrongWayBeginDistance")?,
        wrong_way_end_distance: scalar(f, "WrongWayEndDistance")?,
        wrong_way_min_speed: scalar(f, "WrongWayMinSpeed")?,
        speeding_debounce_time: scalar(f, "SpeedingDebounceTime")?,
    };
    configuration
        .validate()
        .map_err(|_| bad("invalid road query thresholds"))?;
    Ok(configuration)
}
impl AssetIndex {
    pub fn scoring_roads(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|row| row.object.class == "RaceVehicleRoadDrivingQueryEntityData")
            .map(|row| {
                Ok(Node {
                    identity: Identity::from(&row.site),
                    configuration: compile(&row.object)?,
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
    fn every_threshold_is_required_typed_finite_and_nonnegative() {
        let mut object = Object {
            class: "RaceVehicleRoadDrivingQueryEntityData".into(),
            guid: None,
            fields: Fields(
                [
                    "MaxDistToSegment",
                    "SpeedingMinTime",
                    "SpeedingOverLimit",
                    "ExcessiveSpeedingOverLimit",
                    "WrongWayBeginDistance",
                    "WrongWayEndDistance",
                    "WrongWayMinSpeed",
                    "SpeedingDebounceTime",
                ]
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name.into(), Value::Float32(i as f32)))
                .collect(),
            ),
        };
        let c = compile(&object).unwrap();
        let declarations = crate::scoring::property_types::compile(&object)
            .unwrap()
            .unwrap();
        assert_eq!(
            declarations.inputs,
            [(
                nfs_world::scoring::road::SPEED_LIMIT_OVERRIDE,
                nfs_world::scoring::properties::Kind::Float
            )]
        );
        assert_eq!(
            declarations.outputs,
            [(
                nfs_world::scoring::road::SPEED_LIMIT,
                nfs_world::scoring::properties::Kind::Float
            )]
        );
        assert_eq!(
            [
                c.max_distance_to_segment,
                c.speeding_min_time,
                c.speeding_over_limit,
                c.excessive_speeding_over_limit,
                c.wrong_way_begin_distance,
                c.wrong_way_end_distance,
                c.wrong_way_min_speed,
                c.speeding_debounce_time
            ],
            [0., 1., 2., 3., 4., 5., 6., 7.]
        );
        for i in 0..8 {
            let saved = object.fields.0[i].1.clone();
            for bad in [
                Value::Float32(-1.),
                Value::Float32(f32::INFINITY),
                Value::Float32(f32::NAN),
                Value::Int32(1),
            ] {
                object.fields.0[i].1 = bad;
                assert!(compile(&object).is_err());
            }
            object.fields.0[i].1 = saved;
            let field = object.fields.0.remove(i);
            assert!(compile(&object).is_err());
            object.fields.0.insert(i, field);
        }
        object.class = "RaceVehicleCollisionQueryEntityData".into();
        assert!(compile(&object).is_err());
    }
}
