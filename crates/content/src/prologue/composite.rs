// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Final garage presentation selected by the return capability. Completion is
//! a separate client EventSync on that capability, after the sequence has left.
use super::*;
use crate::{graph::scenes::SceneLookup, scoring::events::Identity};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Composite {
    pub objective: u64,
    pub sequence: movies::Sequence,
    pub completion: nfs_world::scoring::event_sync::Binding,
}

fn completion(
    composition: &crate::scoring::events::Composition,
    signals: &[nfs_world::scoring::event_sync::Binding],
    objective: &Identity,
) -> Result<nfs_world::scoring::event_sync::Binding, Error> {
    use nfs_world::scoring::{events::Delivery, nodes::OUT};
    let mut found = vec![];
    for signal in signals {
        let mut pending = vec![(signal.node, OUT)];
        let mut seen = std::collections::BTreeSet::new();
        let mut reaches = false;
        while let Some((node, event)) = pending.pop() {
            if !seen.insert((node, event)) {
                continue;
            }
            if seen.len() > 512 {
                return Err(bad("composite completion path bound"));
            }
            for e in composition.connections.iter().filter(|e| {
                e.source.node == node && e.source.event == event && e.delivery == Delivery::Local
            }) {
                let target = &composition.nodes[usize::from(e.target.node)];
                if target.identity == *objective && e.target.event == 448_895_852 {
                    reaches = true;
                } else if let Some(forward) = target.forward {
                    pending.push((forward, e.target.event));
                }
            }
        }
        if reaches {
            found.push(*signal);
        }
    }
    match found.as_slice() {
        [signal] => Ok(*signal),
        _ => Err(bad("composite objective completion signal")),
    }
}

pub fn extract(index: &mut AssetIndex, capability: &str) -> Result<Composite, Error> {
    let progression = index.identity.progression.clone();
    let rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&rules.level)?;
    let mut graph = Graph::new(&mut index.assets);
    let mut scenes = SceneLookup {
        roots: scene_roots(&mut graph, &rules.level, &startup)?,
        rules: &rules,
        endpoints: BTreeMap::new(),
        loaded: Default::default(),
    };
    let scene = scenes
        .roots
        .get(&progression.scene)
        .ok_or_else(|| bad("composite progression scene"))?
        .clone();
    let (branch, aliases) =
        crate::events::intro::participant_context(&mut graph, scene, &progression)?;
    let roots: Vec<_> = branch
        .iter()
        .filter(|n| n.site.key == (capability.into(), 0))
        .collect();
    let [root] = roots.as_slice() else {
        return Err(bad("composite capability occurrence"));
    };
    let nodes = graph.read(root.site.clone())?;
    let roots: Vec<_> = nodes
        .iter()
        .filter(|n| n.site.key == ("gameplay/progression/compositevideomission".into(), 0))
        .collect();
    let [composite] = roots.as_slice() else {
        return Err(bad("composite mission occurrence"));
    };
    let composite_nodes = graph.read(composite.site.clone())?;
    let selector = unique(&composite_nodes, "StateSequenceSelectorEntityData")?;
    let provider = movies::bound(
        &mut graph,
        selector,
        3_062_566_331,
        &aliases,
        "ParticipantLogicParticipantProviderEntityData",
    )?;
    if provider != *unique(&branch, "ParticipantLogicParticipantProviderEntityData")? {
        return Err(bad("composite participant provider"));
    }
    let referer = movies::bound(
        &mut graph,
        selector,
        3_673_274_396,
        &aliases,
        "StateSequenceRefererEntityData",
    )?;
    let sequence = graph.target_field(&referer, "Sequence")?;
    exported(&mut graph, &sequence, "ParticipantStateSequenceBlueprint")?;
    let provider = movies::bound(
        &mut graph,
        selector,
        2_481_233_868,
        &aliases,
        "StateSequenceProviderEntityData",
    )?;
    let state = movies::sequence_state(&mut graph, &provider)?;
    let objective = unique(&nodes, "RaceObjectiveEntityData")?;
    let stable = endpoint(
        &mut graph,
        objective,
        progression.ports["stable_id"],
        "StableUniqueIdEntityData",
    )?;
    let id = match graph.object(&stable)?.fields.get("StableUniqueId64") {
        Some(Value::UInt64(n)) if *n != 0 => *n,
        Some(Value::Int64(n)) if *n != 0 => *n as u64,
        _ => return Err(bad("composite objective ID")),
    };
    let data = movies::Sequence {
        blueprint: sequence.0,
        reference: Identity::from(&referer),
        state: scenes.endpoint(&mut graph, &state)?,
    };
    let objective = Identity::from(objective);
    drop(graph);
    let composition = index.scoring_events(&progression.intro)?;
    let signals = index.scoring_event_sync(&progression.intro, &composition)?;
    Ok(Composite {
        objective: id,
        sequence: data,
        completion: completion(&composition, &signals, &objective)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::events::{Composition, Node};
    use nfs_world::scoring::{
        event_sync::Binding,
        events::{Connection, Delivery, Endpoint},
    };
    #[test]
    fn completion_follows_prefab_forwarding_but_not_remote_or_other_objectives() {
        let id = |index| Identity {
            asset: "test".into(),
            index,
            instances: vec![],
        };
        let mut c = Composition {
            nodes: (0..4)
                .map(|i| Node {
                    identity: id(i),
                    class: "test".into(),
                    listeners: None,
                    forward: (i == 1).then_some(2),
                })
                .collect(),
            buses: vec![],
            bus_entities: vec![],
            properties: vec![],
            links: vec![],
            connections: vec![
                Connection {
                    source: Endpoint {
                        node: 0,
                        event: 193_453_899,
                    },
                    target: Endpoint { node: 1, event: 72 },
                    delivery: Delivery::Local,
                },
                Connection {
                    source: Endpoint { node: 2, event: 72 },
                    target: Endpoint {
                        node: 3,
                        event: 448_895_852,
                    },
                    delivery: Delivery::Local,
                },
            ],
        };
        let b = Binding {
            entity: 9,
            event: 11,
            node: 0,
        };
        assert_eq!(completion(&c, &[b], &id(3)).unwrap(), b);
        assert!(completion(&c, &[b], &id(2)).is_err());
        assert!(completion(&c, &[b, b], &id(3)).is_err());
        c.connections[1].delivery =
            Delivery::NetworkedClient(nfs_world::scoring::events::Remote { bus: 0, index: 1 });
        assert!(completion(&c, &[b], &id(3)).is_err());
        c.connections[1].delivery = Delivery::Local;
        c.nodes[1].forward = None;
        assert!(completion(&c, &[b], &id(3)).is_err());
        c.nodes[1].forward = Some(2);
        c.connections[1].target.event = 1;
        assert!(completion(&c, &[b], &id(3)).is_err());
    }
}
