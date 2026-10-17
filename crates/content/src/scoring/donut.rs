// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Installed doughnut-query thresholds (E916).
use super::{boolean, events::Identity, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::donut::Configuration;

pub const CLASS: &str = "RaceVehicleDonutQueryEntityData";

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    if object.class != CLASS {
        return Err(bad("unsupported donut query class"));
    }
    let f = &object.fields;
    let configuration = Configuration {
        min_linear_velocity: scalar(f, "MinLinearVelocity")?,
        max_linear_velocity: scalar(f, "MaxLinearVelocity")?,
        donut_scale_threshold: scalar(f, "DonutScaleThreshold")?,
        time_in_donut_threshold: scalar(f, "TimeInDonutThreshold")?,
        allow_donuts_in_reverse: boolean(f, "AllowDonutsInReverse")?,
    };
    configuration
        .validate()
        .map_err(|_| bad("invalid donut query thresholds"))?;
    Ok(configuration)
}
impl AssetIndex {
    pub fn scoring_donuts(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|row| row.object.class == CLASS)
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
    fn thresholds_are_required_typed_and_consistent() {
        let mut object = Object {
            class: CLASS.into(),
            guid: None,
            fields: Fields(vec![
                ("MinLinearVelocity".into(), Value::Float32(5.0)),
                ("MaxLinearVelocity".into(), Value::Float32(35.0)),
                ("DonutScaleThreshold".into(), Value::Float32(0.7)),
                ("TimeInDonutThreshold".into(), Value::Float32(0.0)),
                ("AllowDonutsInReverse".into(), Value::Bool(false)),
            ]),
        };
        let configuration = compile(&object).unwrap();
        assert_eq!(configuration.max_linear_velocity, 35.0);
        assert!(!configuration.allow_donuts_in_reverse);
        object.fields.0[1].1 = Value::Float32(1.0);
        assert!(compile(&object).is_err());
        object.fields.0[1].1 = Value::Float32(35.0);
        object.fields.0.pop();
        assert!(compile(&object).is_err());
        object.class = "RaceVehicleNosQueryEntityData".into();
        assert!(compile(&object).is_err());
    }
}
