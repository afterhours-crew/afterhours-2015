// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::events::Identity;
use nfs_world::scoring::{events::Delivery, nodes as scalar};

/// The false side of the launcher's arrival latch selects its sequence and
/// teleport together, then sets the latch. Other selectors/setters in the same
/// capability belong to movies, retries or resume and must not be substituted.
pub(super) fn before_launcher<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
) -> Result<(Site, Site), Error> {
    let launcher = unique(nodes, "SimpleGameModeLauncherEntityData")?;
    let sources = graph.event_sources(launcher, 230_748_069)?;
    let [(condition, event)] = sources.as_slice() else {
        return Err(bad("arrival launcher condition"));
    };
    let object = graph.object(condition)?;
    if *event != scalar::ON_TRUE
        || object.class != "CompareBoolEntityData"
        || [
            "Bool",
            "TriggerOnPropertyChange",
            "TriggerOnStart",
            "AlwaysSend",
        ]
        .iter()
        .any(|f| object.fields.get(f).and_then(Value::as_bool) != Some(false))
    {
        return Err(bad("arrival condition policy"));
    }
    let properties = graph.property_sources(condition, scalar::BOOL)?;
    let [latch] = properties.as_slice() else {
        return Err(bad("arrival latch input"));
    };
    let object = graph.object(&latch.site)?;
    if latch.port != scalar::FLOAT_VALUE
        || object.class != "BoolEntityData"
        || object.fields.get("DefaultValue").and_then(Value::as_bool) != Some(false)
    {
        return Err(bad("arrival latch default"));
    }
    let composition = crate::scoring::events::read_at(
        graph.source,
        Site {
            key: condition.bus.clone(),
            ..condition.clone()
        },
    )?;
    let condition_id = composition
        .nodes
        .iter()
        .position(|n| n.identity == Identity::from(condition))
        .ok_or_else(|| bad("arrival condition occurrence"))?;
    let source_edges = composition
        .connections
        .iter()
        .filter(|e| usize::from(e.source.node) == condition_id)
        .collect::<Vec<_>>();
    if !source_edges.iter().any(|e| {
        e.source.event == scalar::ON_TRUE
            && e.target.event == 230_748_069
            && e.delivery == Delivery::Local
            && composition.nodes[usize::from(e.target.node)].identity == Identity::from(launcher)
    }) {
        return Err(bad("arrival launcher server edge"));
    }
    let mut selector = None;
    let mut setter = None;
    let mut set = false;
    for edge in source_edges
        .into_iter()
        .filter(|e| e.source.event == scalar::ON_FALSE)
    {
        if edge.delivery != Delivery::Local {
            return Err(bad("arrival server branch"));
        }
        let target = &composition.nodes[usize::from(edge.target.node)];
        let site = &nodes
            .iter()
            .find(|n| Identity::from(&n.site) == target.identity)
            .ok_or_else(|| bad("arrival branch occurrence"))?
            .site;
        match (target.class.as_str(), edge.target.event) {
            ("StateSequenceSelectorEntityData", 283_106_254) if selector.is_none() => {
                selector = Some(site.clone())
            }
            ("SetTeleportLocationEntityData", 135_864_506) if setter.is_none() => {
                setter = Some(site.clone())
            }
            ("ParticipantProviderToParticipantEventEntityData", scalar::IN) => {}
            ("BoolEntityData", scalar::SET_TRUE) if site == &latch.site && !set => set = true,
            _ => return Err(bad("arrival unsupported branch operation")),
        }
    }
    let (Some(selector), Some(setter), true) = (selector, setter, set) else {
        return Err(bad("arrival branch incomplete"));
    };
    if graph
        .object(&selector)?
        .fields
        .get("OverrideSequenceQueue")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(bad("arrival sequence queue policy"));
    }
    // Sequence selection and teleport use the same owned participant provider.
    // The launcher has a participant-set interface, resolved by mode preparation.
    let participant = (-1_232_400_965i32) as u32;
    let owner = graph.links(&selector, participant)?;
    if owner.len() != 1 || graph.links(&setter, participant)? != owner {
        return Err(bad("arrival participant mismatch"));
    }
    if owner[0].port != 0
        || graph.object(&owner[0].site)?.class != "ParticipantLogicParticipantProviderEntityData"
    {
        return Err(bad("arrival participant provider class"));
    }
    let referer = endpoint(
        graph,
        &selector,
        (-621_692_900i32) as u32,
        "StateSequenceRefererEntityData",
    )?;
    Ok((referer, setter))
}

#[cfg(test)]
mod tests;
