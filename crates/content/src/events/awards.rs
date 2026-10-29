// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compile the installed post-event award graph with externally owned metric
//! snapshots. UI text/persona selection remains in the installed client graph.
use super::{
    Binding, Event, Layout,
    values::{Evaluator, Number},
};
use crate::{
    Error,
    assets::AssetIndex,
    graph::*,
    scoring::{
        self,
        events::Identity,
        participant_properties::{self, Property},
    },
};
use nfs_frostbite::ebx::Value as Ebx;
use nfs_world::scoring::{
    integers,
    interpreter::{Inputs, Session},
    nodes,
    program::{Behavior, Program},
    properties::Port,
    synced,
    value::Value,
};
use std::{collections::BTreeMap, sync::Arc};

pub const BLUEPRINT: &str = "gameplay/raceeventawardsselectionparticipantlogic";
const CHANNELS: [&str; 2] = [
    "gameplay/raceeventawardsthresholdchannel",
    "gameplay/scoring/eventawardweights",
];
const RANK_IDENTIFIER: u32 = 233_976_021;

pub(super) fn bound_values<S: Source>(
    graph: &mut Graph<S>,
    refs: &[(Site, String)],
    targets: Vec<Site>,
    bindings: &BTreeMap<Site, (Site, Binding)>,
) -> Result<BTreeMap<Property, Value>, Error> {
    let mut values = BTreeMap::new();
    let channels: BTreeMap<_, _> = bindings
        .iter()
        .map(|(a, (b, _))| (a.clone(), b.clone()))
        .collect();
    for ((reference, _), mut target) in refs.iter().zip(targets) {
        if graph.object(reference)?.class != "LinkedSchematicChannelReferenceEntityData" {
            continue;
        }
        let channel = graph.target_field(reference, "ChannelAsset")?;
        if !CHANNELS.contains(&channel.0.as_str()) {
            continue;
        }
        if let Some((bound, _)) = bindings.get(&target) {
            target = bound.clone();
        }
        if graph.object(&target)?.class != "LinkedSchematicChannelEntityData"
            || graph.target_field(&target, "ChannelAsset")? != channel
        {
            return Err(bad("award channel binding"));
        }
        let object = graph.source.object(&channel)?;
        let properties = object
            .fields
            .get("Properties")
            .and_then(Ebx::as_array)
            .ok_or_else(|| bad("award channel properties"))?;
        let outputs: std::collections::BTreeSet<_> = graph
            .object(reference)?
            .fields
            .get("OutputProperties")
            .and_then(Ebx::as_array)
            .ok_or_else(|| bad("award reference outputs"))?
            .iter()
            .map(|v| {
                v.bits32()
                    .map(|v| v as usize)
                    .ok_or_else(|| bad("award output index"))
            })
            .collect::<Result<_, _>>()?;
        for index in outputs {
            let field = properties
                .get(index)
                .ok_or_else(|| bad("award output index bound"))?;
            let port = field
                .as_fields()
                .and_then(|f| f.get("Id"))
                .and_then(Ebx::bits32)
                .ok_or_else(|| bad("award channel port"))?;
            let value = Evaluator::with_channels(graph, &channels)
                .input(&target, port)?
                .ok_or_else(|| bad(&format!("unbound award channel {}:{port}", channel.0)))?;
            let value = match value {
                Number::Integer(v) => Value::Integer(v),
                Number::Float(v) => Value::Float(v),
                Number::Boolean(_) => return Err(bad("award channel type")),
            };
            if values
                .insert(
                    Property {
                        channel: channel.0.clone(),
                        port,
                    },
                    value,
                )
                .is_some()
            {
                return Err(bad("duplicate award channel value"));
            }
        }
    }
    if CHANNELS
        .iter()
        .any(|c| !values.keys().any(|p| p.channel == *c))
    {
        return Err(bad("missing bound award thresholds/weights"));
    }
    Ok(values)
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ranking {
    pub node: u16,
    pub property: Property,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Content {
    pub program: Arc<Program>,
    pub root: u16,
    pub fields: Vec<scoring::content::Field>,
    pub reads: Vec<(u16, Property)>,
    pub rankings: Vec<Ranking>,
    pub total: u16,
    pub keeper: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultValues {
    pub fields: BTreeMap<usize, i32>,
    pub rep: i32,
}

impl AssetIndex {
    pub fn event_awards(&mut self, event: &Event, layout: &Layout) -> Result<Content, Error> {
        if layout.blueprint != BLUEPRINT {
            return Err(bad("award blueprint"));
        }
        let mut sources = self.scoring_sources(&layout.blueprint)?;
        let mut graph = Graph::new(&mut self.assets);
        let graph_nodes = graph.read(Site::root(&layout.blueprint))?;
        let roots: Vec<_> = graph_nodes
            .iter()
            .filter(|n| n.object.class == "ParticipantLogicEntityData")
            .collect();
        let [participant_root] = roots.as_slice() else {
            return Err(bad("award participant root"));
        };
        let mut snapshots = BTreeMap::new();
        let mut reads = Vec::new();
        let mut rankings = Vec::new();
        let mut total = None;
        let mut root = None;
        let mut disabled_debug = std::collections::BTreeSet::new();
        for row in &graph_nodes {
            let identity = Identity::from(&row.site);
            let node = sources
                .composition
                .nodes
                .iter()
                .position(|n| n.identity == identity)
                .ok_or_else(|| bad("award node identity"))? as u16;
            if matches!(
                row.object.class.as_str(),
                participant_properties::ACCESSOR | "SortAndRankIntParticipantPropertyEntityData"
            ) {
                let providers = graph.links(&row.site, nfs_world::scoring::score::PARTICIPANT)?;
                let [provider] = providers.as_slice() else {
                    return Err(bad("award participant provider count"));
                };
                if provider.port != 0
                    || graph.object(&provider.site)?.class
                        != "ParticipantLogicParticipantProviderEntityData"
                {
                    return Err(bad("award participant provider"));
                }
                let links = graph.links(
                    &provider.site,
                    self.identity.participant_logic.boolean_ports["provider_root"],
                )?;
                if links.len() != 1 || links[0].port != 0 || links[0].site != participant_root.site
                {
                    return Err(bad("foreign award participant"));
                }
            }
            match row.object.class.as_str() {
                "PropertyDebugEntityData"
                    if row
                        .object
                        .fields
                        .get("DisableInFinal")
                        .and_then(Ebx::as_bool)
                        == Some(true) =>
                {
                    disabled_debug.insert(node);
                }
                "ParticipantLogicEntityData" => {
                    if root.replace(node).is_some() {
                        return Err(bad("award root count"));
                    }
                }
                participant_properties::ACCESSOR => {
                    let property = participant_properties::property(&mut graph, &row.site)?;
                    if !graph.input_ports(&row.site)?.contains(&nodes::FLOAT_VALUE) {
                        snapshots.insert(identity.clone(), vec![]);
                        if sources.composition.properties.iter().any(|e| {
                            e.source
                                == (Port {
                                    node,
                                    field: nodes::OUTPUT_VALUE,
                                })
                        }) {
                            snapshots
                                .insert(identity, vec![(nodes::OUTPUT_VALUE, Value::Integer(0))]);
                            reads.push((node, property));
                        }
                    } else if property.channel == "gameplay/scoring/eventresultwywawardscorechannel"
                    {
                        if total.replace(node).is_some() {
                            return Err(bad("award total count"));
                        }
                    } else {
                        return Err(bad("unexpected award property write"));
                    }
                }
                "SortAndRankIntParticipantPropertyEntityData" => {
                    if row
                        .object
                        .fields
                        .get("UseTieBreaker")
                        .and_then(Ebx::as_bool)
                        != Some(false)
                    {
                        return Err(bad("award rank tie breaker"));
                    }
                    let descending = match row.object.fields.get("SortingOrder") {
                        Some(Ebx::Enum {
                            value: 0,
                            name: Some(n),
                        }) if n == "SortOrder_Ascending" => false,
                        Some(Ebx::Enum {
                            value: 1,
                            name: Some(n),
                        }) if n == "SortOrder_Descending" => true,
                        _ => return Err(bad("award rank order")),
                    };
                    let property =
                        participant_properties::identifier(&mut graph, &row.site, RANK_IDENTIFIER)?;
                    snapshots.insert(
                        identity,
                        vec![
                            (integers::RANK, Value::Integer(0)),
                            (integers::COUNT, Value::Integer(0)),
                        ],
                    );
                    rankings.push(Ranking {
                        node,
                        property,
                        descending,
                    });
                }
                "LinkedSchematicChannelReferenceEntityData" => {
                    let channel = graph.target_field(&row.site, "ChannelAsset")?.0;
                    if CHANNELS.contains(&channel.as_str()) {
                        let ports: std::collections::BTreeSet<_> = sources
                            .composition
                            .properties
                            .iter()
                            .filter(|e| e.source.node == node)
                            .map(|e| e.source.field)
                            .collect();
                        let values = ports
                            .into_iter()
                            .map(|port| {
                                event
                                    .award_values
                                    .get(&Property {
                                        channel: channel.clone(),
                                        port,
                                    })
                                    .cloned()
                                    .map(|v| (port, v))
                                    .ok_or_else(|| bad("award channel value missing"))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        snapshots.insert(identity, values);
                    }
                }
                _ => {}
            }
        }
        // Shipping award graphs retain editor debug-display connections in
        // their data. Honor their explicit DisableInFinal flag; every other
        // external effect remains an error during this one-shot evaluation.
        sources
            .composition
            .connections
            .retain(|edge| !disabled_debug_edge(edge, &disabled_debug));
        let program = scoring::program::compile_with_snapshots(&sources, &snapshots)?;
        let [award] = sources.awards.as_slice() else {
            return Err(bad("award score count"));
        };
        let [keeper] = award.context.keepers.as_slice() else {
            return Err(bad("award keeper count"));
        };
        let fields = scoring::content::serializer_fields(
            &sources.composition,
            &self.identity.participant_logic,
        )?;
        if fields.len() != layout.fields.len()
            || fields
                .iter()
                .zip(&layout.fields)
                .any(|(a, b)| a.kind != b.kind)
        {
            return Err(bad("award serializer layout"));
        }
        Ok(Content {
            program: Arc::new(program),
            fields,
            reads,
            rankings,
            root: root.ok_or_else(|| bad("award root missing"))?,
            total: total.ok_or_else(|| bad("award total missing"))?,
            keeper: keeper.field,
        })
    }
}

fn disabled_debug_edge(
    edge: &nfs_world::scoring::events::Connection,
    disabled: &std::collections::BTreeSet<u16>,
) -> bool {
    matches!(
        edge.delivery,
        nfs_world::scoring::events::Delivery::NetworkedClient(_)
    ) && disabled.contains(&edge.target.node)
}

impl Content {
    /// Evaluate one participant from a snapshot of the event's property stores.
    /// The caller retains stable participant order. Missing property entries do
    /// not invent competing values: native ranking enumerates stored pairs.
    pub fn evaluate(
        &self,
        participant: u64,
        players: &[(u64, BTreeMap<Property, i32>)],
    ) -> Result<ResultValues, Error> {
        if players.is_empty() || players.len() > 28 {
            return Err(bad("award participant bound"));
        }
        let own = players
            .iter()
            .find(|(id, _)| *id == participant)
            .map(|(_, p)| p)
            .ok_or_else(|| bad("award participant missing"))?;
        let unique: std::collections::BTreeSet<_> = players.iter().map(|(id, _)| id).collect();
        if unique.len() != players.len() {
            return Err(bad("duplicate award participant"));
        }
        let mut program = (*self.program).clone();
        for (node, property) in &self.reads {
            let node = program
                .nodes
                .get_mut(usize::from(*node))
                .ok_or_else(|| bad("award read node"))?;
            if !matches!(node.behavior, Behavior::Snapshot(_)) {
                return Err(bad("award read is not a snapshot"));
            }
            node.behavior = Behavior::Snapshot(vec![(
                nodes::OUTPUT_VALUE,
                Value::Integer(own.get(property).copied().unwrap_or(0)),
            )]);
        }
        for ranking in &self.rankings {
            let node = program
                .nodes
                .get_mut(usize::from(ranking.node))
                .ok_or_else(|| bad("award ranking node"))?;
            if !matches!(node.behavior, Behavior::Snapshot(_)) {
                return Err(bad("award ranking is not a snapshot"));
            }
            node.behavior = Behavior::Snapshot(rank_values(ranking, participant, players));
        }
        let mut session = Session::new(Arc::new(program)).map_err(|e| bad(&e.to_string()))?;
        let inputs = Inputs {
            participant,
            vehicle: None,
            others: vec![],
            road: None,
            stats: None,
            now: 0.0,
            time_scale: 1.0,
        };
        let startup = session.start(&inputs).map_err(|e| {
            bad(&format!(
                "award startup step {:?}: {e}",
                session.failed_step()
            ))
        })?;
        let output = session
            .event(self.root, super::values::name("OnEnabled"), &inputs)
            .map_err(|e| bad(&format!("award selection: {e}")))?;
        if !output.remote.is_empty()
            || !output.commands.is_empty()
            || !startup.remote.is_empty()
            || !startup.commands.is_empty()
        {
            return Err(bad(&format!(
                "unexpected external award graph effects: startup {:?} {:?}; selected {:?} {:?}",
                startup.remote, startup.commands, output.remote, output.commands
            )));
        }
        let mut fields = BTreeMap::new();
        for field in &self.fields {
            if field.kind == nfs_world::replication::entity::Kind::I32Property {
                match session
                    .read(Port {
                        node: field.node,
                        field: synced::OUT,
                    })
                    .map_err(|e| bad(&e.to_string()))?
                {
                    Some(Value::Integer(v)) => {
                        fields.insert(field.serializer, v);
                    }
                    None => {}
                    _ => return Err(bad("award synchronized type")),
                }
            }
        }
        let rep = match session
            .read(Port {
                node: self.total,
                field: nodes::FLOAT_VALUE,
            })
            .map_err(|e| bad(&e.to_string()))?
        {
            Some(Value::Integer(v)) if v >= 0 => v,
            _ => return Err(bad("award total")),
        };
        let grants: Vec<_> = startup.awards.iter().chain(&output.awards).collect();
        if grants
            .iter()
            .any(|a| a.participant != participant || a.keeper != self.keeper)
            || (rep == 0 && !grants.is_empty())
            || (rep != 0 && (grants.len() != 1 || grants[0].amount != rep))
        {
            return Err(bad(&format!(
                "award score and presented total disagree: {rep}, {grants:?}"
            )));
        }
        Ok(ResultValues { fields, rep })
    }
}

fn rank_values(
    ranking: &Ranking,
    participant: u64,
    players: &[(u64, BTreeMap<Property, i32>)],
) -> Vec<(u32, Value)> {
    let mut pairs: Vec<_> = players
        .iter()
        .filter_map(|(id, props)| props.get(&ranking.property).map(|v| (*id, *v)))
        .collect();
    pairs.sort_by(|a, b| {
        if ranking.descending {
            b.1.cmp(&a.1)
        } else {
            a.1.cmp(&b.1)
        }
    });
    let Some(index) = pairs.iter().position(|(id, _)| *id == participant) else {
        return vec![];
    };
    vec![
        (integers::RANK, Value::Integer(index as i32 + 1)),
        (integers::COUNT, Value::Integer(pairs.len() as i32)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shipping_debug_filter_preserves_other_remote_and_server_effects() {
        use nfs_world::scoring::events::{Connection, Delivery, Endpoint, Remote};
        let mut edge = Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint { node: 1, event: 2 },
            delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 9 }),
        };
        let disabled = std::collections::BTreeSet::from([1]);
        assert!(disabled_debug_edge(&edge, &disabled));
        edge.target.node = 2;
        assert!(!disabled_debug_edge(&edge, &disabled));
        edge.target.node = 1;
        for delivery in [
            Delivery::Local,
            Delivery::NetworkedClientAndServer(Remote { bus: 0, index: 9 }),
        ] {
            edge.delivery = delivery;
            assert!(!disabled_debug_edge(&edge, &disabled));
        }
    }
    #[test]
    fn ranking_uses_stored_metric_pairs_without_inventing_absent_racers() {
        let property = Property {
            channel: "test".into(),
            port: 7,
        };
        let value = |v| BTreeMap::from([(property.clone(), v)]);
        let players = vec![
            (10, value(3)),
            (11, BTreeMap::new()),
            (12, value(9)),
            (13, value(9)),
        ];
        let mut ranking = Ranking {
            node: 0,
            property,
            descending: true,
        };
        for (id, rank) in [(10, 3), (12, 1), (13, 2)] {
            assert_eq!(
                rank_values(&ranking, id, &players),
                vec![
                    (integers::RANK, Value::Integer(rank)),
                    (integers::COUNT, Value::Integer(3))
                ]
            );
        }
        assert!(rank_values(&ranking, 11, &players).is_empty());
        ranking.descending = false;
        assert_eq!(rank_values(&ranking, 10, &players)[0].1, Value::Integer(1));
        assert_eq!(rank_values(&ranking, 13, &players)[0].1, Value::Integer(3));
    }
}
