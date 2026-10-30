// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored objective timers: Activated -> server world-time Delay -> Complete.
//! This reader covers explicitly requested objectives, not every completion in
//! a chapter. Race results and conditional event chains have different owners.
use crate::{Error, assets::AssetIndex, graph::*};
use nfs_frostbite::ebx::Value;
use std::{collections::BTreeSet, time::Duration};

const OBJECTIVE: &str = "RaceObjectiveEntityData";
const ACTIVATED: u32 = (-401_323_747i32) as u32;
const COMPLETE: u32 = 448_895_852;
const IN: u32 = nfs_world::scoring::nodes::IN;
const OUT: u32 = nfs_world::scoring::nodes::OUT;
const MAX_DELAYS: usize = 64;
const MAX_HOPS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Delay {
    pub objective: u64,
    pub duration: Duration,
}

impl AssetIndex {
    /// Resolve every requested timer or fail; unknown event logic is never
    /// converted to an immediate completion or a guessed default duration.
    pub fn objective_delays(
        &mut self,
        chapter: &str,
        objectives: &BTreeSet<u64>,
    ) -> Result<Vec<Delay>, Error> {
        let stable = self.identity.progression.ports["stable_id"];
        read(&mut self.assets, chapter, stable, objectives)
    }
}

/// Keep event delivery in the contract. Graph::event_sources intentionally
/// omits it for inventory callers, but a client-only edge cannot fire this timer.
fn server_source<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<(Site, u32), Error> {
    let rows = graph.object(&Site {
        key: site.bus.clone(),
        ..site.clone()
    })?;
    let rows = rows
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("objective delay event table"))?;
    if rows.len() > crate::scenes::MAX_GRAPH_OBJECTS {
        return Err(bad("objective delay event bound"));
    }
    let sources = graph.event_sources(site, port)?;
    let [source] = sources.as_slice() else {
        return Err(bad("objective delay input must have exactly one source"));
    };
    for row in rows {
        let row = row
            .as_fields()
            .ok_or_else(|| bad("objective delay event row"))?;
        let input = row
            .get("TargetEvent")
            .and_then(Value::as_fields)
            .and_then(|f| f.get("Id"))
            .and_then(Value::bits32);
        if input != Some(port) {
            continue;
        }
        let pointer = row
            .get("Target")
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("objective delay event target"))?;
        if graph.source.target(&site.bus.0, pointer)?.as_ref() != Some(&site.key) {
            continue;
        }
        if row.get("TargetType").and_then(Value::as_enum_name)
            != Some("EventConnectionTargetType_Server")
        {
            return Err(bad("objective delay needs a server event connection"));
        }
    }
    Ok(source.clone())
}

fn read<S: Source>(
    source: &mut S,
    chapter: &str,
    stable: u32,
    objectives: &BTreeSet<u64>,
) -> Result<Vec<Delay>, Error> {
    if objectives.is_empty() || objectives.len() > MAX_DELAYS || objectives.contains(&0) {
        return Err(bad("objective delay selection bound"));
    }
    let mut graph = Graph::new(source);
    let nodes = graph.read(Site::root(chapter))?;
    let mut found = BTreeSet::new();
    let mut out = Vec::new();
    for node in nodes.iter().filter(|n| n.object.class == OBJECTIVE) {
        let objective = crate::chapter_notifications::stable(&mut graph, stable, &node.site)?;
        if !objectives.contains(&objective) {
            continue;
        }
        if !found.insert(objective) {
            return Err(bad("duplicate objective delay identity"));
        }
        let (mut site, mut port) = server_source(&mut graph, &node.site, COMPLETE)?;
        let mut visited = BTreeSet::new();
        let mut seconds = 0f64;
        loop {
            if visited.len() >= MAX_HOPS || !visited.insert(site.clone()) || port != OUT {
                return Err(bad("objective delay chain bound or output"));
            }
            let object = graph.object(&site)?;
            let fields = &object.fields;
            if object.class != "DelayEntityData"
                || fields.get("Realm").and_then(Value::as_enum_name) != Some("Realm_Server")
                || fields.get("TimeDeltaType").and_then(Value::as_enum_name)
                    != Some("TimeDeltaType_World")
                || fields.get("AutoStart").and_then(Value::as_bool) != Some(false)
                || fields.get("RunOnce").and_then(Value::as_bool) != Some(false)
                || fields.get("RemoveDuplicateEvents").and_then(Value::as_bool) != Some(true)
                || !graph.input_ports(&site)?.is_empty()
            {
                return Err(bad("unsupported objective delay policy"));
            }
            let Some(Value::Float32(delay)) = fields.get("Delay") else {
                return Err(bad("objective delay duration"));
            };
            if !delay.is_finite() || !(0. ..=600.).contains(delay) {
                return Err(bad("objective delay duration bound"));
            }
            seconds += f64::from(*delay);
            // Reset/abort/other event inputs would change the lifetime. Do not
            // silently ignore them when reducing a graph to a fixed deadline.
            let bus = graph.object(&Site {
                key: site.bus.clone(),
                ..site.clone()
            })?;
            for row in bus
                .fields
                .get("EventConnections")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("objective delay event table"))?
            {
                let row = row
                    .as_fields()
                    .ok_or_else(|| bad("objective delay event row"))?;
                let pointer = row
                    .get("Target")
                    .and_then(Value::as_pointer)
                    .ok_or_else(|| bad("objective delay event target"))?;
                if graph.source.target(&site.bus.0, pointer)?.as_ref() == Some(&site.key)
                    && row
                        .get("TargetEvent")
                        .and_then(Value::as_fields)
                        .and_then(|f| f.get("Id"))
                        .and_then(Value::bits32)
                        != Some(IN)
                {
                    return Err(bad("unsupported objective delay control input"));
                }
            }
            (site, port) = server_source(&mut graph, &site, IN)?;
            if site == node.site && port == ACTIVATED {
                break;
            }
        }
        out.push(Delay {
            objective,
            duration: Duration::from_secs_f64(seconds),
        });
    }
    if &found != objectives {
        return Err(bad("requested objective timer missing"));
    }
    out.sort_by_key(|d| d.objective);
    Ok(out)
}

#[cfg(test)]
mod tests;
