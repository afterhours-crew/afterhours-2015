// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The intro's direct result-to-objective path. Collector identity includes the
//! launcher occurrence; a reused game-mode blueprint is not a story identity.
use super::*;
use crate::scoring::events::Identity;
use nfs_world::scoring::events::Delivery;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Completion {
    pub source: Identity,
    pub collector: lifecycle::bindings::Link,
    pub objective: u64,
}

fn one<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let ends = graph.links(site, port)?;
    match ends.as_slice() {
        [end] if end.port == 0 => Ok(end.site.clone()),
        _ => Err(bad("intro result direct link")),
    }
}
pub(crate) fn categories<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
) -> Result<BTreeSet<u32>, Error> {
    let object = graph.object(site)?;
    let rows = object
        .fields
        .get("Categories")
        .and_then(Value::as_array)
        .filter(|r| r.len() <= 128)
        .ok_or_else(|| bad("intro result categories"))?;
    let mut out = BTreeSet::new();
    for row in rows {
        let key = graph
            .source
            .target(
                &site.key.0,
                row.as_pointer()
                    .ok_or_else(|| bad("intro result category pointer"))?,
            )?
            .ok_or_else(|| bad("intro result null category"))?;
        let category = graph.source.object(&key)?;
        if category.class != "GameModeCategory" {
            return Err(bad("intro result category class"));
        }
        out.insert(crate::graph::bits32(
            category
                .fields
                .get("CategoryHash")
                .ok_or_else(|| bad("intro result category hash"))?,
        )?);
    }
    Ok(out)
}

