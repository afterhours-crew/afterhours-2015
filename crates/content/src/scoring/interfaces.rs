// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Instance-aware interface fields and the supported typed default grammar.
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
    /// Valid authored data whose type the runtime does not execute. Retain it;
    /// a property layout that uses this port must reject the unsupported type.
    pub opaque_default: Option<String>,
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
        ("CString", "\"\"") => {
            Scalar::Text(nfs_world::scoring::value::Text::new("").map_err(|e| bad(&e.to_string()))?)
        }
        ("LinearTransform", text) => {
            let inner = text
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .ok_or_else(|| bad("interface transform"))?;
            let rows = inner.split("),(").collect::<Vec<_>>();
            if rows.len() != 4 {
                return Err(bad("interface transform rows"));
            }
            let mut transform = [[0.0; 3]; 4];
            for (i, row) in rows.iter().enumerate() {
                let row = if i == 0 {
                    row.strip_prefix('(')
                        .ok_or_else(|| bad("interface transform start"))?
                } else {
                    row
                };
                let row = if i == 3 {
                    row.strip_suffix(')')
                        .ok_or_else(|| bad("interface transform end"))?
                } else {
                    row
                };
                transform[i] = components(row)?;
            }
            Scalar::Transform(transform)
        }
        ("Int32", text)
            if !text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b"+-".contains(&b)) =>
        {
            Scalar::Integer(
                text.parse()
                    .map_err(|_| bad("interface signed integer default"))?,
            )
        }
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

fn components<const N: usize>(text: &str) -> Result<[f32; N], Error> {
    let mut values = [0.0; N];
    let mut count = 0;
    for (i, part) in text.split(',').enumerate() {
        if i >= N
            || part.is_empty()
            || !part
                .bytes()
                .all(|b| b.is_ascii_digit() || b".+-eE".contains(&b))
        {
            return Err(bad("interface vector components"));
        }
        values[i] = part
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| bad("interface finite vector"))?;
        count += 1;
    }
    if count != N {
        return Err(bad("interface vector size"));
    }
    Ok(values)
}

/// Read supported scalar, empty-text and transform defaults. Valid Vec2 defaults
/// remain opaque and cannot feed an executed property cell. Unknown types,
/// references, malformed enums and duplicate IDs fail rather than gaining a value.
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
            let opaque_default = if let Some(vector) = text.strip_prefix("Vec2 ") {
                if text.len() > MAX_DEFAULT_BYTES {
                    return Err(bad("interface default bound"));
                }
                let inner = vector
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .ok_or_else(|| bad("interface Vec2"))?;
                components::<2>(inner)?;
                Some(text.clone())
            } else {
                None
            };
            Ok(Field {
                id,
                access,
                default: if opaque_default.is_some() {
                    None
                } else {
                    default(text)?
                },
                opaque_default,
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
