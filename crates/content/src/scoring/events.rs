// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed event topology. This validates connections, not node behavior.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Frame, Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::ebx::{Fields, Value};
use nfs_world::scoring::events::{self, Connection, Delivery, Endpoint, Remote};
use nfs_world::scoring::properties::Port;
use std::collections::BTreeMap;

/// Scene topology includes linked shape data and client-only reference nodes.
/// Its inventory bound is independent of the executable per-graph node bound.
pub const MAX_SCENE_NODES: usize = crate::scenes::MAX_GRAPH_OBJECTS;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Identity {
    pub asset: String,
    pub index: usize,
    pub instances: Vec<(String, usize)>,
}
impl From<&Site> for Identity {
    fn from(site: &Site) -> Self {
        Self {
            asset: site.key.0.clone(),
            index: site.key.1,
            instances: site.parents.iter().map(|f| f.reference.clone()).collect(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub class: String,
    pub listeners: Option<super::realms::Listeners>,
    /// On receiving an event, emit the same event at this node in the adjacent
    /// prefab bus. None means the runtime must supply the class's behavior.
    pub forward: Option<u16>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Composition {
    /// Indices used by Connection endpoints; independent of network object IDs.
    pub nodes: Vec<Node>,
    /// Composition-order bus identities. Bind these to the replication profile
    /// before encoding Remote rows; these indices alone are not wire selectors.
    pub buses: Vec<Identity>,
    /// Per bus, its 1-based ordinal among the networked buses (blueprint
    /// `Flags` bit 0) in composition order, or None for a bus that is not
    /// networked. This is the entity index the client resolves in an entity
    /// event message (registry 86) for a node of that bus (official E914:
    /// style 3, speed 5, outlaw 12).
    pub bus_entities: Vec<Option<u16>>,
    pub connections: Vec<Connection>,
    /// Authored property rows, including client-side nodes. These are wiring,
    /// not listener registration order or proof that a node runs on the server.
    pub properties: Vec<DataConnection>,
    /// Object/provider links have distinct semantics from scalar properties.
    pub links: Vec<DataConnection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataConnection {
    pub source: Port,
    pub target: Port,
    pub bus: u16,
    pub index: u16,
    /// Full authored property flags, including currently uninterpreted bits.
    /// Provider links have no Flags field and therefore carry None. These are
    /// distinct from both entity listener realms and event TargetType.
    pub flags: Option<u32>,
}
impl Composition {
    pub fn dispatcher(&self) -> Result<events::Graph, Error> {
        events::Graph::new(self.nodes.len(), self.connections.clone())
            .map_err(|e| bad(&e.to_string()))
    }
}
fn field<'a>(fields: &'a Fields, name: &str) -> Result<&'a Value, Error> {
    fields.get(name).ok_or_else(|| bad("event field missing"))
}
fn event(fields: &Fields, name: &str) -> Result<u32, Error> {
    bits32(field(
        field(fields, name)?
            .as_fields()
            .ok_or_else(|| bad("event structure"))?,
        "Id",
    )?)
}
fn target_type(fields: &Fields) -> Result<i32, Error> {
    match field(fields, "TargetType")? {
        Value::Enum {
            value,
            name: Some(name),
        } if matches!(
            (*value, name.as_str()),
            (1, "EventConnectionTargetType_ClientAndServer")
                | (2, "EventConnectionTargetType_Client")
                | (3, "EventConnectionTargetType_Server")
                | (4, "EventConnectionTargetType_NetworkedClient")
                | (5, "EventConnectionTargetType_NetworkedClientAndServer")
        ) =>
        {
            Ok(*value)
        }
        _ => Err(bad("unsupported event target type")),
    }
}
fn insert<S: Source>(
    graph: &mut Graph<'_, S>,
    site: Site,
    sites: &mut BTreeMap<Site, u16>,
    nodes: &mut Vec<Node>,
    limit: usize,
) -> Result<u16, Error> {
    if let Some(&id) = sites.get(&site) {
        return Ok(id);
    }
    if nodes.len() >= limit {
        return Err(bad(&format!(
            "event node bound {limit} at {}:{}",
            site.key.0, site.key.1
        )));
    }
    let id = nodes.len() as u16;
    let object = graph.object(&site)?;
    nodes.push(Node {
        identity: Identity::from(&site),
        listeners: super::realms::Listeners::read(&object).map_err(|e| {
            bad(&format!(
                "{}:{} {}: {e}",
                site.key.0, site.key.1, object.class
            ))
        })?,
        class: object.class,
        forward: None,
    });
    sites.insert(site, id);
    Ok(id)
}

fn data_connections<S: Source>(
    graph: &mut Graph<'_, S>,
    sites: &BTreeMap<Site, u16>,
    bus: &Site,
    bus_index: u16,
    fields: &Fields,
    name: &str,
    budget: &mut ConnectionBudget,
) -> Result<Vec<DataConnection>, Error> {
    let rows = field(fields, name)?
        .as_array()
        .ok_or_else(|| bad("data connection array"))?;
    budget.visit(rows.len())?;
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let fields = row.as_fields().ok_or_else(|| bad("data connection row"))?;
            let mut endpoint = |side: &str, port: &str| -> Result<Port, Error> {
                let pointer = field(fields, side)?
                    .as_pointer()
                    .ok_or_else(|| bad("data pointer"))?;
                let key = graph
                    .source
                    .target(&bus.key.0, pointer)?
                    .ok_or_else(|| bad("null data endpoint"))?;
                let site = Site { key, ..bus.clone() };
                let &node = sites.get(&site).ok_or_else(|| {
                    bad(&format!(
                        "{name} row {index} {side} outside composition: {}:{} in {}:{}",
                        site.key.0, site.key.1, bus.key.0, bus.key.1
                    ))
                })?;
                Ok(Port {
                    node,
                    field: bits32(field(fields, port)?)?,
                })
            };
            Ok(DataConnection {
                source: endpoint("Source", "SourceFieldId")?,
                target: endpoint("Target", "TargetFieldId")?,
                bus: bus_index,
                index: index as u16,
                flags: if name == "PropertyConnections" {
                    Some(bits32(field(fields, "Flags")?)?)
                } else {
                    None
                },
            })
        })
        .collect()
}
struct ConnectionBudget {
    visited: usize,
    limit: usize,
}
impl ConnectionBudget {
    fn visit(&mut self, count: usize) -> Result<(), Error> {
        // Authored row indexes and bus indexes are transmitted as u16 values.
        // The total may span many buses; no individual array may wrap its index.
        if count > usize::from(u16::MAX) + 1 {
            return Err(bad("connection row index bound"));
        }
        self.visited = self
            .visited
            .checked_add(count)
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| bad(&format!("composition connection bound {}", self.limit)))?;
        Ok(())
    }
}
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Composition, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("scoring blueprint name"));
    }
    read_at(source, Site::root(blueprint))
}

