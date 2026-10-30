// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

impl AssetIndex {
    /// Resolve the event-specific bundle, spawned logics and sequence table from
    /// the selected start area's authored bindings. Global scene services and
    /// explicit notification selections remain the caller's configuration.
    /// This resolves composition only; it does not establish playable support.
    pub fn event_request(&mut self, context: &Request) -> Result<Request, Error> {
        let startup = self.startup_entries(&self.identity.scenes.level.clone())?;
        let mut cursor = startup
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(&context.scene))
            .ok_or_else(|| bad("event scene is not loaded"))?;
        let mut bundle = None;
        for _ in 0..startup.len() {
            let entry = &startup[cursor];
            if entry.kind == StartupKind::SubLevel
                && self.reference(&entry.name, &context.game_mode).is_ok()
            {
                bundle = Some(entry.name.to_lowercase());
                break;
            }
            let Some(parent) = entry.parent else { break };
            if parent >= startup.len() {
                return Err(bad("event registry parent"));
            }
            cursor = parent;
        }
        let bundle = bundle.ok_or_else(|| bad("event mode has no loaded ancestor registry"))?;
        let rules = self.identity.participant_logic.clone();
        let mut graph = Graph::new(&mut self.assets);
        let roots = scene_roots(&mut graph, &self.identity.scenes.level, &startup)?;
        let root = roots
            .get(&context.scene.to_lowercase())
            .ok_or_else(|| bad("event scene root"))?
            .clone();
        let nodes = graph.read(root)?;
        let (launcher, instance) =
            start_area(&mut graph, &nodes, &context.game_mode_id.to_lowercase())?;
        if graph.target_field(&launcher, "GameMode")? != (context.game_mode.to_lowercase(), 0) {
            return Err(bad("start area launches another game mode"));
        }
        let mode_nodes = graph.read(Site::root(&context.game_mode.to_lowercase()))?;
        let mut request = context.clone();
        request.bundle = bundle;
        request.start_logics = spawns(&mut graph, &nodes, &rules, |s| within(s, &instance))?;
        request.game_mode_logics = spawns(&mut graph, &mode_nodes, &rules, |_| true)?;
        let mode_roots: Vec<_> = mode_nodes
            .iter()
            .filter(|n| n.object.class == "GameModeEntityData")
            .collect();
        let [mode_root] = mode_roots.as_slice() else {
            return Err(bad("event game-mode root count"));
        };
        let channels: Vec<_> = mode_nodes
            .iter()
            .filter(|n| channel_class(&n.object.class).is_some())
            .map(|n| (n.site.clone(), n.object.class.clone()))
            .collect();
        let targets = bind(&mut graph, &channels, &mode_root.site, &launcher, &rules)?;
        let tables: Vec<_> = channels
            .iter()
            .zip(targets)
            .filter(|((_, class), _)| class == "StateSequenceChannelReferenceEntityData")
            .map(|(_, target)| target)
            .collect();
        let [table] = tables.as_slice() else {
            return Err(bad("event state-sequence channel count"));
        };
        request.sequences = sequences(&mut graph, table)?;
        Ok(request)
    }
}

pub(super) fn start_area<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    id: &str,
) -> Result<(Site, Key), Error> {
    let mut details = Vec::new();
    for node in nodes.iter().filter(|n| n.object.class == DETAILS) {
        if graph.target_field(&node.site, "Id")? == (id.to_owned(), 0) {
            details.push(node.site.clone());
        }
    }
    let mut areas = Vec::new();
    for launcher in nodes.iter().filter(|n| n.object.class == LAUNCHER) {
        let Some(frame) = launcher.site.parents.last() else {
            continue;
        };
        let Ok(channel) = sibling(nodes, DETAILS_CHANNEL, &launcher.site) else {
            continue;
        };
        let port = one_port(graph, &channel.site)?;
        if graph
            .links(&channel.site, port)?
            .iter()
            .any(|e| details.contains(&e.site))
        {
            areas.push((launcher.site.clone(), frame.reference.clone()));
        }
    }
    let [area] =
        <[_; 1]>::try_from(areas).map_err(|_| bad("missing or ambiguous event start area"))?;
    Ok(area)
}

fn spawns<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    rules: &Rules,
    include: impl Fn(&Site) -> bool,
) -> Result<Vec<String>, Error> {
    let mut names = BTreeSet::new();
    for node in nodes
        .iter()
        .filter(|n| rules.spawners.contains(&n.object.class) && include(&n.site))
    {
        let (name, object) = graph.target_field(&node.site, "ParticipantLogicBlueprint")?;
        if object != 0
            || graph.source.object(&(name.clone(), object))?.class != "ParticipantLogicBlueprint"
        {
            return Err(bad("event logic spawner blueprint"));
        }
        names.insert(name);
        if names.len() > 128 {
            return Err(bad("event logic count"));
        }
    }
    Ok(names.into_iter().collect())
}

