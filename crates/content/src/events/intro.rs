// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Modes launched by the intro participant. Reuse event topology and serializer
//! rules without inventing a free-roam start area, rewards or DNF controller.
use super::*;
mod spawned;

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub game_mode: GameMode,
    pub sequences: Vec<Sequence>,
    pub logics: Vec<Layout>,
    /// Serializer of the launcher in the owning intro participant logic.
    pub launcher: usize,
}

fn unique<'a>(nodes: &'a [Node], class: &str) -> Result<&'a Site, Error> {
    let mut found = nodes.iter().filter(|n| n.object.class == class);
    let site = &found
        .next()
        .ok_or_else(|| bad(&format!("intro mode missing {class}")))?
        .site;
    if found.next().is_some() {
        return Err(bad("intro mode ambiguous class"));
    }
    Ok(site)
}

fn launcher<S: Source>(
    graph: &mut Graph<S>,
    branch: &[Node],
    name: &str,
    asset: Option<&str>,
) -> Result<Site, Error> {
    let mut found = vec![];
    for n in branch.iter().filter(|n| {
        n.object.class == LAUNCHER && asset.is_none_or(|a| n.site.key.0.eq_ignore_ascii_case(a))
    }) {
        if graph.target_field(&n.site, "GameMode")? == (name.into(), 0) {
            found.push(n.site.clone());
        }
    }
    match found.as_slice() {
        [site] => Ok(site.clone()),
        _ => Err(bad("intro mode launcher occurrence")),
    }
}

/// Follow declared channel ports across the intro's own scene bindings. Direct
/// channel references remain aliases for replication; only link lookup follows
/// their definitions. Repeated occurrences and source order are retained.
pub(super) fn links<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    aliases: &BTreeMap<Site, Site>,
    active: &mut BTreeSet<(Site, u32)>,
) -> Result<Vec<crate::graph::Endpoint>, Error> {
    if active.len() >= 64 || !active.insert((site.clone(), port)) {
        return Err(bad("intro channel link cycle"));
    }
    let actual = aliases.get(site).unwrap_or(site);
    let mut out = vec![];
    for end in graph.links(actual, port)? {
        if end.port == 0 {
            out.push(end);
        } else {
            out.extend(links(graph, &end.site, end.port, aliases, active)?);
        }
        if out.len() > 512 {
            return Err(bad("intro channel link bound"));
        }
    }
    active.remove(&(site.clone(), port));
    Ok(out)
}