pub(super) fn read<S: Source>(
    graph: &mut Graph<S>,
    branch: &[Node],
    aliases: &BTreeMap<Site, Site>,
    lifecycle: &lifecycle::Lifecycle,
    stable_port: u32,
) -> Result<Option<Completion>, Error> {
    let definitions = &lifecycle.bindings.results;
    if definitions.is_empty() {
        return Ok(None);
    }
    let [result] = definitions.as_slice() else {
        return Err(bad("intro result source count"));
    };
    let channel = lifecycle
        .bindings
        .channels
        .iter()
        .find(|c| c.reference == result.collector.identity)
        .ok_or_else(|| bad("intro result channel binding"))?;
    let targets = channel
        .links
        .get(&result.collector.port)
        .ok_or_else(|| bad("intro result collector port"))?;
    let [target] = targets.as_slice() else {
        return Err(bad("intro result collector count"));
    };
    if target.port != 0 || target.class != "ResultCollectorEntityData" {
        return Err(bad("intro result collector class"));
    }
    let collector = &branch
        .iter()
        .find(|n| Identity::from(&n.site) == target.identity)
        .ok_or_else(|| bad("intro result collector occurrence"))?
        .site;
    let objective = validate(graph, branch, aliases, collector, stable_port)?;
    Ok(Some(Completion {
        source: result.identity.clone(),
        collector: result.collector.clone(),
        objective,
    }))
}
fn validate<S: Source>(
    graph: &mut Graph<S>,
    branch: &[Node],
    aliases: &BTreeMap<Site, Site>,
    collector: &Site,
    stable_port: u32,
) -> Result<u64, Error> {
    let root = Site {
        key: collector.bus.clone(),
        ..collector.clone()
    };
    let nodes = graph.read(root.clone())?;
    let reporter = unique(&nodes, "ParticipantResultReporterEntityData")?;
    let objective = unique(&nodes, "RaceObjectiveEntityData")?;
    let details = one(graph, collector, (-108_135_984i32) as u32)?;
    if graph.object(&details)?.class != "EventDetailsEntityData"
        || graph
            .object(collector)?
            .fields
            .get("SendToResultFilters")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return Err(bad("intro result collector policy"));
    }
    let object = graph.object(reporter)?;
    if object.fields.get("Enabled").and_then(Value::as_bool) != Some(true)
        || object
            .fields
            .get("MatchAnyCategory")
            .and_then(Value::as_bool)
            != Some(false)
        || object.fields.get("Id").and_then(Value::as_pointer)
            != Some(nfs_frostbite::ebx::Pointer::Null)
    {
        return Err(bad("intro result reporter policy"));
    }
    let enabled = crate::events::values::name("Enabled");
    if graph.input_ports(reporter)? != vec![enabled] {
        return Err(bad("intro result reporter enable input"));
    }
    let input = graph.input(reporter, enabled)?;
    if input.site != *objective || input.port != crate::events::values::name("IsActive") {
        return Err(bad("intro result reporter objective activation"));
    }
    let wanted = categories(graph, reporter)?;
    if wanted.is_empty() || !wanted.is_subset(&categories(graph, &details)?) {
        return Err(bad("intro result category mismatch"));
    }
    let manager_port = (-1_713_696_348i32) as u32;
    let manager = links(
        graph,
        collector,
        manager_port,
        aliases,
        &mut BTreeSet::new(),
    )?;
    if manager.len() != 1
        || manager[0].port != 0
        || graph.object(&manager[0].site)?.class != "ResultManagerEntityData"
        || manager != links(graph, reporter, manager_port, aliases, &mut BTreeSet::new())?
    {
        return Err(bad("intro result manager ownership"));
    }
    let provider = one(graph, reporter, (-1_232_400_965i32) as u32)?;
    if graph.object(&provider)?.class != "ParticipantLogicParticipantProviderEntityData"
        || !branch.iter().any(|n| n.site == provider)
    {
        return Err(bad("intro result participant ownership"));
    }
    let composition = crate::scoring::events::read_at(graph.source, root)?;
    let receiver = composition
        .nodes
        .iter()
        .position(|n| n.identity == Identity::from(objective))
        .ok_or_else(|| bad("intro result objective occurrence"))?;
    let edges: Vec<_> = composition
        .connections
        .iter()
        .filter(|e| {
            usize::from(e.target.node) == receiver
                && e.target.event == 448_895_852
                && matches!(
                    e.delivery,
                    Delivery::Local | Delivery::NetworkedClientAndServer(_)
                )
        })
        .collect();
    if edges.len() != 1
        || edges[0].source.event != 89_858_563
        || composition.nodes[usize::from(edges[0].source.node)].identity != Identity::from(reporter)
    {
        return Err(bad("intro result completion connection"));
    }
    let stable = one(graph, objective, stable_port)?;
    let object = graph.object(&stable)?;
    if object.class != "StableUniqueIdEntityData" {
        return Err(bad("intro result objective class"));
    }
    let objective = match object.fields.get("StableUniqueId64") {
        Some(Value::UInt64(id)) => *id,
        Some(Value::Int64(id)) => *id as u64,
        _ => return Err(bad("intro result objective identity")),
    };
    if objective == 0 {
        return Err(bad("intro result zero objective"));
    }
    Ok(objective)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local};
    use nfs_frostbite::ebx::{Fields, Pointer};
    fn structure(rows: &[(&str, Value)]) -> Value {
        Value::Struct(Fields(
            rows.iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ))
    }
    fn fixture() -> Memory {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "LogicPrefabBlueprint",
            &[
                ("Objects", Value::Array((1..=8).map(local).collect())),
                ("Interface", Value::Pointer(Pointer::Null)),
                ("Flags", Value::UInt16(0)),
                (
                    "PropertyConnections",
                    Value::Array(vec![structure(&[
                        ("Source", local(3)),
                        ("Target", local(2)),
                        (
                            "SourceFieldId",
                            Value::UInt32(crate::events::values::name("IsActive")),
                        ),
                        (
                            "TargetFieldId",
                            Value::UInt32(crate::events::values::name("Enabled")),
                        ),
                        ("Flags", Value::UInt32(3)),
                    ])]),
                ),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(local(1), (-108_135_984i32) as u32, local(4), 0),
                        edge(local(1), (-1_713_696_348i32) as u32, local(5), 0),
                        edge(local(2), (-1_713_696_348i32) as u32, local(5), 0),
                        edge(local(2), (-1_232_400_965i32) as u32, local(6), 0),
                        edge(local(3), 99, local(7), 0),
                    ]),
                ),
                (
                    "EventConnections",
                    Value::Array(vec![structure(&[
                        ("Source", local(2)),
                        ("Target", local(3)),
                        (
                            "SourceEvent",
                            structure(&[("Id", Value::UInt32(89_858_563))]),
                        ),
                        (
                            "TargetEvent",
                            structure(&[("Id", Value::UInt32(448_895_852))]),
                        ),
                        (
                            "TargetType",
                            Value::Enum {
                                value: 3,
                                name: Some("EventConnectionTargetType_Server".into()),
                            },
                        ),
                    ])]),
                ),
            ],
        );
        m.put(
            "a",
            1,
            "ResultCollectorEntityData",
            &[("SendToResultFilters", Value::Bool(false))],
        );
        m.put(
            "a",
            2,
            "ParticipantResultReporterEntityData",
            &[
                ("Enabled", Value::Bool(true)),
                ("MatchAnyCategory", Value::Bool(false)),
                ("Id", Value::Pointer(Pointer::Null)),
                ("Categories", Value::Array(vec![local(8)])),
            ],
        );
        m.put("a", 3, "RaceObjectiveEntityData", &[]);
        m.put(
            "a",
            4,
            "EventDetailsEntityData",
            &[("Categories", Value::Array(vec![local(8)]))],
        );
        m.put("a", 5, "ResultManagerEntityData", &[]);
        m.put("a", 6, "ParticipantLogicParticipantProviderEntityData", &[]);
        m.put(
            "a",
            7,
            "StableUniqueIdEntityData",
            &[("StableUniqueId64", Value::UInt64(1234))],
        );
        m.put(
            "a",
            8,
            "GameModeCategory",
            &[("CategoryHash", Value::UInt32(77))],
        );
        m
    }
    fn check(m: &mut Memory) -> Result<u64, Error> {
        let mut g = Graph::new(m);
        let branch = g.read(Site::root("a"))?;
        validate(
            &mut g,
            &branch,
            &BTreeMap::new(),
            &Site {
                key: ("a".into(), 1),
                ..Site::root("a")
            },
            99,
        )
    }
    #[test]
    fn completion_requires_matching_categories_and_owned_provider() {
        let mut m = fixture();
        assert_eq!(check(&mut m).unwrap(), 1234);
        m.put(
            "a",
            4,
            "EventDetailsEntityData",
            &[("Categories", Value::Array(vec![]))],
        );
        assert!(check(&mut m).is_err());
        let mut m = fixture();
        m.put("a", 6, "OtherParticipantProvider", &[]);
        assert!(check(&mut m).is_err());
        let mut m = fixture();
        m.put("a", 5, "OtherManager", &[]);
        assert!(check(&mut m).is_err());
    }
    #[test]
    fn disabled_filtered_or_client_only_completion_is_rejected() {
        let mut m = fixture();
        m.put(
            "a",
            1,
            "ResultCollectorEntityData",
            &[("SendToResultFilters", Value::Bool(true))],
        );
        assert!(check(&mut m).is_err());
        let mut m = fixture();
        let reporter = m.0.get_mut(&("a".into(), 2)).unwrap();
        reporter
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "Enabled")
            .unwrap()
            .1 = Value::Bool(false);
        assert!(check(&mut m).is_err());
        let mut m = fixture();
        let root = m.0.get_mut(&("a".into(), 0)).unwrap();
        let Value::Array(rows) = &mut root
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "EventConnections")
            .unwrap()
            .1
        else {
            panic!()
        };
        let Value::Struct(fields) = &mut rows[0] else {
            panic!()
        };
        fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "TargetType")
            .unwrap()
            .1 = Value::Enum {
            value: 2,
            name: Some("EventConnectionTargetType_Client".into()),
        };
        assert!(check(&mut m).is_err());
    }
}
