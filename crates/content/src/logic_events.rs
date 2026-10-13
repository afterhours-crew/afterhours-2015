// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed event bindings. The caller owns event timing and lifecycle policy.
use crate::{Error, assets::AssetIndex, graph::*};
use nfs_frostbite::ebx::Value;
use serde_json::{Value as Json, json};

const MAX_EVENTS: usize = 50_000;
const MAX_TOTAL_EVENTS: usize = 500_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Client,
    ClientAndServer,
}
/// An endpoint in the event's own bus occurrence. Asset/index are extracted
/// inspection identities; they are never live object IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub asset: String,
    pub index: usize,
    pub class: String,
    pub port: u32,
    /// Referenced prefab, when this endpoint is a prefab instance.
    pub blueprint: Option<String>,
    /// Named presentation context, when the endpoint declares one.
    pub presentation_key: Option<String>,
}
impl Endpoint {
    fn to_json(&self) -> Json {
        json!({"asset":self.asset,"index":self.index,"class":self.class,"port":self.port,
            "blueprint":self.blueprint,"presentation_key":self.presentation_key})
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    /// One-based registered-bus occurrence in composition order.
    pub bus: u32,
    /// Zero-based ordinal in the complete event table, including local rows.
    pub ordinal: u32,
    pub delivery: Delivery,
    pub source: Endpoint,
    pub target: Endpoint,
}
impl Event {
    pub fn to_json(&self) -> Json {
        json!({"bus":self.bus,"ordinal":self.ordinal,"delivery":match self.delivery {
            Delivery::Client=>"client",Delivery::ClientAndServer=>"client_and_server"},
            "source":self.source.to_json(),"target":self.target.to_json()})
    }
}
fn bus_class(class: &str) -> bool {
    class.ends_with("Blueprint") || matches!(class, "SubWorldData" | "LevelData")
}
fn delivery(v: &Value) -> Result<Option<Delivery>, Error> {
    match v {
        Value::Enum { value: 1, name }
            if name.as_deref() == Some("EventConnectionTargetType_ClientAndServer") =>
        {
            Ok(None)
        }
        Value::Enum { value: 2, name }
            if name.as_deref() == Some("EventConnectionTargetType_Client") =>
        {
            Ok(None)
        }
        Value::Enum { value: 3, name }
            if name.as_deref() == Some("EventConnectionTargetType_Server") =>
        {
            Ok(None)
        }
        Value::Enum { value: 4, name }
            if name.as_deref() == Some("EventConnectionTargetType_NetworkedClient") =>
        {
            Ok(Some(Delivery::Client))
        }
        Value::Enum { value: 5, name }
            if name.as_deref() == Some("EventConnectionTargetType_NetworkedClientAndServer") =>
        {
            Ok(Some(Delivery::ClientAndServer))
        }
        _ => Err(bad("unsupported event delivery type")),
    }
}
fn endpoint<S: Source>(
    graph: &mut Graph<S>,
    occurrences: &std::collections::BTreeMap<Site, usize>,
    bus: &Node,
    pointer: &Value,
    event: &Value,
) -> Result<Endpoint, Error> {
    let key = graph
        .source
        .target(
            &bus.site.key.0,
            pointer
                .as_pointer()
                .ok_or_else(|| bad("event endpoint pointer"))?,
        )?
        .ok_or_else(|| bad("null event endpoint"))?;
    let site = Site {
        key: key.clone(),
        ..bus.site.clone()
    };
    let object = graph.object(&site)?;
    if occurrences.get(&site).is_some_and(|count| *count != 1) {
        return Err(bad("ambiguous event endpoint occurrence"));
    }
    if !occurrences.contains_key(&site) {
        // Interface descriptors are referenced by the bus rather than included
        // in its Objects list. A foreign interface is not a valid endpoint.
        if object.class != "InterfaceDescriptorData"
            || graph.target_field(&bus.site, "Interface")? != key
        {
            return Err(bad("event endpoint outside bus composition"));
        }
    }
    let port = bits32(
        event
            .as_fields()
            .and_then(|f| f.get("Id"))
            .ok_or_else(|| bad("event port"))?,
    )?;
    let blueprint = if object.class.ends_with("ReferenceObjectData")
        && object.class != "WorldPartReferenceObjectData"
    {
        if object.class == "SubWorldReferenceObjectData" {
            Some(
                object
                    .fields
                    .get("BundleName")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty() && n.len() <= 512)
                    .ok_or_else(|| bad("event subworld name"))?
                    .to_lowercase(),
            )
        } else {
            Some(graph.target_field(&site, "Blueprint")?.0)
        }
    } else {
        None
    };
    let presentation_key = if object.class == "DirectPresentationContextEntityData" {
        Some(
            object
                .fields
                .get("Key")
                .and_then(Value::as_str)
                .filter(|k| !k.is_empty() && k.len() <= 128)
                .ok_or_else(|| bad("event presentation key"))?
                .into(),
        )
    } else {
        None
    };
    Ok(Endpoint {
        asset: key.0,
        index: key.1,
        class: object.class,
        port,
        blueprint,
        presentation_key,
    })
}
fn extract<S: Source>(graph: &mut Graph<S>, nodes: &[Node]) -> Result<Vec<Event>, Error> {
    if nodes.first().is_none_or(|n| !bus_class(&n.object.class)) {
        return Err(bad("event root class"));
    }
    let mut result = Vec::new();
    let mut occurrences = std::collections::BTreeMap::new();
    for node in nodes {
        *occurrences.entry(node.site.clone()).or_insert(0usize) += 1;
    }
    let mut bus_count = 0u32;
    let mut total = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for node in nodes.iter().filter(|n| bus_class(&n.object.class)) {
        if !seen.insert(node.site.clone()) {
            return Err(bad("duplicate event bus occurrence"));
        }
        let flags = match node.object.fields.get("Flags") {
            Some(Value::UInt16(n)) => u32::from(*n),
            Some(v) => bits32(v)?,
            None => return Err(bad("event bus flags")),
        };
        let registered = flags & 1 != 0;
        if registered {
            bus_count = bus_count
                .checked_add(1)
                .filter(|n| *n <= u16::MAX.into())
                .ok_or_else(|| bad("event bus count"))?;
        }
        let rows = node
            .object
            .fields
            .get("EventConnections")
            .and_then(Value::as_array)
            .filter(|r| r.len() <= MAX_EVENTS)
            .ok_or_else(|| bad("event table bound"))?;
        total = total
            .checked_add(rows.len())
            .filter(|n| *n <= MAX_TOTAL_EVENTS)
            .ok_or_else(|| bad("total event bound"))?;
        for (ordinal, row) in rows.iter().enumerate() {
            let fields = row.as_fields().ok_or_else(|| bad("event row"))?;
            let field = |k| fields.get(k).ok_or_else(|| bad("event row field"));
            let Some(delivery) = delivery(field("TargetType")?)? else {
                continue;
            };
            if !registered {
                return Err(bad("network event on unregistered bus"));
            }
            result.push(Event {
                bus: bus_count,
                ordinal: ordinal as u32,
                delivery,
                source: endpoint(
                    graph,
                    &occurrences,
                    node,
                    field("Source")?,
                    field("SourceEvent")?,
                )?,
                target: endpoint(
                    graph,
                    &occurrences,
                    node,
                    field("Target")?,
                    field("TargetEvent")?,
                )?,
            });
        }
    }
    if bus_count == 0 {
        return Err(bad("no registered event bus"));
    }
    Ok(result)
}
impl AssetIndex {
    /// Enumerate client-networked events in a scene or participant blueprint.
    /// Bus occurrences and unfiltered connection ordinals follow installed
    /// composition. This selects no events to fire and allocates no live IDs.
    pub fn network_events(&mut self, blueprint: &str) -> Result<Vec<Event>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("event root name"));
        }
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(blueprint))?;
        extract(&mut graph, &nodes)
    }
}

#[cfg(test)]
mod tests;
