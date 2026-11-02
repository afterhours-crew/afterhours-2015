// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Live membership bounds may use literals and unbound interface defaults.
//! The general startup evaluator also accepts stateful nodes; reject those
//! before using its result as an immutable range definition.
use super::*;

pub(super) fn no_events<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<(), Error> {
    let bus = g.source.object(&site.bus)?;
    let rows = bus
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|r| r.len() <= crate::scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("event range event table"))?;
    for row in rows {
        let target = row
            .as_fields()
            .and_then(|f| f.get("Target"))
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("event range event target"))?;
        if g.source.target(&site.bus.0, target)?.as_ref() == Some(&site.key) {
            return Err(bad("event range has event-written bounds"));
        }
    }
    Ok(())
}

pub(super) fn inputs<S: Source>(g: &mut Graph<S>, site: &Site, port: u32) -> Result<(), Error> {
    visit(g, site, port, &mut BTreeSet::new(), &mut 0)
}
fn visit<S: Source>(
    g: &mut Graph<S>,
    site: &Site,
    port: u32,
    active: &mut BTreeSet<(Site, u32)>,
    count: &mut usize,
) -> Result<(), Error> {
    *count += 1;
    if *count > 256 || active.len() >= 32 || !active.insert((site.clone(), port)) {
        return Err(bad("event range constant bound or cycle"));
    }
    let sources = g.property_sources(site, port)?;
    if sources.len() > 8 {
        return Err(bad("event range constant fan-in"));
    }
    for source in sources {
        match g.object(&source.site)?.class.as_str() {
            "IntEntityData" if source.port == name("Value") => {
                if !g.input_ports(&source.site)?.is_empty() {
                    return Err(bad("event range constant has inputs"));
                }
                no_events(g, &source.site)?;
            }
            "InterfaceDescriptorData" => {
                let mut parent = source.site;
                if let Some(frame) = parent.parents.pop() {
                    parent.key = frame.reference;
                    parent.bus = frame.bus;
                    visit(g, &parent, source.port, active, count)?;
                }
            }
            "LogicPrefabReferenceObjectData" | "SpatialPrefabReferenceObjectData" => {
                let child = g.target_field(&source.site, "Blueprint")?;
                let mut inner = source.site.clone();
                inner.parents.push(Frame {
                    bus: source.site.bus,
                    reference: source.site.key,
                });
                inner.bus = child.clone();
                inner.key = child;
                inner.key = g.target_field(&inner, "Interface")?;
                if g.property_sources(&inner, source.port)?.is_empty() {
                    return Err(bad("event range empty prefab output"));
                }
                visit(g, &inner, source.port, active, count)?;
            }
            _ => return Err(bad("event range bound is not constant")),
        }
    }
    active.remove(&(site.clone(), port));
    Ok(())
}
