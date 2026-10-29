// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Direct sequence references in the intro participant's composition.
use super::*;
use crate::{graph::scenes::SceneLookup, participant_logic::Channel, world::StartupKind};
use nfs_world::{
    replication::entity::{Kind, Profile, creation::Asset},
    sequences::Component,
};
use std::collections::{BTreeMap, BTreeSet};
mod parent;
pub mod program;
mod scope;
pub mod timing;
pub use parent::Relay;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sequence {
    pub blueprint: String,
    pub asset: Asset,
    pub catalog: Vec<(u32, u32)>,
    pub manager: Component,
    pub embedded_bus: u32,
    pub bus_count: u16,
    pub fields: Vec<Kind>,
    /// Serializer index to a scene or the owning intro participant-logic field.
    pub channels: BTreeMap<usize, Channel>,
}
impl Sequence {
    pub fn profile(&self) -> Result<Profile, Error> {
        Profile::new(&self.fields).map_err(|_| bad("sequence serializer profile"))
    }
}

/// EventSync outputs connected directly to the sequence root's server Stop.
/// Other outputs (including selection/load callbacks) are deliberately not
/// completion signals. The existing composition reader filters client-only rows.
pub fn direct_stop_signals(
    index: &mut AssetIndex,
    name: &str,
) -> Result<Vec<nfs_world::scoring::event_sync::Binding>, Error> {
    let composition = index.scoring_events(name)?;
    let signals = index.scoring_event_sync(name, &composition)?;
    stop_signals(&composition, &signals)
}
fn stop_signals(
    composition: &crate::scoring::events::Composition,
    signals: &[nfs_world::scoring::event_sync::Binding],
) -> Result<Vec<nfs_world::scoring::event_sync::Binding>, Error> {
    use nfs_world::scoring::{events::Delivery, nodes::OUT};
    const STOP: u32 = 2_089_401_213;
    let roots: Vec<_> = composition
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.class == "ParticipantStateSequenceEntityData")
        .map(|(i, _)| i)
        .collect();
    let [root] = roots.as_slice() else {
        return Err(bad("sequence completion root"));
    };
    let mut out = Vec::new();
    for signal in signals {
        if composition.connections.iter().any(|c| {
            c.source.node == signal.node
                && c.source.event == OUT
                && usize::from(c.target.node) == *root
                && c.target.event == STOP
                && matches!(
                    c.delivery,
                    Delivery::Local | Delivery::NetworkedClientAndServer(_)
                )
        }) {
            out.push(*signal);
        }
    }
    Ok(out)
}

fn bindings<S: Source>(
    graph: &mut Graph<S>,
    branch: &[Node],
    sequence_nodes: &[Node],
    sequence_name: &str,
    rules: &crate::sequences::Rules,
    parent_bindings: &BTreeMap<Site, Site>,
    reference: Option<&crate::scoring::events::Identity>,
) -> Result<BTreeMap<Site, Site>, Error> {
    let mut referers = Vec::new();
    for node in branch
        .iter()
        .filter(|n| n.object.class == "StateSequenceRefererEntityData")
    {
        if reference.is_none_or(|r| *r == crate::scoring::events::Identity::from(&node.site))
            && graph.target_field(&node.site, "Sequence")? == (sequence_name.into(), 0)
        {
            referers.push(node.site.clone());
        }
    }
    let [referer] = referers.as_slice() else {
        return Err(bad("sequence referer occurrence"));
    };
    let root = unique(sequence_nodes, "ParticipantStateSequenceEntityData")?;
    let references = graph.links(root, rules.root_channels)?;
    let mut targets = Vec::new();
    for endpoint in graph.links(referer, rules.spawner_channels)? {
        if endpoint.port == 0 {
            targets.push(endpoint);
        } else {
            let bound = parent_bindings
                .get(&endpoint.site)
                .ok_or_else(|| bad("sequence parent channel"))?;
            for end in graph.links(bound, endpoint.port)? {
                if end.port != 0 {
                    return Err(bad("sequence channel chain depth"));
                }
                targets.push(end);
            }
        }
    }
    crate::participant_logic::bind(graph, sequence_nodes, references, targets)
}

/// Resolve a sequence only through its unique referer in the intro branch.
/// The same channel asset in another prefab or scene is never a candidate.
pub fn extract(index: &mut AssetIndex, name: &str) -> Result<Sequence, Error> {
    Ok(extract_bound(index, name, false, None)?.0)
}

