// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An initially empty owned set can stay empty only when its complete incoming
//! link graph is read-only. An unrecognized consumer is not evidence of that.
use super::*;
use crate::graph::{Endpoint as Link, Frame};

const MAX_CONSUMERS: usize = 4096;

pub(super) fn is_read_only<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<bool, Error> {
    if g.object(site)?.class != "ParticipantSetEntityData" {
        return Err(bad("owned membership class"));
    }
    if !g.input_ports(site)?.is_empty() || !g.link_ports(site)?.is_empty() {
        return Ok(false);
    }
    let bus = g.source.object(&site.bus)?;
    let events = bus
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= crate::scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("owned membership event table"))?;
    for event in events {
        let pointer = event
            .as_fields()
            .and_then(|f| f.get("Target"))
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("owned membership event target"))?;
        if g.source.target(&site.bus.0, pointer)?.as_ref() == Some(&site.key) {
            return Ok(false);
        }
    }
    known_consumers(g, site, &BTreeSet::new())
}

pub(super) fn known_consumers<S: Source>(
    g: &mut Graph<S>,
    site: &Site,
    accessors: &BTreeSet<Identity>,
) -> Result<bool, Error> {
    consumers(
        g,
        Link {
            site: site.clone(),
            port: 0,
        },
        &mut BTreeSet::new(),
        &mut 0,
        accessors,
    )
}

fn consumers<S: Source>(
    g: &mut Graph<S>,
    target: Link,
    active: &mut BTreeSet<Link>,
    steps: &mut usize,
    accessors: &BTreeSet<Identity>,
) -> Result<bool, Error> {
    *steps += 1;
    if *steps > MAX_CONSUMERS || active.len() >= MAX_DEPTH || !active.insert(target.clone()) {
        return Err(bad("owned membership consumer cycle or bound"));
    }
    let mut read_only = true;
    for mut source in g.link_sources_at(&target.site, target.port)? {
        *steps += 1;
        if *steps > MAX_CONSUMERS {
            return Err(bad("owned membership consumer count bound"));
        }
        let object = g.object(&source.site)?;
        match object.class.as_str() {
            "InterfaceDescriptorData" => {
                let Some(frame) = source.site.parents.pop() else {
                    read_only = false;
                    continue;
                };
                source.site.key = frame.reference;
                source.site.bus = frame.bus;
                read_only &= consumers(g, source, active, steps, accessors)?;
            }
            "ReferenceObjectData"
            | "LogicPrefabReferenceObjectData"
            | "SpatialPrefabReferenceObjectData"
            | "SubWorldReferenceObjectData" => {
                let child = if object.class == "SubWorldReferenceObjectData" {
                    (
                        object
                            .fields
                            .get("BundleName")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty() && s.len() <= 512)
                            .ok_or_else(|| bad("owned membership subworld name"))?
                            .to_lowercase(),
                        0,
                    )
                } else {
                    g.target_field(&source.site, "Blueprint")?
                };
                source.site.parents.push(Frame {
                    bus: source.site.bus.clone(),
                    reference: source.site.key.clone(),
                });
                source.site.key = child.clone();
                source.site.bus = child;
                source.site.key = g.target_field(&source.site, "Interface")?;
                read_only &= consumers(g, source, active, steps, accessors)?;
            }
            "ParticipantSetPredicateEntityData" => {
                let count = object
                    .fields
                    .get("NumberOfOperands")
                    .and_then(Value::bits32)
                    .filter(|n| {
                        *n > 0 && *n <= nfs_world::scoring::participant_sets::MAX_OPERANDS as u32
                    })
                    .ok_or_else(|| bad("owned membership consumer operand count"))?;
                read_only &= (0..count).any(|i| name(&i.to_string()) == source.port);
            }
            "ParticipantSetAccessorEntityData" => {
                read_only &= source.port == name("ParticipantSet")
                    && accessors.contains(&Identity::from(&source.site));
            }
            // Accessors and channels can mutate or expose the set. Keep its
            // runtime owner required, even if no current event appears to fire.
            _ => read_only = false,
        }
    }
    active.remove(&target);
    Ok(read_only)
}

#[cfg(test)]
mod tests;
