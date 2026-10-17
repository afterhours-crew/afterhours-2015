// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Authored proximity-filter radius fallback (E916). A connected ready
//! property overrides it; the installed prefabs connect a Float constant.
use super::{events::Identity, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;

pub const CLASS: &str = "VehicleProximitySetFilterEntityData";

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub radius: f32,
}
pub fn compile(object: &Object) -> Result<f32, Error> {
    if object.class != CLASS {
        return Err(bad("unsupported proximity filter class"));
    }
    let radius = scalar(&object.fields, "Radius")?;
    if radius < 0.0 {
        return Err(bad("negative proximity radius"));
    }
    Ok(radius)
}
impl AssetIndex {
    pub fn scoring_proximities(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
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
                    radius: compile(&row.object)?,
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
    fn radius_is_a_finite_nonnegative_float_of_the_declared_class() {
        let mut object = Object {
            class: CLASS.into(),
            guid: None,
            fields: Fields(vec![("Radius".into(), Value::Float32(0.0))]),
        };
        assert_eq!(compile(&object).unwrap(), 0.0);
        object.fields.0[0].1 = Value::Float32(-1.0);
        assert!(compile(&object).is_err());
        object.fields.0[0].1 = Value::Int32(3);
        assert!(compile(&object).is_err());
        object.fields.0[0].1 = Value::Float32(30.0);
        object.class = "ParticipantSetCounterEntityData".into();
        assert!(compile(&object).is_err());
    }
}
