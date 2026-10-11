// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Rules, SequenceContent};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{
        scenes::{SceneLookup, scene_roots},
        *,
    },
    scan::Assets,
    world::StartupKind,
};
use nfs_frostbite::ebx::Value;
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

fn unique<'a>(nodes: &'a [Node], class: &str) -> Result<&'a Site, Error> {
    let mut matches = nodes.iter().filter(|n| n.object.class == class);
    let site = &matches
        .next()
        .ok_or_else(|| bad(&format!("sequence missing {class}")))?
        .site;
    if matches.next().is_some() {
        return Err(bad(&format!("sequence has multiple {class}")));
    }
    Ok(site)
}
fn one(mut endpoints: Vec<Endpoint>) -> Result<Endpoint, Error> {
    if endpoints.len() != 1 {
        return Err(bad(&format!(
            "sequence requires one endpoint: {endpoints:?}"
        )));
    }
    Ok(endpoints.remove(0))
}
fn component(
    graph: &mut Graph<Assets>,
    scenes: &mut SceneLookup<'_>,
    site: &Site,
) -> Result<Json, Error> {
    let (_, index) = scenes.endpoint(graph, site)?;
    Ok(json!({"scene":scenes.scene_name(site)?,"component_index":index}))
}
fn profile(nodes: &[Node], rules: &Rules, layout: usize) -> Result<(), Error> {
    if nodes
        .first()
        .is_none_or(|n| n.object.class != "ParticipantStateSequenceBlueprint")
    {
        return Err(bad("sequence blueprint class"));
    }
    let mut roles = Vec::new();
    let unknown: BTreeSet<_> = nodes
        .iter()
        .filter(|n| !rules.classes.contains_key(&n.object.class))
        .map(|n| n.object.class.as_str())
        .collect();
    if !unknown.is_empty() {
        return Err(bad(&format!(
            "unknown sequence composition classes: {}",
            unknown.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    for node in nodes {
        let role = rules.classes.get(&node.object.class).ok_or_else(|| {
            bad(&format!(
                "unknown sequence composition class {}",
                node.object.class
            ))
        })?;
        if let Some(role) = role {
            roles.push(role.as_str());
        }
    }
    let expected: &[&str] = if layout == 2 {
        &["root", "provider", "event", "event", "channel", "channel"]
    } else if layout == 1 {
        &["root", "provider", "event", "event", "channel"]
    } else {
        &["root", "provider", "channel"]
    };
    if roles != expected {
        return Err(bad(&format!(
            "unsupported sequence serializer order: {roles:?}"
        )));
    }
    Ok(())
}
/// Resolve the sequence's unique channel through its particular scene spawner.
/// Matching a channel asset elsewhere in the world is deliberately insufficient.
fn binding(
    graph: &mut Graph<Assets>,
    scenes: &SceneLookup<'_>,
    rules: &Rules,
    nodes: &[Node],
    scene: &str,
    name: &str,
) -> Result<Vec<(Site, Site)>, Error> {
    let scene_root = scenes
        .roots
        .get(scene)
        .ok_or_else(|| bad("sequence spawner scene missing"))?;
    let scene_nodes = graph.read(scene_root.clone())?;
    let mut spawners = Vec::new();
    for node in &scene_nodes {
        if node.object.class == "StateSequenceRefererEntityData"
            && graph.target_field(&node.site, "Sequence")? == (name.into(), 0)
        {
            spawners.push(node.site.clone());
        }
    }
    if spawners.len() != 1 {
        return Err(bad("sequence spawner is missing or ambiguous"));
    }
    let root = unique(nodes, "ParticipantStateSequenceEntityData")?;
    let references = graph.links(root, rules.root_channels)?;
    let targets = graph.links(&spawners[0], rules.spawner_channels)?;
    let declared: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "LinkedSchematicChannelReferenceEntityData")
        .collect();
    if references.len() != declared.len() || targets.len() != declared.len() {
        return Err(bad("sequence spawner channel count"));
    }
    let mut out = Vec::new();
    let mut used = BTreeSet::new();
    for node in declared {
        if references
            .iter()
            .filter(|r| r.port == 0 && r.site == node.site)
            .count()
            != 1
        {
            return Err(bad("sequence root channel"));
        }
        let asset = graph.target_field(&node.site, "ChannelAsset")?;
        let mut matched = Vec::new();
        for target in &targets {
            if target.port != 0
                || graph.object(&target.site)?.class != "LinkedSchematicChannelEntityData"
            {
                return Err(bad("sequence spawner channel type"));
            }
            if graph.target_field(&target.site, "ChannelAsset")? == asset {
                matched.push(target.site.clone());
            }
        }
        if matched.len() != 1 || !used.insert(matched[0].clone()) {
            return Err(bad("sequence spawner channel mismatch"));
        }
        out.push((node.site.clone(), matched.remove(0)));
    }
    Ok(out)
}

pub(crate) fn build(index: &mut AssetIndex) -> Result<Vec<u8>, Error> {
    let rules = Rules::from_json(&index.identity.sequences.to_json())?;
    let scene_rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&scene_rules.level)?;
    let bundle = |name: &str| -> Result<usize, Error> {
        startup
            .iter()
            .position(|e| e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(name))
            .map(|i| i + 1)
            .ok_or_else(|| bad("sequence bundle is not loaded"))
    };
    let mut definitions = Vec::new();
    for (name, scene) in [
        (&rules.waiting, &rules.waiting_scene),
        (&rules.exit, &rules.exit_scene),
        (&rules.entry, &rules.exit_scene),
    ] {
        let asset = index.reference(scene, name)?;
        definitions.push(json!({"asset":{"bundle":bundle(scene)?,"type_id":asset.class_id,"local_index":asset.local_index},"catalog":index.catalog(scene)?}));
    }
    let splines = index.registered_objects(&rules.exit_scene, "ScriptedAISplineEntityData")?;
    if splines.is_empty() {
        return Err(bad("no registered spline definitions"));
    }
    let mut graph = Graph::new(&mut index.assets);
    let mut scenes = SceneLookup {
        roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
        rules: &scene_rules,
        endpoints: BTreeMap::new(),
        loaded: BTreeSet::new(),
    };
    let manager_nodes = graph.read(
        scenes
            .roots
            .get(&rules.manager_scene)
            .ok_or_else(|| bad("sequence manager scene missing"))?
            .clone(),
    )?;
    let manager = unique(&manager_nodes, "StateSequenceManagerEntityData")?;
    let bus = scenes.embedded_bus(&mut graph, manager)?;
    let manager_component = component(&mut graph, &mut scenes, manager)?;
    for (i, (name, scene)) in [
        (&rules.waiting, &rules.waiting_scene),
        (&rules.exit, &rules.exit_scene),
        (&rules.entry, &rules.exit_scene),
    ]
    .into_iter()
    .enumerate()
    {
        let nodes = graph.read(Site::root(name))?;
        profile(&nodes, &rules, i)?;
        let channels = binding(&mut graph, &scenes, &rules, &nodes, scene, name)?;
        let (reference, target) = channels
            .first()
            .cloned()
            .ok_or_else(|| bad("sequence channel missing"))?;
        let doc = &mut definitions[i];
        doc["blueprint"] = json!(
            nodes[0]
                .object
                .fields
                .get("Name")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("sequence name"))?
        );
        doc["manager"] = manager_component.clone();
        doc["embedded_bus"] = json!(bus);
        doc["channel"] = component(&mut graph, &mut scenes, &target)?;
        if i == 2 {
            doc["presentation"] = component(&mut graph, &mut scenes, &channels[1].1)?;
        }
        if i != 0 {
            let command = unique(&nodes, "ScriptedSplineCommandEntityData")?;
            let endpoint = one(graph.links(command, rules.spline_link)?)?;
            if endpoint.site != reference || endpoint.port == 0 {
                return Err(bad("spline command channel binding"));
            }
            let spline = one(graph.links(&target, endpoint.port)?)?;
            if spline.port != 0 || graph.object(&spline.site)?.class != "ScriptedAISplineEntityData"
            {
                return Err(bad("spline definition endpoint"));
            }
            let local_index = splines
                .iter()
                .position(|key| *key == spline.site.key)
                .ok_or_else(|| bad("linked spline is not registered"))?;
            let flag = graph
                .object(command)?
                .fields
                .get("LoopWhenFinished")
                .and_then(Value::as_bool)
                .ok_or_else(|| bad("spline loop flag"))?;
            doc["spline"] = json!({"asset":{"bundle":bundle(&rules.exit_scene)?,"type_id":rules.spline_class_id,"local_index":local_index},"catalog":[[rules.spline_class_id,splines.len()]],"flag":flag});
        }
        if i == 2 {
            doc["duration_ms"] = json!(entry_duration(&nodes, &rules)?);
        }
    }
    let mut result = definitions.remove(0);
    result["format"] = json!("nfs-waiting-sequence");
    result["version"] = json!(2);
    result["build_sha256"] = json!(nfs_services::SUPPORTED_BUILD_SHA256);
    result["arrive"] = definitions.remove(0);
    result["returning"] = definitions.remove(0);
    SequenceContent::from_json(&result)
        .map_err(|_| bad("generated sequence content failed validation"))?;
    let bytes = serde_json::to_vec(&result).map_err(|_| bad("sequence encoding"))?;
    if bytes.len() > 16384 {
        return Err(bad("sequence output size"));
    }
    Ok(bytes)
}

fn entry_duration(nodes: &[Node], rules: &Rules) -> Result<u64, Error> {
    use nfs_frostbite::ebx::Pointer;
    let root = unique(nodes, "ParticipantStateSequenceEntityData")?;
    let mut timers = nodes.iter().filter(|n| {
        n.object.class == "DelayEntityData"
            && n.site.key.0 == root.key.0
            && n.site.bus == root.bus
            && n.site.parents == root.parents
    });
    let delay = &timers
        .next()
        .ok_or_else(|| bad("entry timer missing"))?
        .site;
    if timers.next().is_some() {
        return Err(bad("entry timer ambiguous"));
    }
    let fields = &nodes
        .iter()
        .find(|n| n.site == *delay)
        .ok_or_else(|| bad("entry delay"))?
        .object
        .fields;
    if fields.get("Realm").and_then(Value::as_enum_name) != Some("Realm_Server")
        || fields.get("TimeDeltaType").and_then(Value::as_enum_name) != Some("TimeDeltaType_World")
        || fields.get("AutoStart").and_then(Value::as_bool) != Some(false)
        || root.key.0 != delay.key.0
        || root.bus != nodes[0].site.key
        || delay.bus != root.bus
    {
        return Err(bad("entry timer context"));
    }
    let duration = match fields.get("Delay") {
        Some(Value::Float32(v)) => f64::from(*v) * 1000.,
        _ => return Err(bad("entry delay value")),
    };
    if !duration.is_finite() || !(1.0..=30000.0).contains(&duration) || duration.fract() != 0. {
        return Err(bad("entry delay bound"));
    }
    let rows = nodes[0]
        .object
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 4096)
        .ok_or_else(|| bad("entry timer connections"))?;
    let [started, start, elapsed, stop] = rules.entry_timer_events;
    for (source, target, out_event, in_event) in
        [(root, delay, started, start), (delay, root, elapsed, stop)]
    {
        let matches = rows
            .iter()
            .filter_map(Value::as_fields)
            .filter(|f| {
                let event = |k| {
                    f.get(k)
                        .and_then(Value::as_fields)
                        .and_then(|f| f.get("Id"))
                        .and_then(Value::bits32)
                };
                f.get("Source").and_then(Value::as_pointer) == Some(Pointer::Internal(source.key.1))
                    && f.get("Target").and_then(Value::as_pointer)
                        == Some(Pointer::Internal(target.key.1))
                    && event("SourceEvent") == Some(out_event)
                    && event("TargetEvent") == Some(in_event)
                    && f.get("TargetType").and_then(Value::as_enum_name)
                        == Some("EventConnectionTargetType_Server")
            })
            .count();
        if matches != 1 {
            return Err(bad("entry timer event binding"));
        }
    }
    Ok(duration as u64)
}
