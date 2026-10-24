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

#[derive(Clone, Debug, Eq, PartialEq)]
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

fn bindings<S: Source>(
    graph: &mut Graph<S>,
    branch: &[Node],
    sequence_nodes: &[Node],
    sequence_name: &str,
    rules: &crate::sequences::Rules,
    parent_bindings: &BTreeMap<Site, Site>,
) -> Result<BTreeMap<Site, Site>, Error> {
    let mut referers = Vec::new();
    for node in branch
        .iter()
        .filter(|n| n.object.class == "StateSequenceRefererEntityData")
    {
        if graph.target_field(&node.site, "Sequence")? == (sequence_name.into(), 0) {
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
    if name.is_empty() || name.len() > 512 || name.contains("..") {
        return Err(bad("sequence name"));
    }
    let name = name.to_lowercase();
    let progression = index.identity.progression.clone();
    let sequence_rules = index.identity.sequences.clone();
    let scene_rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&scene_rules.level)?;
    let bundle = startup
        .iter()
        .position(|e| {
            e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&progression.scene)
        })
        .and_then(|i| u16::try_from(i + 1).ok())
        .ok_or_else(|| bad("sequence bundle"))?;
    let reference = index.reference(&progression.scene, &name)?;
    let asset = Asset {
        bundle,
        type_id: u16::try_from(reference.class_id).map_err(|_| bad("sequence class ID"))?,
        local_index: u16::try_from(reference.local_index).map_err(|_| bad("sequence ordinal"))?,
    };
    let catalog = index.catalog(&progression.scene)?;
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
    let bound = bindings(
        &mut graph,
        &branch,
        &sequence_nodes,
        &name,
        &sequence_rules,
        &parent_bindings,
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
    let manager_nodes = graph.read(
        scenes
            .roots
            .get(&sequence_rules.manager_scene)
            .ok_or_else(|| bad("sequence manager scene"))?
            .clone(),
    )?;
    let manager = unique(&manager_nodes, "StateSequenceManagerEntityData")?;
    let embedded_bus = scenes.embedded_bus(&mut graph, manager)?;
    let (scene_key, index) = scenes.endpoint(&mut graph, manager)?;
    let sequence = Sequence {
        blueprint: name,
        asset,
        catalog,
        manager: Component { scene_key, index },
        embedded_bus,
        bus_count,
        fields: fields.into_iter().map(|(_, _, kind)| kind).collect(),
        channels,
    };
    sequence.profile()?;
    Ok(sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};

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
        let bound = bindings(&mut graph, &owner, &sequence, "b", &rules, &BTreeMap::new()).unwrap();
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
                bindings(&mut graph, &owner, &sequence, "b", &rules, &BTreeMap::new()).is_err()
            );
        }
    }
}
