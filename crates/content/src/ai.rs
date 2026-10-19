// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared installed spawn lifecycle for traffic and roaming racers.
use crate::{
    Error,
    graph::{Graph, Site, Source, scenes::SceneLookup},
};
use nfs_frostbite::ebx::Value;
use nfs_world::replication::players::ai::State;
use serde_json::{Value as Json, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Links {
    pub initial: u32,
    pub source: u32,
    pub destination: u32,
    /// AiResetTrackerEntityData link to its streamed trigger-volume collection.
    pub reset_volumes: u32,
}
fn bad(reason: &str) -> Error {
    Error::Content(format!("AI lifecycle: {reason}"))
}
impl Links {
    pub(crate) fn to_json(self) -> Json {
        json!({"initial":self.initial,"source":self.source,"destination":self.destination,
            "reset_volumes":self.reset_volumes})
    }
    pub(crate) fn from_json(v: &Json) -> Result<Self, Error> {
        let o = v
            .as_object()
            .filter(|o| o.len() == 4)
            .ok_or_else(|| bad("ports"))?;
        let get = |key| {
            o.get(key)
                .and_then(Json::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad("port"))
        };
        let out = Self {
            initial: get("initial")?,
            source: get("source")?,
            destination: get("destination")?,
            reset_volumes: get("reset_volumes")?,
        };
        let ports = BTreeSet::from([out.initial, out.source, out.destination, out.reset_volumes]);
        if ports.len() != 4 || ports.contains(&0) {
            return Err(bad("distinct ports"));
        }
        Ok(out)
    }
}

pub(crate) fn state_json(state: State) -> Json {
    json!({"scene":state.scene,"serializer":state.serializer})
}
pub(crate) fn state_from_json(v: &Json) -> Result<State, Error> {
    let o = v
        .as_object()
        .filter(|o| o.len() == 2)
        .ok_or_else(|| bad("state"))?;
    let scene = o
        .get("scene")
        .and_then(Json::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n != 0)
        .ok_or_else(|| bad("state scene"))?;
    let serializer = o
        .get("serializer")
        .and_then(Json::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n < nfs_world::replication::sublevel::MAX_SERIALIZERS)
        .ok_or_else(|| bad("state serializer"))?;
    Ok(State { scene, serializer })
}

fn one<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let links = graph.links(site, port)?;
    if links.len() != 1 {
        return Err(bad("ambiguous state link"));
    }
    Ok(links[0].site.clone())
}

/// Validate the supported sequence instead of selecting by debug label or
/// assuming a fixed serializer ordinal. Intermediate states do not send RPCs.
fn alive<S: Source>(
    graph: &mut Graph<S>,
    roots: &[Site],
    manager: &Site,
    ports: Links,
) -> Result<Site, Error> {
    let mut transitions = Vec::new();
    for root in roots {
        for node in graph.read(root.clone())? {
            if node
                .object
                .class
                .ends_with("ParticipantTransitionEntityData")
            {
                transitions.push(node);
            }
        }
    }
    let mut state = one(graph, manager, ports.initial)?;
    let classes = [
        "UpdateableParticipantStateEntityData",
        "GetAiSpawnParametersParticipantStateEntityData",
        "SpawningParticipantStateEntityData",
        "ParticipantStateEntityData",
    ];
    let mut visited = BTreeSet::new();
    for (index, class) in classes.into_iter().enumerate() {
        let object = graph.object(&state)?;
        if !visited.insert(state.clone())
            || object.class != class
            || object
                .fields
                .get("SendRpcToClientSide")
                .and_then(Value::as_bool)
                != Some(index == 3)
        {
            return Err(bad("unsupported spawn state"));
        }
        if index == 3 {
            return Ok(state);
        }
        let mut next = None;
        for transition in &transitions {
            let sources = graph.links(&transition.site, ports.source)?;
            if sources.iter().any(|s| s.site == state)
                && (sources.len() != 1
                    || transition.object.class != "ParticipantTransitionEntityData"
                    || next
                        .replace(one(graph, &transition.site, ports.destination)?)
                        .is_some())
            {
                return Err(bad("ambiguous spawn transition"));
            }
        }
        state = next.ok_or_else(|| bad("missing spawn transition"))?;
    }
    Err(bad("incomplete spawn sequence"))
}

