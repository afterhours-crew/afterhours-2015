// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Object links of a bound schematic channel. Keep the definition occurrence
//! and owning scene: the channel asset is a schema, not a runtime instance.
use super::*;
use crate::scoring::events::Identity;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Target {
    pub identity: Identity,
    pub class: String,
    pub port: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    pub reference: Identity,
    pub definition: Identity,
    pub asset: String,
    pub owner: Binding,
    /// Empty declared links remain empty; source order and repeats are kept.
    pub links: BTreeMap<u32, Vec<Target>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    pub asset: String,
    pub reference: bool,
    /// A relay obtains its definition through link 27802951, rather than the
    /// enclosing participant/event's channel-binding table.
    pub relay: bool,
    /// Object/provider link IDs are distinct from scalar property handles.
    pub links: Vec<u32>,
    /// Scalar property handles constructed by native 04050ae0/0404e2d0.
    /// The entity stores schema indices; execution uses the corresponding IDs.
    pub properties: Vec<u32>,
    pub reference_properties: Vec<u32>,
}

fn property_ids(object: &Object, schema: &Object, field: &str) -> Result<Vec<u32>, Error> {
    let properties = schema
        .fields
        .get("Properties")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("schematic channel property schema"))?;
    let indices = object
        .fields
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| bad("schematic channel property indices"))?;
    if properties.len() > 512 || indices.len() > 512 {
        return Err(bad("schematic channel property bound"));
    }
    let mut result = Vec::new();
    for index in indices {
        let row = properties
            .get(bits32(index)? as usize)
            .and_then(Value::as_fields)
            .ok_or_else(|| bad("schematic channel property index"))?;
        let realm = row
            .get("Realm")
            .ok_or_else(|| bad("schematic channel property realm"))?;
        // Server constructor 04050c37 omits Realm_Client (numeric zero).
        // Preserve the other schema rows; listener filtering is separate.
        match realm {
            Value::Enum {
                value: 0,
                name: Some(name),
            } if name == "Realm_Client" => continue,
            Value::Enum {
                value: 1,
                name: Some(name),
            } if name == "Realm_Server" => {}
            Value::Enum {
                value: 2,
                name: Some(name),
            } if name == "Realm_ClientAndServer" => {}
            Value::Enum {
                value: 3,
                name: Some(name),
            } if name == "Realm_None" => {}
            _ => return Err(bad("schematic channel property realm")),
        }
        let id = bits32(
            row.get("Id")
                .ok_or_else(|| bad("schematic channel property ID"))?,
        )?;
        if id == 0 {
            return Err(bad("schematic channel property ID"));
        }
        result.push(id);
    }
    Ok(result)
}

pub(super) fn definitions<S: Source>(
    source: &mut S,
    blueprint: &str,
) -> Result<BTreeMap<Identity, Definition>, Error> {
    definitions_at(source, Site::root(blueprint))
}

pub(super) fn definitions_at<S: Source>(
    source: &mut S,
    root: Site,
) -> Result<BTreeMap<Identity, Definition>, Error> {
    let mut graph = Graph::new(source);
    let mut result = BTreeMap::new();
    for node in graph.read(root)? {
        let (reference, relay, prefix) = match node.object.class.as_str() {
            "LinkedSchematicChannelEntityData" => (false, false, "Input"),
            "LinkedSchematicChannelReferenceEntityData" => (true, false, "Output"),
            "LinkedSchematicChannelRelayEntityData" => (true, true, "Output"),
            _ => continue,
        };
        let key = graph.target_field(&node.site, "ChannelAsset")?;
        let schema = graph.source.object(&key)?;
        if schema.class != "SchematicChannelAsset" {
            return Err(bad("schematic channel schema class"));
        }
        let definition = Definition {
            asset: key.0,
            reference,
            relay,
            links: link_ids(&schema)?,
            properties: property_ids(&node.object, &schema, &format!("{prefix}Properties"))?,
            reference_properties: property_ids(
                &node.object,
                &schema,
                &format!("{prefix}RefProperties"),
            )?,
        };
        if result.len() >= 256
            || result
                .insert(Identity::from(&node.site), definition)
                .is_some()
        {
            return Err(bad("schematic channel occurrence bound"));
        }
    }
    Ok(result)
}

fn link_ids(schema: &Object) -> Result<Vec<u32>, Error> {
    let rows = schema
        .fields
        .get("Links")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("schematic channel links"))?;
    if rows.len() > 512 {
        return Err(bad("schematic channel link bound"));
    }
    let mut ids = Vec::new();
    for row in rows {
        let id = bits32(
            row.as_fields()
                .and_then(|r| r.get("Id"))
                .ok_or_else(|| bad("schematic channel link ID"))?,
        )?;
        if id == 0 || ids.contains(&id) {
            return Err(bad("schematic channel link ID"));
        }
        ids.push(id);
    }
    Ok(ids)
}

pub(in crate::events) fn read<S: Source>(
    graph: &mut Graph<S>,
    channels: &BTreeMap<Site, (Site, Binding)>,
) -> Result<Vec<Channel>, Error> {
    let mut result = Vec::new();
    for (reference, (definition, owner)) in channels {
        if graph.object(reference)?.class != "LinkedSchematicChannelReferenceEntityData" {
            continue;
        }
        if graph.object(definition)?.class != "LinkedSchematicChannelEntityData" {
            return Err(bad("bound schematic channel definition class"));
        }
        let key = graph.target_field(reference, "ChannelAsset")?;
        if key != graph.target_field(definition, "ChannelAsset")? {
            return Err(bad("bound schematic channel schema mismatch"));
        }
        let object = graph.source.object(&key)?;
        if object.class != "SchematicChannelAsset" {
            return Err(bad("bound schematic channel schema class"));
        }
        let declarations = object
            .fields
            .get("Links")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("bound schematic channel links"))?;
        if declarations.len() > 512 || result.len() >= 256 {
            return Err(bad("bound schematic channel bound"));
        }
        let mut links = BTreeMap::new();
        for declaration in declarations {
            let port = declaration
                .as_fields()
                .and_then(|f| f.get("Id"))
                .ok_or_else(|| bad("bound schematic channel link declaration"))
                .and_then(bits32)?;
            if port == 0 || links.contains_key(&port) {
                return Err(bad("bound schematic channel duplicate or null link"));
            }
            let targets = graph
                .links(definition, port)?
                .into_iter()
                .map(|target| {
                    Ok(Target {
                        identity: Identity::from(&target.site),
                        class: graph.object(&target.site)?.class,
                        port: target.port,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            links.insert(port, targets);
        }
        result.push(Channel {
            reference: Identity::from(reference),
            definition: Identity::from(definition),
            asset: key.0,
            owner: *owner,
            links,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
