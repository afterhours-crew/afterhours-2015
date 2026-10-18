// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Instance-aware interface fields and the supported scalar default grammar.
//! This preserves declarations; it does not choose aliases or initialization order.
use super::{events::Identity, field};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::ebx::{Object, Pointer, Value};
use nfs_world::scoring::{events::MAX_NODES, properties::MAX_PROPERTIES, value::Value as Scalar};
use std::collections::BTreeSet;

pub const MAX_DEFAULT_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Source,
    Target,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: u32,
    pub access: Access,
    /// Empty authored text means no default, not a zero or a ready value.
    pub default: Option<Scalar>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub identity: Identity,
    pub bus: Identity,
    /// Authored order, with signed IDs preserved as their original bits.
    pub fields: Vec<Field>,
}

fn default(value: &str) -> Result<Option<Scalar>, Error> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > MAX_DEFAULT_BYTES {
        return Err(bad("interface default bound"));
    }
    let (kind, text) = value
        .split_once(' ')
        .ok_or_else(|| bad("interface default type and value"))?;
    let scalar = match (kind, text) {
        ("Boolean", "False") => Scalar::Boolean(false),
        ("Boolean", "True") => Scalar::Boolean(true),
        ("Float32", text)
            if !text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b".+-eE".contains(&b)) =>
        {
            let value = text
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| bad("interface finite float default"))?;
            Scalar::Float(value)
        }
        _ => return Err(bad("unsupported interface default")),
    };
    Ok(Some(scalar))
}

/// Read the supported empty/Boolean/Float32 subset. Unknown types, reference
/// defaults, malformed enums and duplicate IDs fail rather than gaining a value.
pub fn compile(object: &Object) -> Result<Vec<Field>, Error> {
    if object.class != "InterfaceDescriptorData" {
        return Err(bad("scoring interface class"));
    }
    let rows = field(&object.fields, "Fields")?
        .as_array()
        .filter(|r| r.len() <= MAX_PROPERTIES)
        .ok_or_else(|| bad("interface fields bound"))?;
    let mut seen = BTreeSet::new();
    rows.iter()
        .map(|row| {
            let fields = row.as_fields().ok_or_else(|| bad("interface field"))?;
            let id = bits32(field(fields, "Id")?)?;
            if !seen.insert(id) {
                return Err(bad("duplicate interface field"));
            }
            let access = match field(fields, "AccessType")? {
                Value::Enum {
                    value: 0,
                    name: Some(name),
                } if name == "FieldAccessType_Source" => Access::Source,
                Value::Enum {
                    value: 1,
                    name: Some(name),
                } if name == "FieldAccessType_Target" => Access::Target,
                _ => return Err(bad("interface field access enum")),
            };
            if field(fields, "ValueRef")?.as_pointer() != Some(Pointer::Null) {
                return Err(bad("unsupported interface reference default"));
            }
            let Value::String(text) = field(fields, "Value")? else {
                return Err(bad("interface default text"));
            };
            Ok(Field {
                id,
                access,
                default: default(text)?,
            })
        })
        .collect()
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Interface>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("scoring blueprint name"));
    }
    let mut graph = Graph::new(source);
    let nodes = graph.read(Site::root(blueprint))?;
    let mut interfaces = Vec::new();
    let mut fields_count = 0usize;
    let mut buses = BTreeSet::new();
    for node in nodes.iter().filter(|n| n.site.key == n.site.bus) {
        if !buses.insert(node.site.clone()) || buses.len() > MAX_NODES {
            return Err(bad("scoring interface bus bound or duplicate"));
        }
        let pointer = field(&node.object.fields, "Interface")?
            .as_pointer()
            .ok_or_else(|| bad("interface pointer"))?;
        let Some(key) = graph.source.target(&node.site.key.0, pointer)? else {
            if !node.site.parents.is_empty() {
                return Err(bad("nested scoring bus has no interface"));
            }
            continue;
        };
        let site = Site {
            key,
            ..node.site.clone()
        };
        let fields = compile(&graph.object(&site)?)?;
        fields_count = fields_count
            .checked_add(fields.len())
            .filter(|&n| n <= MAX_PROPERTIES)
            .ok_or_else(|| bad("composition interface fields bound"))?;
        interfaces.push(Interface {
            identity: Identity::from(&site),
            bus: Identity::from(&node.site),
            fields,
        });
    }
    Ok(interfaces)
}
impl AssetIndex {
    pub fn scoring_interfaces(&mut self, blueprint: &str) -> Result<Vec<Interface>, Error> {
        read(&mut self.assets, blueprint)
    }
}

#[cfg(test)]
mod tests;