/// A separately owned scene retains its enclosing subworld occurrence path.
/// Its root interface has no local parent; the scene owner binds that boundary.
pub(crate) fn read_at<S: Source>(source: &mut S, root: Site) -> Result<Composition, Error> {
    read_scoped(source, root, false)
}

/// Scene links can address data that is not constructed from Objects (for
/// example checkpoint OBBData). Expose those identities to the service owner,
/// without adding them to any constructor or activation list.
pub(crate) fn read_scene<S: Source>(source: &mut S, root: Site) -> Result<Composition, Error> {
    read_scoped(
        &mut crate::graph::scenes::ServerScene { source },
        root,
        true,
    )
}

fn read_scoped<S: Source>(source: &mut S, root: Site, scene: bool) -> Result<Composition, Error> {
    let limit = if scene {
        MAX_SCENE_NODES
    } else {
        events::MAX_NODES
    };
    let mut graph = Graph::new(source);
    let rows = graph.read(root.clone())?;
    let mut sites = BTreeMap::new();
    let mut nodes = Vec::new();
    let mut buses = Vec::new();
    for row in &rows {
        insert(&mut graph, row.site.clone(), &mut sites, &mut nodes, limit)?;
        if row.site.key == row.site.bus {
            if buses.contains(&row.site) {
                return Err(bad(&format!(
                    "duplicate event bus {}:{} ({:?}) in {}",
                    row.site.key.0, row.site.key.1, row.site.parents, root.key.0
                )));
            }
            buses.push(row.site.clone());
        }
    }
    // Interfaces are not necessarily in Objects. Register them explicitly and
    // retain the two directions of each prefab boundary as distinct endpoints.
    for bus in &buses {
        let object = graph.object(bus)?;
        let pointer = field(&object.fields, "Interface")?
            .as_pointer()
            .ok_or_else(|| bad("event interface pointer"))?;
        let Some(key) = graph.source.target(&bus.key.0, pointer)? else {
            // An absent authored interface supplies no forwarding endpoint.
            // Internal rows are still checked below; a row targeting a missing
            // interface cannot silently disappear from the graph.
            continue;
        };
        let interface = Site { key, ..bus.clone() };
        if graph.object(&interface)?.class != "InterfaceDescriptorData" {
            return Err(bad("event interface class"));
        }
        let interface_id = insert(&mut graph, interface, &mut sites, &mut nodes, limit)?;
        if *bus != root
            && let Some(Frame {
                bus: parent_bus,
                reference,
            }) = bus.parents.last()
        {
            let mut parents = bus.parents.clone();
            parents.pop();
            let parent = Site {
                key: reference.clone(),
                bus: parent_bus.clone(),
                parents,
            };
            let &parent_id = sites
                .get(&parent)
                .ok_or_else(|| bad("event prefab parent"))?;
            if nodes[parent_id as usize]
                .forward
                .replace(interface_id)
                .is_some()
            {
                return Err(bad("duplicate event prefab binding"));
            }
            nodes[interface_id as usize].forward = Some(parent_id);
        }
    }
    let mut connections = Vec::new();
    let mut properties = Vec::new();
    let mut links = Vec::new();
    let mut budget = ConnectionBudget {
        visited: 0,
        limit: if scene {
            events::MAX_TOPOLOGY_CONNECTIONS
        } else {
            events::MAX_CONNECTIONS
        },
    };
    for (bus_index, bus) in buses.iter().enumerate() {
        let object = graph.object(bus)?;
        if scene {
            let rows = field(&object.fields, "LinkConnections")?
                .as_array()
                .ok_or_else(|| bad("scene link array"))?;
            if rows.len() > usize::from(u16::MAX) + 1 {
                return Err(bad("scene link count bound"));
            }
            for row in rows {
                let fields = row.as_fields().ok_or_else(|| bad("scene link row"))?;
                for side in ["Source", "Target"] {
                    let pointer = field(fields, side)?
                        .as_pointer()
                        .ok_or_else(|| bad("scene link pointer"))?;
                    let key = graph
                        .source
                        .target(&bus.key.0, pointer)?
                        .ok_or_else(|| bad("null scene link"))?;
                    insert(
                        &mut graph,
                        Site { key, ..bus.clone() },
                        &mut sites,
                        &mut nodes,
                        limit,
                    )?;
                }
            }
        }
        properties.extend(data_connections(
            &mut graph,
            &sites,
            bus,
            bus_index as u16,
            &object.fields,
            "PropertyConnections",
            &mut budget,
        )?);
        links.extend(data_connections(
            &mut graph,
            &sites,
            bus,
            bus_index as u16,
            &object.fields,
            "LinkConnections",
            &mut budget,
        )?);
        let rows = field(&object.fields, "EventConnections")?
            .as_array()
            .ok_or_else(|| bad("event connection array"))?;
        budget.visit(rows.len())?;
        for (index, row) in rows.iter().enumerate() {
            let fields = row.as_fields().ok_or_else(|| bad("event connection row"))?;
            let kind = target_type(fields)?;
            let mut endpoint = |side: &str, port: &str| -> Result<Endpoint, Error> {
                let pointer = field(fields, side)?
                    .as_pointer()
                    .ok_or_else(|| bad("event pointer"))?;
                let key = graph
                    .source
                    .target(&bus.key.0, pointer)?
                    .ok_or_else(|| bad("null event endpoint"))?;
                let site = Site { key, ..bus.clone() };
                let &node = sites.get(&site).ok_or_else(|| {
                    bad(&format!(
                        "event row {index} {side} outside composition: {}:{} in {}:{}",
                        site.key.0, site.key.1, bus.key.0, bus.key.1
                    ))
                })?;
                Ok(Endpoint {
                    node,
                    event: event(fields, port)?,
                })
            };
            let source = endpoint("Source", "SourceEvent")?;
            let target = endpoint("Target", "TargetEvent")?;
            // Enumerate before filtering: native remote events name authored rows.
            let remote = Remote {
                bus: bus_index as u16,
                index: index as u16,
            };
            let delivery = match kind {
                1 | 3 => Delivery::Local,
                2 => continue,
                4 => Delivery::NetworkedClient(remote),
                5 => Delivery::NetworkedClientAndServer(remote),
                _ => unreachable!(),
            };
            connections.push(Connection {
                source,
                target,
                delivery,
            });
        }
    }
    let mut bus_entities = Vec::with_capacity(buses.len());
    let mut networked = 0u16;
    for bus in &buses {
        let flags = match field(&graph.object(bus)?.fields, "Flags")? {
            Value::UInt16(v) => u32::from(*v),
            other => bits32(other)?,
        };
        bus_entities.push(if flags & 1 != 0 {
            networked = networked
                .checked_add(1)
                .ok_or_else(|| bad("networked bus count"))?;
            Some(networked)
        } else {
            None
        });
    }
    let result = Composition {
        nodes,
        buses: buses.iter().map(Identity::from).collect(),
        bus_entities,
        connections,
        properties,
        links,
    };
    events::validate_connections(result.nodes.len(), &result.connections)
        .map_err(|e| bad(&e.to_string()))?;
    Ok(result)
}
impl AssetIndex {
    /// Read server event topology while preserving authored order and prefab
    /// boundaries. Does not validate node semantics or activate participants.
    pub fn scoring_events(&mut self, blueprint: &str) -> Result<Composition, Error> {
        read(&mut self.assets, blueprint)
    }
}

#[cfg(test)]
mod tests;
