// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    Error,
    scan::Assets,
    scenes::{MAX_GRAPH_DEPTH, MAX_GRAPH_OBJECTS},
};
use nfs_frostbite::ebx::{Object, Pointer, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type Key = (String, usize);
pub(crate) fn bad(message: &str) -> Error {
    Error::Content(format!("content graph: {message}"))
}
pub(crate) trait Source {
    fn object(&mut self, key: &Key) -> Result<Object, Error>;
    fn target(&mut self, source: &str, pointer: Pointer) -> Result<Option<Key>, Error>;
}
impl Source for Assets {
    fn object(&mut self, key: &Key) -> Result<Object, Error> {
        self.objects(&key.0)?
            .get(key.1)
            .cloned()
            .ok_or_else(|| bad("object index"))
    }
    fn target(&mut self, source: &str, pointer: Pointer) -> Result<Option<Key>, Error> {
        Assets::target(self, source, pointer)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct Frame {
    pub bus: Key,
    pub reference: Key,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct Site {
    pub key: Key,
    pub bus: Key,
    pub parents: Vec<Frame>,
}
impl Site {
    pub fn root(name: &str) -> Self {
        let key = (name.to_lowercase(), 0);
        Self {
            key: key.clone(),
            bus: key,
            parents: Vec::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub site: Site,
    pub object: Object,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct Endpoint {
    pub site: Site,
    pub port: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Connections {
    Link,
    Property,
}
#[derive(Clone)]
struct Edge {
    source: Key,
    source_port: u32,
    target: Key,
    target_port: u32,
}

pub(crate) struct Graph<'a, S: Source> {
    pub source: &'a mut S,
    edges: BTreeMap<(Key, Connections), Vec<Edge>>,
    edge_count: usize,
}
impl<'a, S: Source> Graph<'a, S> {
    pub fn new(source: &'a mut S) -> Self {
        Self {
            source,
            edges: BTreeMap::new(),
            edge_count: 0,
        }
    }
    pub fn object(&mut self, site: &Site) -> Result<Object, Error> {
        self.source.object(&site.key)
    }
    pub fn target_field(&mut self, site: &Site, field: &str) -> Result<Key, Error> {
        let object = self.object(site)?;
        let pointer = object
            .fields
            .get(field)
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("missing pointer field"))?;
        self.source
            .target(&site.key.0, pointer)?
            .ok_or_else(|| bad("null pointer field"))
    }
    pub fn read(&mut self, root: Site) -> Result<Vec<Node>, Error> {
        let mut rows = Vec::new();
        self.visit(root, &mut BTreeSet::new(), &mut rows)?;
        Ok(rows)
    }
    fn visit(
        &mut self,
        site: Site,
        active: &mut BTreeSet<Key>,
        rows: &mut Vec<Node>,
    ) -> Result<(), Error> {
        if active.len() >= MAX_GRAPH_DEPTH
            || rows.len() >= MAX_GRAPH_OBJECTS
            || site.parents.len() >= MAX_GRAPH_DEPTH
        {
            return Err(bad("composition bound"));
        }
        if !active.insert(site.key.clone()) {
            return Err(bad("composition cycle"));
        }
        let object = self.object(&site)?;
        // Follow the scene constructor's composition order. An editor Excluded
        // flag does not remove an object from these installed connection tables.
        rows.push(Node {
            site: site.clone(),
            object: object.clone(),
        });
        for field in ["Objects", "Object", "Components", "Blueprint"] {
            let Some(value) = object.fields.get(field) else {
                continue;
            };
            let pointers = match value {
                Value::Pointer(p) => vec![*p],
                Value::Array(a) if a.len() <= MAX_GRAPH_OBJECTS => a
                    .iter()
                    .map(|v| v.as_pointer().ok_or_else(|| bad("component pointer")))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(bad("composition edge")),
            };
            for p in pointers {
                let Some(key) = self.source.target(&site.key.0, p)? else {
                    continue;
                };
                let mut child = Site {
                    key: key.clone(),
                    ..site.clone()
                };
                // Layers share the enclosing scene's connection tables. Prefab
                // instances own separate buses, even when their asset repeats.
                if field == "Blueprint"
                    && object.class.ends_with("ReferenceObjectData")
                    && object.class != "WorldPartReferenceObjectData"
                {
                    child.parents.push(Frame {
                        bus: site.bus.clone(),
                        reference: site.key.clone(),
                    });
                    child.bus = key;
                }
                self.visit(child, active, rows)?;
            }
        }
        active.remove(&site.key);
        Ok(())
    }
    fn edges(&mut self, bus: &Key, kind: Connections) -> Result<&[Edge], Error> {
        let key = (bus.clone(), kind);
        if !self.edges.contains_key(&key) {
            let object = self.source.object(bus)?;
            let field = match kind {
                Connections::Link => "LinkConnections",
                Connections::Property => "PropertyConnections",
            };
            let rows = object
                .fields
                .get(field)
                .and_then(Value::as_array)
                .ok_or_else(|| bad("connection table missing"))?;
            self.edge_count = self
                .edge_count
                .checked_add(rows.len())
                .filter(|n| *n <= 500_000)
                .ok_or_else(|| bad("connection count bound"))?;
            let mut edges = Vec::with_capacity(rows.len());
            for row in rows {
                let fields = row.as_fields().ok_or_else(|| bad("connection row"))?;
                let mut pointer = |field: &str| -> Result<Key, Error> {
                    self.source
                        .target(
                            &bus.0,
                            fields
                                .get(field)
                                .and_then(Value::as_pointer)
                                .ok_or_else(|| bad("connection pointer"))?,
                        )?
                        .ok_or_else(|| bad("null connection"))
                };
                let source = pointer("Source")?;
                let target = pointer("Target")?;
                let port = |field: &str| -> Result<u32, Error> {
                    bits32(fields.get(field).ok_or_else(|| bad("connection port"))?)
                };
                edges.push(Edge {
                    source,
                    target,
                    source_port: port("SourceFieldId")?,
                    target_port: port("TargetFieldId")?,
                });
            }
            self.edges.insert(key.clone(), edges);
        }
        Ok(&self.edges[&key])
    }
    pub fn input_ports(&mut self, site: &Site) -> Result<Vec<u32>, Error> {
        Ok(self
            .edges(&site.bus, Connections::Property)?
            .iter()
            .filter(|e| e.target == site.key)
            .map(|e| e.target_port)
            .collect())
    }
    /// The immediate property sources of `site`'s field `port` in its own bus,
    /// without following prefab boundaries.
    pub fn property_sources(&mut self, site: &Site, port: u32) -> Result<Vec<Endpoint>, Error> {
        Ok(self
            .edges(&site.bus, Connections::Property)?
            .iter()
            .filter(|e| e.target == site.key && e.target_port == port)
            .map(|e| Endpoint {
                site: Site {
                    key: e.source.clone(),
                    ..site.clone()
                },
                port: e.source_port,
            })
            .collect())
    }
    /// Event connections of `site`'s bus that end at `site`'s event `event`, as
    /// (source site, source event).
    pub fn event_sources(&mut self, site: &Site, event: u32) -> Result<Vec<(Site, u32)>, Error> {
        let object = self.source.object(&site.bus)?;
        let rows = object
            .fields
            .get("EventConnections")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("event connection table missing"))?;
        if rows.len() > MAX_GRAPH_OBJECTS {
            return Err(bad("event connection count bound"));
        }
        let id = |fields: &nfs_frostbite::ebx::Fields, name: &str| -> Option<u32> {
            fields
                .get(name)
                .and_then(Value::as_fields)
                .and_then(|f| f.get("Id"))
                .and_then(Value::bits32)
        };
        let mut out = Vec::new();
        for row in rows {
            let fields = row.as_fields().ok_or_else(|| bad("event connection row"))?;
            if id(fields, "TargetEvent") != Some(event) {
                continue;
            }
            let pointer = |name: &str| {
                fields
                    .get(name)
                    .and_then(Value::as_pointer)
                    .ok_or_else(|| bad("event connection pointer"))
            };
            let target = self.source.target(&site.bus.0, pointer("Target")?)?;
            if target.as_ref() != Some(&site.key) {
                continue;
            }
            let source = self
                .source
                .target(&site.bus.0, pointer("Source")?)?
                .ok_or_else(|| bad("null event source"))?;
            let source_event = id(fields, "SourceEvent").ok_or_else(|| bad("event source id"))?;
            out.push((
                Site {
                    key: source,
                    ..site.clone()
                },
                source_event,
            ));
        }
        Ok(out)
    }
    /// Distinct ports of the link connections leaving `site` in its own bus.
    pub fn link_ports(&mut self, site: &Site) -> Result<Vec<u32>, Error> {
        let mut ports: Vec<u32> = self
            .edges(&site.bus, Connections::Link)?
            .iter()
            .filter(|e| e.source == site.key)
            .map(|e| e.source_port)
            .collect();
        ports.sort_unstable();
        ports.dedup();
        Ok(ports)
    }
    pub fn links(&mut self, site: &Site, port: u32) -> Result<Vec<Endpoint>, Error> {
        let mut out = Vec::new();
        self.resolve(
            Endpoint {
                site: site.clone(),
                port,
            },
            Connections::Link,
            &mut BTreeSet::new(),
            &mut out,
            &mut 0,
        )?;
        Ok(out)
    }
    pub fn input(&mut self, site: &Site, port: u32) -> Result<Endpoint, Error> {
        let mut out = Vec::new();
        self.resolve(
            Endpoint {
                site: site.clone(),
                port,
            },
            Connections::Property,
            &mut BTreeSet::new(),
            &mut out,
            &mut 0,
        )?;
        if out.len() != 1 {
            return Err(bad("ambiguous or missing property input"));
        }
        Ok(out.remove(0))
    }
    fn resolve(
        &mut self,
        query: Endpoint,
        kind: Connections,
        active: &mut BTreeSet<Endpoint>,
        out: &mut Vec<Endpoint>,
        steps: &mut usize,
    ) -> Result<(), Error> {
        *steps += 1;
        if *steps > MAX_GRAPH_OBJECTS
            || active.len() >= MAX_GRAPH_DEPTH
            || query.site.parents.len() >= MAX_GRAPH_DEPTH
            || out.len() >= MAX_GRAPH_OBJECTS
        {
            return Err(bad("connection traversal bound"));
        }
        if !active.insert(query.clone()) {
            return Err(bad("connection cycle"));
        }
        let edges: Vec<_> = self
            .edges(&query.site.bus, kind)?
            .iter()
            .filter(|e| match kind {
                Connections::Link => e.source == query.site.key && e.source_port == query.port,
                Connections::Property => e.target == query.site.key && e.target_port == query.port,
            })
            .cloned()
            .collect();
        if kind == Connections::Property && edges.len() != 1 {
            return Err(bad(&format!(
                "ambiguous or missing property edge at {}:{} field {} ({} sources)",
                query.site.key.0,
                query.site.key.1,
                query.port,
                edges.len()
            )));
        }
        for e in edges {
            *steps += 1;
            if *steps > MAX_GRAPH_OBJECTS || out.len() >= MAX_GRAPH_OBJECTS {
                return Err(bad("connection traversal bound"));
            }
            let (key, port) = match kind {
                Connections::Link => (e.target, e.target_port),
                Connections::Property => (e.source, e.source_port),
            };
            let mut next = Endpoint {
                site: Site {
                    key,
                    ..query.site.clone()
                },
                port,
            };
            let object = self.object(&next.site)?;
            if object.class.ends_with("ReferenceObjectData")
                && object.class != "WorldPartReferenceObjectData"
            {
                let child = if object.class == "SubWorldReferenceObjectData" {
                    (
                        object
                            .fields
                            .get("BundleName")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty() && s.len() <= 512)
                            .ok_or_else(|| bad("subworld name"))?
                            .to_lowercase(),
                        0,
                    )
                } else {
                    self.target_field(&next.site, "Blueprint")?
                };
                next.site.parents.push(Frame {
                    bus: query.site.bus.clone(),
                    reference: next.site.key.clone(),
                });
                next.site.bus = child.clone();
                next.site.key = child;
                next.site.key = self.target_field(&next.site, "Interface")?;
                self.resolve(next, kind, active, out, steps)?;
            } else if object.class == "InterfaceDescriptorData" {
                let frame = next
                    .site
                    .parents
                    .pop()
                    .ok_or_else(|| bad("unbound root interface"))?;
                next.site.bus = frame.bus;
                next.site.key = frame.reference;
                self.resolve(next, kind, active, out, steps)?;
            } else if kind == Connections::Property
                || port == 0
                || object.class == "LinkedSchematicChannelReferenceEntityData"
            {
                out.push(next);
            } else {
                return Err(bad("unsupported link endpoint port"));
            }
        }
        active.remove(&query);
        Ok(())
    }
}
pub(crate) fn bits32(value: &Value) -> Result<u32, Error> {
    match value {
        Value::Int32(n) => Ok(*n as u32),
        Value::UInt32(n) => Ok(*n),
        _ => Err(bad("expected 32-bit field")),
    }
}

#[cfg(test)]
#[path = "progression/graph_tests.rs"]
pub(crate) mod tests;

pub(crate) mod scenes;
