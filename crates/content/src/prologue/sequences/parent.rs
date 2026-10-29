// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::{Endpoint, bad};
use crate::scoring::events::{self, Identity};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Relay {
    pub objective: u64,
    pub selected: u32,
    pub watched: u32,
    pub primary_slot: Channel,
}

pub(super) struct Resolved {
    pub objective: u64,
    pub selected: u32,
    pub watched: u32,
    pub slot: Endpoint,
}

/// Resolve the parent receiver through the same event asset and relay instance.
/// Only the observed direct Boolean/completion/slot-load subset is accepted.
pub(super) fn read<S: Source>(
    graph: &mut Graph<S>,
    relay: &Site,
    event_asset: &crate::graph::Key,
    stable_port: u32,
) -> Result<Resolved, Error> {
    let root = Site {
        key: relay.bus.clone(),
        ..relay.clone()
    };
    let nodes = graph.read(root.clone())?;
    let mut receivers = vec![];
    for node in &nodes {
        if node.object.class == "ParticipantEventEntityData"
            && graph.target_field(&node.site, "EventAsset")? == *event_asset
            && graph.links(&node.site, 3_253_352_650)?
                == vec![Endpoint {
                    site: relay.clone(),
                    port: 0,
                }]
        {
            receivers.push(node.site.clone());
        }
    }
    let [receiver] = receivers.as_slice() else {
        return Err(bad("parent participant event receiver"));
    };
    let topology = events::read_at(graph.source, root)?;
    let source = topology
        .nodes
        .iter()
        .position(|n| n.identity == Identity::from(receiver))
        .ok_or_else(|| bad("parent event topology"))? as u16;
    let mut watched = None;
    let mut selected = None;
    let mut loader = None;
    let mut objective = None;
    for edge in topology
        .connections
        .iter()
        .filter(|e| e.source.node == source)
    {
        if edge.delivery != nfs_world::scoring::events::Delivery::Local {
            return Err(bad("parent event delivery"));
        }
        let target = &topology.nodes[usize::from(edge.target.node)];
        let site = &nodes
            .iter()
            .find(|n| Identity::from(&n.site) == target.identity)
            .ok_or_else(|| bad("parent target identity"))?
            .site;
        match (target.class.as_str(), edge.target.event) {
            ("BoolEntityData", nfs_world::scoring::nodes::SET_TRUE) if watched.is_none() => {
                watched = Some(edge.source.event)
            }
            ("RaceObjectiveEntityData", 448_895_852) if objective.is_none() => {
                selected = Some(edge.source.event);
                let stable =
                    super::super::endpoint(graph, site, stable_port, "StableUniqueIdEntityData")?;
                objective = Some(
                    match graph.object(&stable)?.fields.get("StableUniqueId64") {
                        Some(Value::UInt64(id)) => *id,
                        Some(Value::Int64(id)) => *id as u64,
                        _ => return Err(bad("parent objective identity")),
                    },
                );
            }
            ("VehicleSlotLoaderEntityData", 2_089_019_715) if loader.is_none() => {
                let ends = graph.links(site, 926_267_033)?;
                let [end] = ends.as_slice() else {
                    return Err(bad("parent primary slot"));
                };
                loader = Some((edge.source.event, end.clone()));
            }
            _ => return Err(bad("unsupported parent participant event effect")),
        }
    }
    let (load_event, slot) = loader.ok_or_else(|| bad("missing parent loader"))?;
    let selected = selected.ok_or_else(|| bad("missing parent completion"))?;
    let watched = watched.ok_or_else(|| bad("missing watched introduction"))?;
    let objective = objective
        .filter(|n| *n != 0)
        .ok_or_else(|| bad("missing parent objective"))?;
    if load_event != selected || selected == watched {
        return Err(bad("parent event ordering"));
    }
    Ok(Resolved {
        objective,
        selected,
        watched,
        slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge as link, local};
    use nfs_frostbite::ebx::{Fields, Pointer};
    fn structure(fields: &[(&str, Value)]) -> Value {
        Value::Struct(Fields(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ))
    }
    fn event(event: u32, target: usize, input: u32) -> Value {
        structure(&[
            ("Source", local(1)),
            ("Target", local(target)),
            ("SourceEvent", structure(&[("Id", Value::UInt32(event))])),
            ("TargetEvent", structure(&[("Id", Value::UInt32(input))])),
            (
                "TargetType",
                Value::Enum {
                    value: 3,
                    name: Some("EventConnectionTargetType_Server".into()),
                },
            ),
        ])
    }
    fn fixture() -> Memory {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "LogicPrefabBlueprint",
            &[
                ("Objects", Value::Array((1..=7).map(local).collect())),
                ("Interface", Value::Pointer(Pointer::Null)),
                ("Flags", Value::UInt16(0)),
                ("PropertyConnections", Value::Array(vec![])),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        link(local(1), 3_253_352_650, local(2), 0),
                        link(local(4), 99, local(6), 0),
                        link(local(5), 926_267_033, local(7), 0),
                    ]),
                ),
                (
                    "EventConnections",
                    Value::Array(vec![
                        event(41, 3, nfs_world::scoring::nodes::SET_TRUE),
                        event(42, 5, 2_089_019_715),
                        event(42, 4, 448_895_852),
                    ]),
                ),
            ],
        );
        m.put(
            "a",
            1,
            "ParticipantEventEntityData",
            &[("EventAsset", local(8))],
        );
        m.put("a", 2, "ParticipantEventRelayEntityData", &[]);
        m.put(
            "a",
            3,
            "BoolEntityData",
            &[(
                "Realm",
                Value::Enum {
                    value: 2,
                    name: Some("Realm_Server".into()),
                },
            )],
        );
        m.put("a", 4, "RaceObjectiveEntityData", &[]);
        m.put("a", 5, "VehicleSlotLoaderEntityData", &[]);
        m.put(
            "a",
            6,
            "StableUniqueIdEntityData",
            &[("StableUniqueId64", Value::UInt64(1234))],
        );
        m.put("a", 7, "VehicleSlotEntityData", &[]);
        m.put("a", 8, "EventDefinitionAsset", &[]);
        m
    }
    #[test]
    fn parent_requires_the_owned_relay_and_matching_completion_and_load_event() {
        let mut m = fixture();
        let relay = Site {
            key: ("a".into(), 2),
            ..Site::root("a")
        };
        let resolved = read(&mut Graph::new(&mut m), &relay, &("a".into(), 8), 99).unwrap();
        assert_eq!(
            (resolved.objective, resolved.selected, resolved.watched),
            (1234, 42, 41)
        );
        assert_eq!(resolved.slot.site.key, ("a".into(), 7));
        assert!(read(&mut Graph::new(&mut m), &relay, &("a".into(), 7), 99).is_err());
        let rows =
            m.0.get_mut(&("a".into(), 0))
                .unwrap()
                .fields
                .0
                .iter_mut()
                .find(|(k, _)| k == "EventConnections")
                .unwrap();
        rows.1 = Value::Array(vec![
            event(41, 3, nfs_world::scoring::nodes::SET_TRUE),
            event(43, 5, 2_089_019_715),
            event(42, 4, 448_895_852),
        ]);
        assert!(read(&mut Graph::new(&mut m), &relay, &("a".into(), 8), 99).is_err());
    }
}