fn sequences<S: Source>(graph: &mut Graph<S>, table: &Site) -> Result<Vec<String>, Error> {
    let object = graph.object(table)?;
    let entries = object
        .fields
        .get("Sequences")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= 64)
        .ok_or_else(|| bad("state-sequence channel table"))?;
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for entry in entries {
        let fields = entry
            .as_fields()
            .ok_or_else(|| bad("state-sequence entry"))?;
        let mut target = |field: &str| -> Result<Key, Error> {
            let pointer = fields
                .get(field)
                .and_then(Value::as_pointer)
                .ok_or_else(|| bad("state-sequence entry pointer"))?;
            graph
                .source
                .target(&table.key.0, pointer)?
                .ok_or_else(|| bad("null state-sequence entry"))
        };
        let id = target("Id")?;
        let (name, object) = target("Sequence")?;
        if !ids.insert(id)
            || object != 0
            || !matches!(
                graph.source.object(&(name.clone(), object))?.class.as_str(),
                "ParticipantStateSequenceBlueprint" | "VehicleStateSequenceBlueprint"
            )
        {
            return Err(bad("duplicate sequence ID or invalid blueprint"));
        }
        names.insert(name);
    }
    Ok(names.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    use nfs_frostbite::ebx::Fields;

    fn node(m: &Memory, asset: &str, index: usize, instance: usize) -> Node {
        Node {
            site: Site {
                key: (asset.into(), index),
                bus: (asset.into(), 0),
                parents: vec![Frame {
                    bus: ("s".into(), 0),
                    reference: ("s".into(), instance),
                }],
            },
            object: m.0[&(asset.into(), index)].clone(),
        }
    }

    #[test]
    fn selected_area_follows_its_details_link_and_rejects_ambiguous_identity() {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "LogicPrefabBlueprint",
            &[(
                "LinkConnections",
                Value::Array(vec![edge(local(2), 7, local(3), 0)]),
            )],
        );
        m.put("a", 1, LAUNCHER, &[]);
        m.put("a", 2, DETAILS_CHANNEL, &[]);
        m.put("a", 3, DETAILS, &[("Id", ptr("i", 0))]);
        m.put("i", 0, "GameModeId", &[]);
        let nodes = vec![
            node(&m, "a", 1, 10),
            node(&m, "a", 2, 10),
            node(&m, "a", 3, 10),
        ];
        assert_eq!(
            start_area(&mut Graph::new(&mut m), &nodes, "i").unwrap().1,
            ("s".into(), 10)
        );
        assert!(start_area(&mut Graph::new(&mut m), &nodes, "j").is_err());
        let mut ambiguous = nodes.clone();
        ambiguous.extend([
            node(&m, "a", 1, 11),
            node(&m, "a", 2, 11),
            node(&m, "a", 3, 11),
        ]);
        assert!(start_area(&mut Graph::new(&mut m), &ambiguous, "i").is_err());
    }

    #[test]
    fn spawns_are_scoped_to_the_area_and_blueprint_names_are_unique() {
        let mut m = Memory::default();
        m.put(
            "a",
            1,
            "ParticipantLogicSpawnerEntityData",
            &[("ParticipantLogicBlueprint", ptr("b", 0))],
        );
        m.put(
            "a",
            2,
            "ParticipantLogicSpawnerSetControlledEntityData",
            &[("ParticipantLogicBlueprint", ptr("c", 0))],
        );
        for asset in ["b", "c"] {
            m.put(asset, 0, "ParticipantLogicBlueprint", &[]);
        }
        let nodes = vec![
            node(&m, "a", 1, 10),
            node(&m, "a", 1, 10),
            node(&m, "a", 2, 11),
        ];
        let rules = crate::BuildIdentity::supported().participant_logic;
        assert_eq!(
            spawns(&mut Graph::new(&mut m), &nodes, &rules, |s| within(
                s,
                &("s".into(), 10)
            ))
            .unwrap(),
            ["b"]
        );
        assert_eq!(
            spawns(&mut Graph::new(&mut m), &nodes, &rules, |_| true).unwrap(),
            ["b", "c"]
        );
        m.put("c", 0, "WrongBlueprint", &[]);
        assert!(spawns(&mut Graph::new(&mut m), &nodes, &rules, |_| true).is_err());
    }

    #[test]
    fn sequence_table_requires_unique_ids_valid_targets_and_bounded_entries() {
        let mut m = Memory::default();
        for asset in ["i", "j"] {
            m.put(asset, 0, "StateSequenceId", &[]);
        }
        m.put("b", 0, "ParticipantStateSequenceBlueprint", &[]);
        let entry = |id| {
            Value::Struct(Fields(vec![
                ("Id".into(), ptr(id, 0)),
                ("Sequence".into(), ptr("b", 0)),
            ]))
        };
        let table = |m: &mut Memory, entries| {
            m.put(
                "a",
                0,
                "StateSequenceChannelEntityData",
                &[("Sequences", Value::Array(entries))],
            )
        };
        table(&mut m, vec![entry("i"), entry("j")]);
        // Reusing one sequence for two IDs is valid; duplicate IDs are ambiguous.
        assert_eq!(
            sequences(&mut Graph::new(&mut m), &Site::root("a")).unwrap(),
            ["b"]
        );
        m.put("b", 0, "VehicleStateSequenceBlueprint", &[]);
        assert_eq!(
            sequences(&mut Graph::new(&mut m), &Site::root("a")).unwrap(),
            ["b"]
        );
        for entries in [vec![], vec![entry("i"); 2], vec![entry("i"); 65]] {
            table(&mut m, entries);
            assert!(sequences(&mut Graph::new(&mut m), &Site::root("a")).is_err());
        }
        table(&mut m, vec![entry("i")]);
        m.put("b", 0, "WrongBlueprint", &[]);
        assert!(sequences(&mut Graph::new(&mut m), &Site::root("a")).is_err());
    }
}