pub(crate) fn resolve(
    graph: &mut Graph<crate::scan::Assets>,
    lookup: &mut SceneLookup,
    manager: &Site,
    ports: Links,
) -> Result<State, Error> {
    let roots: Vec<_> = lookup.roots.values().cloned().collect();
    let state = alive(graph, &roots, manager, ports)?;
    let (scene, serializer) = lookup.endpoint(graph, &state)?;
    Ok(State { scene, serializer })
}

/// The single AI reset tracker's streamed trigger-volume collection. The host
/// must own its membership (`ClientControlled` false, `NotifyServer` true).
fn reset_collection<S: Source>(
    graph: &mut Graph<S>,
    roots: &[Site],
    ports: Links,
) -> Result<Site, Error> {
    let mut trackers = Vec::new();
    for root in roots {
        for node in graph.read(root.clone())? {
            if node.object.class == "AiResetTrackerEntityData" && !trackers.contains(&node.site) {
                trackers.push(node.site);
            }
        }
    }
    let [tracker] = trackers.as_slice() else {
        return Err(bad("reset tracker count"));
    };
    let collection = one(graph, tracker, ports.reset_volumes)?;
    let object = graph.object(&collection)?;
    let flag = |name| object.fields.get(name).and_then(Value::as_bool);
    if object.class != "StreamedTriggerVolumeCollectionEntityData"
        || flag("ClientControlled") != Some(false)
        || flag("NotifyServer") != Some(true)
    {
        return Err(bad("reset volume collection"));
    }
    Ok(collection)
}

