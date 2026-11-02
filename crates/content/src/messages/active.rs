// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unique invitations directly driven by an objective's Active property.
//! Restricted, delayed or event-controlled senders require their own models.
use super::*;
use std::collections::BTreeSet;

const SEND: u32 = 1_037_321_618;
const ACTIVE: u32 = 1_010_522_579;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invitation {
    pub objective: u64,
    pub hash: u32,
    pub manager: usize,
}

impl AssetIndex {
    /// Resolve every explicitly selected message asset, rejecting any sender
    /// whose trigger is more than the objective's direct Active property.
    pub fn active_objective_messages(
        &mut self,
        selected: &BTreeSet<String>,
    ) -> Result<Vec<Invitation>, Error> {
        if selected.is_empty()
            || selected.len() > 64
            || selected.iter().any(|s| s.is_empty() || s.len() > 512)
        {
            return Err(bad("active invitation selection bound"));
        }
        let rules = self.identity.progression.clone();
        let bindings = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == MANAGER)
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(&rules.main))?;
        let world = graph.read(Site::root(GAMEPLAY))?;
        read(
            &mut graph,
            &nodes,
            &world,
            &bindings,
            rules.ports["stable_id"],
            selected,
        )
    }
}

fn read<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    bindings: &BTreeMap<Key, usize>,
    stable: u32,
    selected: &BTreeSet<String>,
) -> Result<Vec<Invitation>, Error> {
    let mut found = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|n| n.object.class == "UniqueMessageSenderEntityData")
    {
        let asset = graph.target_field(&node.site, "MessageAsset")?;
        if !selected.contains(&asset.0) {
            continue;
        }
        if node.object.fields.get("HasMessageRestrictor") != Some(&Value::Bool(false))
            || node.object.fields.get("SendMessage") != Some(&Value::Bool(false))
            || graph.input_ports(&node.site)? != [SEND]
        {
            return Err(bad("active invitation has additional state"));
        }
        let input = graph.input(&node.site, SEND)?;
        if input.port != ACTIVE || graph.object(&input.site)?.class != "RaceObjectiveEntityData" {
            return Err(bad(
                "active invitation trigger is not direct objective activation",
            ));
        }
        // An event-driven trigger must not be reduced to its property input.
        let bus = graph.source.object(&node.site.bus)?;
        for row in bus
            .fields
            .get("EventConnections")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("active invitation event table"))?
        {
            let fields = row
                .as_fields()
                .ok_or_else(|| bad("active invitation event row"))?;
            let target = fields
                .get("Target")
                .and_then(Value::as_pointer)
                .ok_or_else(|| bad("active invitation event target"))?;
            if graph.source.target(&node.site.bus.0, target)?.as_ref() == Some(&node.site.key) {
                return Err(bad("active invitation has event input"));
            }
        }
        let message = graph.source.object(&asset)?;
        if message.class != "MessageAsset"
            || message.fields.get("Persisted") != Some(&Value::Bool(true))
        {
            return Err(bad("active invitation must be a durable message"));
        }
        let invitation = Invitation {
            objective: crate::chapter_notifications::stable(graph, stable, &input.site)?,
            hash: bits32(
                message
                    .fields
                    .get("HashId")
                    .ok_or_else(|| bad("active invitation hash"))?,
            )?,
            manager: sender_manager(graph, &node.site, world, bindings)?,
        };
        if found.insert(asset.0, invitation).is_some() {
            return Err(bad("active invitation has multiple senders"));
        }
    }
    if found.keys().cloned().collect::<BTreeSet<_>>() != *selected {
        return Err(bad("selected active invitation missing"));
    }
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests;