/// Select a unique launcher by its authored mode reference. An optional declaring
/// prefab distinguishes the two intro races that intentionally reuse one mode.
pub fn extract(
    index: &mut AssetIndex,
    name: &str,
    launcher_asset: Option<&str>,
) -> Result<Mode, Error> {
    let name = name.to_lowercase();
    if name.is_empty() || name.len() > 512 || name.contains("..") {
        return Err(bad("intro mode name"));
    }
    let progression = index.identity.progression.clone();
    let scene_rules = index.identity.scenes.clone();
    let rules = index.identity.participant_logic.clone();
    let sequence_rules = index.identity.sequences.clone();
    let manager_scene = sequence_rules.manager_scene.clone();
    let startup = index.startup_entries(&scene_rules.level)?;
    let bundle = startup
        .iter()
        .position(|e| {
            e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&progression.scene)
        })
        .and_then(|i| u16::try_from(i + 1).ok())
        .ok_or_else(|| bad("intro mode bundle"))?;
    let reference = index.reference(&progression.scene, &name)?;
    let catalog = index.catalog(&progression.scene)?;
    let mut graph = Graph::new(&mut index.assets);
    let mut scenes = SceneLookup {
        roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
        rules: &scene_rules,
        endpoints: BTreeMap::new(),
        loaded: BTreeSet::new(),
    };
    let branch = graph.read(Site::root(&progression.intro))?;
    let mut parent_fields = BTreeMap::new();
    for n in &branch {
        if progression
            .classes
            .get(&n.object.class)
            .ok_or_else(|| bad("intro mode parent class"))?
            .is_some()
        {
            parent_fields.insert(n.site.clone(), parent_fields.len());
        }
    }
    let launcher = launcher(&mut graph, &branch, &name, launcher_asset)?;
    let scene_nodes = graph.read(
        scenes
            .roots
            .get(&progression.scene)
            .ok_or_else(|| bad("intro mode scene"))?
            .clone(),
    )?;
    let mut spawners = vec![];
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
        return Err(bad("intro participant spawner occurrence"));
    };
    let refs = graph.links(
        unique(&branch, "ParticipantLogicEntityData")?,
        progression.ports["root_channels"],
    )?;
    let targets = graph.links(spawner, progression.ports["spawner_channels"])?;
    let aliases = crate::participant_logic::bind(&mut graph, &branch, refs, targets)?;
    let nodes = graph.read(Site::root(&name))?;
    let (bus_count, field_sites) = fields(&nodes, &rules, "GameModeEntityData")?;
    let channels: Vec<_> = field_sites
        .iter()
        .filter(|(_, _, k)| *k == Kind::RpcReference)
        .map(|(s, c, _)| (s.clone(), c.clone()))
        .collect();
    let targets = links(
        &mut graph,
        &launcher,
        rules.spawner_channels,
        &aliases,
        &mut BTreeSet::new(),
    )?;
    let bound = bind_targets(&mut graph, &channels, &field_sites[0].0, &rules, &targets)?;
    let mut wire = BTreeMap::new();
    let mut logical = BTreeMap::new();
    for ((source, _), target) in channels.iter().zip(bound) {
        let binding = if let Some(&serializer) = parent_fields.get(&target) {
            Binding::Participant { serializer }
        } else {
            let (content_key, serializer) = scenes.endpoint(&mut graph, &target)?;
            Binding::Scene {
                content_key,
                serializer,
            }
        };
        wire.insert(source.clone(), binding);
        let definition = aliases.get(&target).unwrap_or(&target).clone();
        let owner = if definition == target {
            binding
        } else {
            let (content_key, serializer) = scenes.endpoint(&mut graph, &definition)?;
            Binding::Scene {
                content_key,
                serializer,
            }
        };
        logical.insert(source.clone(), (definition, owner));
    }
    // Keep parent aliases available when nested links traverse a second channel.
    for (source, target) in &aliases {
        let (content_key, serializer) = scenes.endpoint(&mut graph, target)?;
        logical.insert(
            source.clone(),
            (
                target.clone(),
                Binding::Scene {
                    content_key,
                    serializer,
                },
            ),
        );
    }
    let mut lifecycle = lifecycle::extract(&mut graph, &field_sites)?;
    lifecycle.bindings.channels = lifecycle::channels::read(
        &mut graph,
        &logical
            .iter()
            .filter(|(s, _)| wire.contains_key(*s))
            .map(|(s, t)| (s.clone(), t.clone()))
            .collect(),
    )?;
    lifecycle::resolve_configurations(&mut lifecycle, &mut graph, &field_sites, &logical)?;
    lifecycle::resolve_imports(
        &mut lifecycle,
        &mut graph,
        &field_sites,
        &logical,
        |g, s| scenes.endpoint(g, s),
    )?;
    let mut checkpoints = vec![];
    let mut points = vec![];
    let mut checkpoint_points = vec![];
    let waypoint_channels: Vec<_> = channels
        .iter()
        .filter(|(_, c)| c == "WaypointListChannelReferenceEntityData")
        .collect();
    let laps = match waypoint_channels.as_slice() {
        [] => 1,
        [(site, _)] => {
            let channel = &logical
                .get(site)
                .ok_or_else(|| bad("intro waypoint binding"))?
                .0;
            let laps = route::laps(&mut graph, channel)?;
            let port = one_port(&mut graph, channel)?;
            for end in links(&mut graph, channel, port, &aliases, &mut BTreeSet::new())? {
                if graph.object(&end.site)?.class != CHECKPOINT {
                    return Err(bad("intro waypoint class"));
                }
                checkpoints.push(scenes.endpoint(&mut graph, &end.site)?);
                points.push(route::point(&mut graph, &end.site)?);
                checkpoint_points.push(route::checkpoint(&mut graph, &end.site)?);
            }
            if checkpoints.is_empty()
                || checkpoints.len() > MAX_CHECKPOINTS
                || checkpoints.iter().collect::<BTreeSet<_>>().len() != checkpoints.len()
            {
                return Err(bad("intro waypoint route bound"));
            }
            laps
        }
        _ => return Err(bad("intro waypoint channels")),
    };
    let managers = graph.read(
        scenes
            .roots
            .get(&manager_scene)
            .ok_or_else(|| bad("intro manager scene"))?
            .clone(),
    )?;
    let mut root_bindings = [(0, 0); 3];
    for (slot, class) in MANAGER_CLASSES.iter().enumerate() {
        root_bindings[slot] = scenes.endpoint(&mut graph, unique(&managers, class)?)?;
    }
    let sub_id = embedded_bus(&managers, unique(&managers, MANAGER_CLASSES[1])?)?;
    let (mut sequences, mut logics) = spawned::extract(
        &mut graph,
        &mut scenes,
        spawned::Context {
            mode_nodes: &nodes,
            mode_fields: &field_sites,
            parent_fields: &parent_fields,
            aliases: &aliases,
            bound: &logical,
            rules: &rules,
            sequences: &sequence_rules,
            lifecycle: &lifecycle,
        },
    )?;
    let recovery = recovery::extract(&nodes)?;
    let drift = route::drift(&nodes)?;
    let pacing = pacing::extract(&mut graph, &nodes, &logical)?;
    let standings_interval = standings::interval(&mut graph, &nodes)?;
    let fields = field_sites
        .iter()
        .map(|(s, c, k)| {
            Ok(Field {
                kind: *k,
                class: c.clone(),
                asset: s.key.0.clone(),
                object: s.key.1,
                binding: wire.get(s).copied(),
                mirrors: mirrored(&mut graph, s)?,
            })
        })
        .collect::<Result<_, Error>>()?;
    drop(graph);
    for child in logics
        .iter_mut()
        .chain(sequences.iter_mut().map(|s| &mut s.layout))
    {
        let r = index.reference(&progression.scene, &child.blueprint)?;
        child.asset = Asset {
            bundle,
            type_id: u16::try_from(r.class_id).map_err(|_| bad("intro child class"))?,
            local_index: u16::try_from(r.local_index).map_err(|_| bad("intro child ordinal"))?,
        };
        child.catalog = catalog.clone();
        child.profile()?;
    }
    let layout = Layout {
        blueprint: name,
        asset: Asset {
            bundle,
            type_id: u16::try_from(reference.class_id).map_err(|_| bad("intro mode class"))?,
            local_index: u16::try_from(reference.local_index)
                .map_err(|_| bad("intro mode ordinal"))?,
        },
        catalog,
        bus_count,
        sub_id,
        occurrence: 0,
        fields,
    };
    layout.profile()?;
    Ok(Mode {
        sequences,
        logics,
        launcher: *parent_fields
            .get(&launcher)
            .ok_or_else(|| bad("intro launcher serializer"))?,
        game_mode: GameMode {
            lifecycle,
            recovery,
            layout,
            root_bindings,
            checkpoints,
            points,
            checkpoint_points,
            drift,
            pacing,
            standings_interval,
            laps,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    fn site(asset: &str, i: usize) -> Site {
        Site {
            key: (asset.into(), i),
            ..Site::root(asset)
        }
    }

    #[test]
    fn launcher_occurrence_is_scoped_and_ambiguous_modes_are_rejected() {
        let mut m = Memory::default();
        m.put("m", 0, "GameModeBlueprint", &[]);
        let mut nodes = vec![];
        for asset in ["a", "b"] {
            m.put(asset, 1, LAUNCHER, &[("GameMode", ptr("m", 0))]);
            nodes.push(Node {
                site: site(asset, 1),
                object: m.object(&(asset.into(), 1)).unwrap(),
            });
        }
        let mut g = Graph::new(&mut m);
        assert!(launcher(&mut g, &nodes, "m", None).is_err());
        assert!(launcher(&mut g, &nodes, "missing", Some("a")).is_err());
        assert_eq!(
            launcher(&mut g, &nodes, "m", Some("a")).unwrap(),
            site("a", 1)
        );
    }

    #[test]
    fn channel_alias_links_preserve_order_and_reject_cycles() {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 10, local(2), 20),
                    edge(local(1), 10, local(5), 0),
                    edge(local(3), 20, local(4), 0),
                    edge(local(6), 30, local(2), 30),
                ]),
            )],
        );
        for i in 1..=6 {
            m.put("a", i, "LinkedSchematicChannelEntityData", &[]);
        }
        m.put("a", 2, "LinkedSchematicChannelReferenceEntityData", &[]);
        let aliases = BTreeMap::from([(site("a", 2), site("a", 3))]);
        let mut g = Graph::new(&mut m);
        assert_eq!(
            links(&mut g, &site("a", 1), 10, &aliases, &mut BTreeSet::new())
                .unwrap()
                .iter()
                .map(|e| e.site.key.1)
                .collect::<Vec<_>>(),
            [4, 5]
        );
        let cycle = BTreeMap::from([(site("a", 2), site("a", 6))]);
        assert!(links(&mut g, &site("a", 6), 30, &cycle, &mut BTreeSet::new()).is_err());
    }

    #[test]
    fn wire_binding_keeps_parent_reference_and_uses_each_occurrence_once() {
        let mut m = Memory::default();
        m.put("s", 0, "SchematicChannelAsset", &[]);
        let rules = crate::BuildIdentity::supported().participant_logic;
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), rules.root_channels, local(2), 0),
                    edge(local(1), rules.root_channels, local(3), 0),
                ]),
            )],
        );
        m.put("a", 1, "GameModeEntityData", &[]);
        for i in [2, 3, 4, 5] {
            m.put(
                "a",
                i,
                "LinkedSchematicChannelReferenceEntityData",
                &[("ChannelAsset", ptr("s", 0))],
            );
        }
        let refs = vec![
            (
                site("a", 2),
                "LinkedSchematicChannelReferenceEntityData".into(),
            ),
            (
                site("a", 3),
                "LinkedSchematicChannelReferenceEntityData".into(),
            ),
        ];
        let targets = vec![
            crate::graph::Endpoint {
                site: site("a", 5),
                port: 0,
            },
            crate::graph::Endpoint {
                site: site("a", 4),
                port: 0,
            },
        ];
        let mut g = Graph::new(&mut m);
        assert_eq!(
            bind_targets(&mut g, &refs, &site("a", 1), &rules, &targets).unwrap(),
            [site("a", 5), site("a", 4)]
        );
        assert!(bind_targets(&mut g, &refs, &site("a", 1), &rules, &targets[..1]).is_err());
    }
}