pub(crate) fn resolve_reset_volumes(
    graph: &mut Graph<crate::scan::Assets>,
    lookup: &mut SceneLookup,
    ports: Links,
) -> Result<State, Error> {
    let roots: Vec<_> = lookup.roots.values().cloned().collect();
    let collection = reset_collection(graph, &roots, ports)?;
    let (scene, serializer) = lookup.endpoint(graph, &collection)?;
    Ok(State { scene, serializer })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local};
    fn fixture() -> (Memory, Links) {
        let ports = Links {
            initial: 11,
            source: 22,
            destination: 33,
            reset_volumes: 44,
        };
        let mut memory = Memory::default();
        let mut edges = vec![edge(local(1), ports.initial, local(2), 0)];
        for i in 0..3 {
            edges.push(edge(local(6 + i), ports.source, local(2 + i), 0));
            edges.push(edge(local(6 + i), ports.destination, local(3 + i), 0));
        }
        memory.put(
            "s",
            0,
            "SubWorldData",
            &[
                ("Objects", Value::Array((1..9).rev().map(local).collect())),
                ("LinkConnections", Value::Array(edges)),
            ],
        );
        memory.put("s", 1, "AiParticipantManagerEntityData", &[]);
        for (i, class) in [
            "UpdateableParticipantStateEntityData",
            "GetAiSpawnParametersParticipantStateEntityData",
            "SpawningParticipantStateEntityData",
            "ParticipantStateEntityData",
        ]
        .into_iter()
        .enumerate()
        {
            memory.put(
                "s",
                2 + i,
                class,
                &[("SendRpcToClientSide", Value::Bool(i == 3))],
            );
        }
        for i in 6..9 {
            memory.put("s", i, "ParticipantTransitionEntityData", &[]);
        }
        (memory, ports)
    }
    fn resolve_test(memory: &mut Memory, ports: Links) -> Result<Site, Error> {
        let root = Site::root("s");
        alive(
            &mut Graph::new(memory),
            std::slice::from_ref(&root),
            &Site {
                key: ("s".into(), 1),
                ..root.clone()
            },
            ports,
        )
    }
    fn reset_fixture(collections: &[(bool, bool)], trackers: usize) -> Memory {
        let mut memory = Memory::default();
        let mut objects = Vec::new();
        let mut edges = Vec::new();
        for t in 0..trackers {
            objects.push(local(1 + t));
            memory.put("s", 1 + t, "AiResetTrackerEntityData", &[]);
            for (c, (client, notify)) in collections.iter().enumerate() {
                let index = 10 + c;
                edges.push(edge(local(1 + t), 44, local(index), 0));
                memory.put(
                    "s",
                    index,
                    "StreamedTriggerVolumeCollectionEntityData",
                    &[
                        ("NotifyServer", Value::Bool(*notify)),
                        ("ClientControlled", Value::Bool(*client)),
                    ],
                );
            }
        }
        // An unrelated collection linked from a different port must be ignored.
        objects.push(local(20));
        memory.put(
            "s",
            20,
            "StreamedTriggerVolumeCollectionEntityData",
            &[
                ("NotifyServer", Value::Bool(true)),
                ("ClientControlled", Value::Bool(false)),
            ],
        );
        edges.push(edge(local(1), 55, local(20), 0));
        objects.extend((10..10 + collections.len()).map(local));
        memory.put(
            "s",
            0,
            "SubWorldData",
            &[
                ("Objects", Value::Array(objects)),
                ("LinkConnections", Value::Array(edges)),
            ],
        );
        memory
    }

    #[test]
    fn reset_volumes_follow_the_unique_tracker_to_a_server_controlled_collection() {
        let (_, ports) = fixture();
        let roots = [Site::root("s")];
        let mut memory = reset_fixture(&[(false, true)], 1);
        let site = reset_collection(&mut Graph::new(&mut memory), &roots, ports).unwrap();
        assert_eq!(site.key, ("s".into(), 10));
        for (collections, trackers) in [
            (vec![(false, true)], 0),
            (vec![(false, true)], 2),
            (vec![(false, true), (false, true)], 1),
            (vec![(true, true)], 1),
            (vec![(false, false)], 1),
        ] {
            let mut memory = reset_fixture(&collections, trackers);
            assert!(reset_collection(&mut Graph::new(&mut memory), &roots, ports).is_err());
        }
    }

    #[test]
    fn follows_edges_independently_of_order_and_rejects_incomplete_or_ambiguous_lifecycle() {
        let (mut memory, ports) = fixture();
        assert_eq!(
            resolve_test(&mut memory, ports).unwrap().key,
            ("s".into(), 5)
        );
        for index in 2..6 {
            let (mut memory, ports) = fixture();
            memory.0.get_mut(&("s".into(), index)).unwrap().fields.0[0].1 = Value::Bool(index != 5);
            assert!(resolve_test(&mut memory, ports).is_err());
        }
        for replacement in [
            vec![edge(local(1), ports.initial, local(2), 0)],
            vec![
                edge(local(1), ports.initial, local(2), 0),
                edge(local(1), ports.initial, local(5), 0),
            ],
        ] {
            let (mut memory, ports) = fixture();
            memory.0.get_mut(&("s".into(), 0)).unwrap().fields.0[1].1 = Value::Array(replacement);
            assert!(resolve_test(&mut memory, ports).is_err());
        }
        let (mut memory, ports) = fixture();
        memory.0.get_mut(&("s".into(), 6)).unwrap().class =
            "TriggeredParticipantTransitionEntityData".into();
        assert!(resolve_test(&mut memory, ports).is_err());
        let (mut memory, ports) = fixture();
        let Value::Array(edges) = &mut memory.0.get_mut(&("s".into(), 0)).unwrap().fields.0[1].1
        else {
            panic!()
        };
        edges.push(edge(local(7), ports.source, local(2), 0));
        assert!(resolve_test(&mut memory, ports).is_err());
    }
}
