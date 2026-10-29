// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! A mode's final transition driven by its owning story objective.
use super::*;
use crate::{
    graph::{Endpoint, Frame},
    scoring::events::Identity,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    pub objective: u64,
    pub source: Identity,
    pub event: u32,
    /// The objective comparison whose output drives the shutdown channel.
    pub compare: Identity,
}

fn property<S: Source>(
    graph: &mut Graph<S>,
    mut site: Site,
    mut port: u32,
) -> Result<Endpoint, Error> {
    let mut seen = BTreeSet::new();
    loop {
        if seen.len() >= 64 || !seen.insert((site.clone(), port)) {
            return Err(bad("shutdown property cycle"));
        }
        let [source] = <[_; 1]>::try_from(graph.property_sources(&site, port)?)
            .map_err(|_| bad("shutdown property source count"))?;
        site = source.site;
        port = source.port;
        match graph.object(&site)?.class.as_str() {
            "InterfaceDescriptorData" => {
                let frame = site
                    .parents
                    .pop()
                    .ok_or_else(|| bad("shutdown root interface"))?;
                site.key = frame.reference;
                site.bus = frame.bus;
            }
            "LogicPrefabReferenceObjectData" => {
                let child = graph.target_field(&site, "Blueprint")?;
                site.parents.push(Frame {
                    bus: site.bus.clone(),
                    reference: site.key.clone(),
                });
                site.bus = child.clone();
                site.key = child;
                site.key = graph.target_field(&site, "Interface")?;
            }
            _ => return Ok(Endpoint { site, port }),
        }
    }
}

pub(super) fn read<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    aliases: &BTreeMap<Site, (Site, crate::events::Binding)>,
    lifecycle: &crate::events::lifecycle::Lifecycle,
    fields: &[(Site, String, Kind)],
    stable_port: u32,
) -> Result<Option<Binding>, Error> {
    let mut found = vec![];
    for transition in lifecycle.transitions.iter().filter(|t| {
        t.kind == crate::events::lifecycle::TransitionKind::Triggered
            && t.target
                .is_some_and(|s| s.serializer == lifecycle.final_state)
    }) {
        for (source, event) in graph.event_sources(
            &fields[transition.serializer].0,
            nfs_world::scoring::transitions::TRIGGER_ALL,
        )? {
            if graph.object(&source)?.class != "LinkedSchematicChannelReferenceEntityData" {
                continue;
            }
            let (parent, _) = aliases
                .get(&source)
                .ok_or_else(|| bad("shutdown channel owner"))?;
            if graph.target_field(&source, "ChannelAsset")?
                != graph.target_field(parent, "ChannelAsset")?
            {
                return Err(bad("shutdown channel identity"));
            }
            let [(compare, output)] = <[_; 1]>::try_from(graph.event_sources(parent, event)?)
                .map_err(|_| bad("shutdown channel source"))?;
            let object = graph.object(&compare)?;
            if object.class != "CompareBoolEntityData"
                || output != 2_861_454_674
                || object.fields.get("TriggerOnPropertyChange") != Some(&Value::Bool(true))
            {
                return Err(bad("shutdown comparison"));
            }
            let compare_identity = Identity::from(&compare);
            let completed = property(graph, compare, 2_088_812_747)?;
            if completed.port != 1_514_830_046
                || graph.object(&completed.site)?.class != "RaceObjectiveEntityData"
            {
                return Err(bad("shutdown objective source"));
            }
            let [stable] = <[_; 1]>::try_from(graph.links(&completed.site, stable_port)?)
                .map_err(|_| bad("shutdown stable link"))?;
            let object = graph.object(&stable.site)?;
            if object.class != "StableUniqueIdEntityData" || stable.port != 0 {
                return Err(bad("shutdown stable identity"));
            }
            let objective = match object.fields.get("StableUniqueId64") {
                Some(Value::UInt64(n)) if *n != 0 => *n,
                Some(Value::Int64(n)) if *n != 0 => *n as u64,
                _ => return Err(bad("shutdown stable value")),
            };
            if !nodes.iter().any(|n| n.site == source) {
                return Err(bad("shutdown source occurrence"));
            }
            found.push(Binding {
                objective,
                source: Identity::from(&source),
                event,
                compare: compare_identity,
            });
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => Err(bad("ambiguous shutdown")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    #[test]
    fn shutdown_property_keeps_the_exact_prefab_occurrence_and_rejects_ambiguity() {
        let mut memory = Memory::default();
        memory.put(
            "a",
            0,
            "LogicPrefabBlueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![edge(local(2), 7, local(1), 8)]),
            )],
        );
        memory.put(
            "a",
            1,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("b", 0))],
        );
        memory.put("a", 2, "RaceObjectiveEntityData", &[]);
        memory.put(
            "b",
            0,
            "LogicPrefabBlueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![edge(local(1), 8, local(2), 9)]),
            )],
        );
        memory.put("b", 1, "InterfaceDescriptorData", &[]);
        memory.put("b", 2, "CompareBoolEntityData", &[]);
        let site = Site {
            key: ("b".into(), 2),
            bus: ("b".into(), 0),
            parents: vec![Frame {
                bus: ("a".into(), 0),
                reference: ("a".into(), 1),
            }],
        };
        let resolved = property(&mut Graph::new(&mut memory), site.clone(), 9).unwrap();
        assert_eq!(resolved.site.key, ("a".into(), 2));
        assert!(resolved.site.parents.is_empty());
        assert_eq!(resolved.port, 7);
        memory.put(
            "b",
            0,
            "LogicPrefabBlueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(1), 8, local(2), 9),
                    edge(local(1), 8, local(2), 9),
                ]),
            )],
        );
        assert!(property(&mut Graph::new(&mut memory), site, 9).is_err());
    }
}