/// Reused sequences bind through the exact referer selected by the story graph.
/// A missing or foreign occurrence never falls back to another matching asset.
pub fn extract_referenced(
    index: &mut AssetIndex,
    name: &str,
    reference: &crate::scoring::events::Identity,
) -> Result<Sequence, Error> {
    Ok(extract_bound(index, name, false, Some(reference))?.0)
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServiceLink {
    /// None is an explicitly identified, non-replicated node in the parent
    /// participant graph. It is not a missing wire endpoint.
    pub target: Option<Channel>,
    pub identity: crate::scoring::events::Identity,
    pub port: u32,
    pub class: String,
    pub definition: Option<[u8; 16]>,
    pub relay: Option<Relay>,
}
type ServiceLinks = BTreeMap<crate::scoring::events::Identity, BTreeMap<u32, ServiceLink>>;

fn extract_bound(
    index: &mut AssetIndex,
    name: &str,
    include_services: bool,
    selected: Option<&crate::scoring::events::Identity>,
) -> Result<(Sequence, ServiceLinks), Error> {
    if name.is_empty() || name.len() > 512 || name.contains("..") {
        return Err(bad("sequence name"));
    }
    let name = name.to_lowercase();
    let progression = index.identity.progression.clone();
    let sequence_rules = index.identity.sequences.clone();
    let scene_rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&scene_rules.level)?;
    let mut graph = Graph::new(&mut index.assets);
    let mut scenes = SceneLookup {
        roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
        rules: &scene_rules,
        endpoints: BTreeMap::new(),
        loaded: BTreeSet::new(),
    };
    let branch = graph.read(Site::root(&progression.intro))?;
    let scene = scenes
        .roots
        .get(&progression.scene)
        .ok_or_else(|| bad("progression scene"))?
        .clone();
    let scene_nodes = graph.read(scene)?;
    let mut spawners = Vec::new();
    for n in scene_nodes
        .iter()
        .filter(|n| n.object.class == "ParticipantLogicSpawnerEntityData")
    {
        if graph.target_field(&n.site, "ParticipantLogicBlueprint")?
            == (progression.intro.clone(), 0)
        {
            spawners.push(n.site.clone());
        }
    }
    let [spawner] = spawners.as_slice() else {
        return Err(bad("intro spawner occurrence"));
    };
    let references = graph.links(
        unique(&branch, "ParticipantLogicEntityData")?,
        progression.ports["root_channels"],
    )?;
    let targets = graph.links(spawner, progression.ports["spawner_channels"])?;
    let parent_bindings = crate::participant_logic::bind(&mut graph, &branch, references, targets)?;
    let mut parent_fields = BTreeMap::new();
    for n in &branch {
        let role = progression
            .classes
            .get(&n.object.class)
            .ok_or_else(|| bad("intro serializer class"))?;
        if role.is_some() {
            parent_fields.insert(n.site.clone(), parent_fields.len());
        }
    }
    let sequence_nodes = graph.read(Site::root(&name))?;
    let (bus_count, fields) = crate::sequences::authored::shape(&sequence_nodes, &sequence_rules)?;
    let (reference_nodes, registry) = scope::resolve(
        &mut graph,
        &branch,
        &scenes.roots,
        selected,
        &progression.scene,
    )?;
    let bound = bindings(
        &mut graph,
        &reference_nodes,
        &sequence_nodes,
        &name,
        &sequence_rules,
        &parent_bindings,
        selected,
    )?;
    let mut channels = BTreeMap::new();
    for (i, (site, _, kind)) in fields.iter().enumerate() {
        if *kind != Kind::RpcReference {
            continue;
        }
        let target = bound
            .get(site)
            .ok_or_else(|| bad("sequence channel binding"))?;
        let channel = if let Some(serializer) = parent_fields.get(target) {
            Channel::Participant {
                serializer: *serializer,
            }
        } else {
            let (content_key, serializer) = scenes.endpoint(&mut graph, target)?;
            Channel::Scene {
                content_key,
                serializer,
            }
        };
        channels.insert(i, channel);
    }
    let mut service_links = BTreeMap::new();
    for node in &sequence_nodes {
        if !include_services {
            break;
        }
        let ports: &[u32] = match node.object.class.as_str() {
            "AwardItemTransactionEntityData" => &[2_612_322_229],
            "ItemSelectorEntityData" => &[3_339_738_264, 2_356_970_361],
            "VehicleSlotLoaderEntityData" => &[926_267_033],
            "ParticipantEventEntityData" => &[3_253_352_650],
            "ParticipantProviderStateReporterEntityData" => &[230_748_402],
            _ => continue,
        };
        let mut links = BTreeMap::new();
        for &port in ports {
            let mut ends = graph.links(&node.site, port)?;
            let mut visited = BTreeSet::new();
            let end = loop {
                let [end] = ends.as_slice() else {
                    return Err(bad(&format!(
                        "sequence service link cardinality {} {:?} port {port}: {ends:?}",
                        node.object.class, node.site
                    )));
                };
                if !visited.insert(end.clone()) || visited.len() > 64 {
                    return Err(bad("sequence service channel cycle"));
                }
                if graph.object(&end.site)?.class != "LinkedSchematicChannelReferenceEntityData" {
                    break end.clone();
                }
                let target = bound
                    .get(&end.site)
                    .or_else(|| parent_bindings.get(&end.site))
                    .ok_or_else(|| bad("unbound sequence service channel"))?;
                ends = graph.links(target, end.port)?;
            };
            let object = graph.object(&end.site)?;
            let target = if let Some(serializer) = parent_fields.get(&end.site) {
                Some(Channel::Participant {
                    serializer: *serializer,
                })
            } else if branch.iter().any(|n| n.site == end.site) {
                None
            } else {
                let (content_key, serializer) =
                    scenes.endpoint(&mut graph, &end.site).map_err(|e| {
                        bad(&format!(
                            "service {} port {port} target {:?} ({}) : {e}",
                            node.object.class, end, object.class
                        ))
                    })?;
                Some(Channel::Scene {
                    content_key,
                    serializer,
                })
            };
            let definition = if object.class == "ItemEntityData" {
                let pointer = object
                    .fields
                    .get("Item")
                    .and_then(Value::as_pointer)
                    .ok_or_else(|| bad("sequence item pointer"))?;
                if let Some(key) = graph.source.target(&end.site.key.0, pointer)? {
                    Some(
                        graph
                            .source
                            .object(&key)?
                            .guid
                            .filter(|g| *g != [0; 16])
                            .ok_or_else(|| bad("sequence source item definition"))?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            let relay = if object.class == "ParticipantEventRelayEntityData" {
                let event = graph.target_field(&node.site, "EventAsset")?;
                let resolved = parent::read(
                    &mut graph,
                    &end.site,
                    &event,
                    progression.ports["stable_id"],
                )?;
                let mut slot = resolved.slot;
                let mut visited = BTreeSet::new();
                while graph.object(&slot.site)?.class == "LinkedSchematicChannelReferenceEntityData"
                {
                    if !visited.insert(slot.clone()) || visited.len() > 64 {
                        return Err(bad("parent slot channel cycle"));
                    }
                    let target = parent_bindings
                        .get(&slot.site)
                        .ok_or_else(|| bad("parent slot binding"))?;
                    let ends = graph.links(target, slot.port)?;
                    let [end] = ends.as_slice() else {
                        return Err(bad("parent slot cardinality"));
                    };
                    slot = end.clone();
                }
                if graph.object(&slot.site)?.class != "VehicleSlotEntityData" {
                    return Err(bad("parent vehicle slot class"));
                }
                let (content_key, serializer) = scenes.endpoint(&mut graph, &slot.site)?;
                Some(Relay {
                    objective: resolved.objective,
                    selected: resolved.selected,
                    watched: resolved.watched,
                    primary_slot: Channel::Scene {
                        content_key,
                        serializer,
                    },
                })
            } else {
                None
            };
            links.insert(
                port,
                ServiceLink {
                    target,
                    identity: crate::scoring::events::Identity::from(&end.site),
                    port: end.port,
                    class: object.class,
                    definition,
                    relay,
                },
            );
        }
        service_links.insert(crate::scoring::events::Identity::from(&node.site), links);
    }
    let manager_nodes = graph.read(
        scenes
            .roots
            .get(&sequence_rules.manager_scene)
            .ok_or_else(|| bad("sequence manager scene"))?
            .clone(),
    )?;
    let manager = unique(&manager_nodes, "StateSequenceManagerEntityData")?;
    let embedded_bus = scenes.embedded_bus(&mut graph, manager)?;
    let (scene_key, manager_index) = scenes.endpoint(&mut graph, manager)?;
    let bundle = startup
        .iter()
        .position(|e| e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&registry))
        .and_then(|i| u16::try_from(i + 1).ok())
        .ok_or_else(|| bad("sequence bundle"))?;
    let reference = index.reference(&registry, &name)?;
    let asset = Asset {
        bundle,
        type_id: u16::try_from(reference.class_id).map_err(|_| bad("sequence class ID"))?,
        local_index: u16::try_from(reference.local_index).map_err(|_| bad("sequence ordinal"))?,
    };
    let catalog = index.catalog(&registry)?;
    let sequence = Sequence {
        blueprint: name,
        asset,
        catalog,
        manager: Component {
            scene_key,
            index: manager_index,
        },
        embedded_bus,
        bus_count,
        fields: fields.into_iter().map(|(_, _, kind)| kind).collect(),
        channels,
    };
    sequence.profile()?;
    Ok((sequence, service_links))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};

    #[test]
    fn stop_callbacks_require_the_direct_server_root_edge() {
        use crate::scoring::events::{Composition, Identity, Node};
        use nfs_world::scoring::{
            event_sync::Binding,
            events::{Connection, Delivery, Endpoint, Remote},
            nodes::OUT,
        };
        let nodes = [
            "ParticipantStateSequenceEntityData",
            "EventSyncEntityData",
            "EventSyncEntityData",
            "EventSyncEntityData",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, class)| Node {
            identity: Identity {
                asset: "a".into(),
                index,
                instances: vec![],
            },
            class: class.into(),
            listeners: None,
            forward: None,
        })
        .collect();
        let mut composition = Composition {
            nodes,
            buses: vec![],
            bus_entities: vec![],
            connections: vec![
                Connection {
                    source: Endpoint {
                        node: 1,
                        event: OUT,
                    },
                    target: Endpoint {
                        node: 0,
                        event: 2_089_401_213,
                    },
                    delivery: Delivery::Local,
                },
                Connection {
                    source: Endpoint {
                        node: 2,
                        event: OUT,
                    },
                    target: Endpoint {
                        node: 0,
                        event: 2_089_401_213,
                    },
                    delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 0 }),
                },
                Connection {
                    source: Endpoint {
                        node: 3,
                        event: OUT,
                    },
                    target: Endpoint {
                        node: 1,
                        event: 2_089_401_213,
                    },
                    delivery: Delivery::Local,
                },
            ],
            properties: vec![],
            links: vec![],
        };
        let signals = (1..=3)
            .map(|node| Binding {
                entity: 1,
                event: u32::from(node) + 10,
                node,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stop_signals(&composition, &signals).unwrap(),
            vec![signals[0]]
        );
        composition.nodes[0].class = "Other".into();
        assert!(stop_signals(&composition, &signals).is_err());
    }

    fn fixture(duplicate: bool, missing: bool) -> (Memory, crate::sequences::Rules) {
        let rules = crate::BuildIdentity::supported().sequences;
        let mut memory = Memory::default();
        memory.put("c", 0, "LinkedSchematicChannelAsset", &[]);
        let mut objects = vec![local(1), local(2), local(3), local(5)];
        if duplicate {
            objects.push(local(4));
        }
        let mut links = vec![edge(local(1), rules.spawner_channels, local(2), 0)];
        if !missing {
            links.push(edge(local(1), rules.spawner_channels, local(3), 0));
        }
        memory.put(
            "a",
            0,
            "ParticipantLogicBlueprint",
            &[
                ("Objects", Value::Array(objects)),
                ("LinkConnections", Value::Array(links)),
            ],
        );
        for i in [1, 4] {
            memory.put(
                "a",
                i,
                "StateSequenceRefererEntityData",
                &[("Sequence", ptr("b", 0))],
            );
        }
        // The third matching channel is deliberately not linked to the referer.
        for i in [2, 3, 5] {
            memory.put(
                "a",
                i,
                "LinkedSchematicChannelEntityData",
                &[("ChannelAsset", ptr("c", 0))],
            );
        }
        memory.put(
            "b",
            0,
            "ParticipantStateSequenceBlueprint",
            &[
                ("Flags", Value::UInt16(1)),
                ("Objects", Value::Array((1..=4).map(local).collect())),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(local(1), rules.root_channels, local(2), 0),
                        edge(local(1), rules.root_channels, local(4), 0),
                    ]),
                ),
            ],
        );
        memory.put("b", 1, "ParticipantStateSequenceEntityData", &[]);
        memory.put("b", 3, "StateSequenceParticipantProviderEntityData", &[]);
        for i in [2, 4] {
            memory.put(
                "b",
                i,
                "LinkedSchematicChannelReferenceEntityData",
                &[("ChannelAsset", ptr("c", 0))],
            );
        }
        (memory, rules)
    }

    #[test]
    fn duplicate_channel_assets_bind_in_referer_order_and_ignore_unlinked_matches() {
        let (mut memory, rules) = fixture(false, false);
        let mut graph = Graph::new(&mut memory);
        let owner = graph.read(Site::root("a")).unwrap();
        let sequence = graph.read(Site::root("b")).unwrap();
        let bound = bindings(
            &mut graph,
            &owner,
            &sequence,
            "b",
            &rules,
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        assert_eq!(bound.len(), 2);
        for i in [2, 4] {
            let site = &sequence
                .iter()
                .find(|n| n.site.key == ("b".into(), i))
                .unwrap()
                .site;
            assert_eq!(bound[site].key, ("a".into(), if i == 2 { 2 } else { 3 }));
        }
    }

    #[test]
    fn shape_keeps_channel_before_provider_and_rejects_unknown_classes() {
        let (mut memory, rules) = fixture(false, false);
        let mut graph = Graph::new(&mut memory);
        let mut nodes = graph.read(Site::root("b")).unwrap();
        let (buses, fields) = crate::sequences::authored::shape(&nodes, &rules).unwrap();
        assert_eq!(buses, 1);
        assert_eq!(
            fields.iter().map(|(_, _, kind)| *kind).collect::<Vec<_>>(),
            [
                Kind::SequenceRoot,
                Kind::RpcReference,
                Kind::RpcOptionalReference,
                Kind::RpcReference,
            ]
        );
        nodes.last_mut().unwrap().object.class = "UnknownSerializer".into();
        assert!(crate::sequences::authored::shape(&nodes, &rules).is_err());
    }

    #[test]
    fn missing_channel_or_ambiguous_referer_is_rejected() {
        for (duplicate, missing) in [(true, false), (false, true)] {
            let (mut memory, rules) = fixture(duplicate, missing);
            let mut graph = Graph::new(&mut memory);
            let owner = graph.read(Site::root("a")).unwrap();
            let sequence = graph.read(Site::root("b")).unwrap();
            assert!(
                bindings(
                    &mut graph,
                    &owner,
                    &sequence,
                    "b",
                    &rules,
                    &BTreeMap::new(),
                    None
                )
                .is_err()
            );
        }
    }

    #[test]
    fn explicit_referer_keeps_its_channels_and_rejects_foreign_or_missing_occurrences() {
        let (mut memory, rules) = fixture(true, false);
        let mut graph = Graph::new(&mut memory);
        let owner = graph.read(Site::root("a")).unwrap();
        let sequence = graph.read(Site::root("b")).unwrap();
        let selected = crate::scoring::events::Identity::from(
            &owner
                .iter()
                .find(|n| n.site.key == ("a".into(), 1))
                .unwrap()
                .site,
        );
        let bound = bindings(
            &mut graph,
            &owner,
            &sequence,
            "b",
            &rules,
            &BTreeMap::new(),
            Some(&selected),
        )
        .unwrap();
        assert_eq!(bound.len(), 2);
        let mut invalid = selected.clone();
        invalid.index = 4;
        assert!(
            bindings(
                &mut graph,
                &owner,
                &sequence,
                "b",
                &rules,
                &BTreeMap::new(),
                Some(&invalid)
            )
            .is_err()
        );
        invalid.asset = "foreign".into();
        assert!(
            bindings(
                &mut graph,
                &owner,
                &sequence,
                "b",
                &rules,
                &BTreeMap::new(),
                Some(&invalid)
            )
            .is_err()
        );
        invalid = selected.clone();
        invalid.instances.push(("other-parent".into(), 1));
        assert!(
            bindings(
                &mut graph,
                &owner,
                &sequence,
                "b",
                &rules,
                &BTreeMap::new(),
                Some(&invalid)
            )
            .is_err()
        );
    }
}
