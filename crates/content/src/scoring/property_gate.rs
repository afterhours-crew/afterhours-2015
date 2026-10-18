// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed property-gate defaults; output bindings are compiled separately.
use super::{boolean, events::Identity, field, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::{Fields, Object, Value};
use nfs_world::scoring::{property_gate::Definition, value::Value as GraphValue};

#[derive(Clone, Debug, PartialEq)]
pub struct Configuration {
    pub definition: Definition,
    pub defaults: [GraphValue; 6],
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}
fn vector<const N: usize>(value: &Value, names: [&str; N]) -> Result<[f32; N], Error> {
    let fields = value
        .as_fields()
        .ok_or_else(|| bad("scoring vector fields"))?;
    let mut values = [0.0; N];
    for (value, name) in values.iter_mut().zip(names) {
        *value = scalar(fields, name)?;
    }
    Ok(values)
}
fn transform(fields: &Fields) -> Result<GraphValue, Error> {
    let fields = field(fields, "TransformIn")?
        .as_fields()
        .ok_or_else(|| bad("scoring transform fields"))?;
    let mut values = [[0.0; 3]; 4];
    for (value, name) in values.iter_mut().zip(["right", "up", "forward", "trans"]) {
        *value = vector(field(fields, name)?, ["x", "y", "z"])?;
    }
    Ok(GraphValue::Transform(values))
}
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    if object.class != "PropertyGateEntityData" {
        return Err(bad("unsupported property gate class"));
    }
    let f = &object.fields;
    let integer = match field(f, "IntIn")? {
        Value::Int32(i) => *i,
        _ => return Err(bad("property gate integer")),
    };
    Ok(Configuration {
        definition: Definition {
            open: boolean(f, "Default")?,
            write_on_open: boolean(f, "WritePropertyOnOpenGate")?,
        },
        defaults: [
            GraphValue::Boolean(boolean(f, "BoolIn")?),
            GraphValue::Integer(integer),
            GraphValue::Float(scalar(f, "FloatIn")?),
            GraphValue::Vec3(vector(field(f, "Vec3In")?, ["x", "y", "z"])?),
            GraphValue::Vec4(vector(field(f, "Vec4In")?, ["x", "y", "z", "w"])?),
            transform(f)?,
        ],
    })
}
impl AssetIndex {
    pub fn scoring_property_gates(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|n| n.object.class == "PropertyGateEntityData")
            .map(|n| {
                Ok(Node {
                    identity: Identity::from(&n.site),
                    configuration: compile(&n.object)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
