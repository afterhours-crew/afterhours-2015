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
        return Err(bad("sequence requires one endpoint"));
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
fn profile(nodes: &[Node], rules: &Rules, exit: bool) -> Result<(), Error> {
    if nodes
        .first()
        .is_none_or(|n| n.object.class != "ParticipantStateSequenceBlueprint")
    {
        return Err(bad("sequence blueprint class"));
    }
    let mut roles = Vec::new();
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
    let expected: &[&str] = if exit {
        &["root", "provider", "event", "event", "channel"]
    } else {
        &["root", "provider", "channel"]
    };
    if roles != expected {
        return Err(bad("unsupported sequence serializer order"));
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
) -> Result<(Site, Site), Error> {
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
    let reference = one(graph.links(root, rules.root_channels)?)?;
    let declared = unique(nodes, "LinkedSchematicChannelReferenceEntityData")?;
    if reference.port != 0 || reference.site != *declared {
        return Err(bad("sequence root channel"));
    }
    let target = one(graph.links(&spawners[0], rules.spawner_channels)?)?;
    if target.port != 0
        || graph.object(&target.site)?.class != "LinkedSchematicChannelEntityData"
        || graph.target_field(&target.site, "ChannelAsset")?
            != graph.target_field(declared, "ChannelAsset")?
    {
        return Err(bad("sequence spawner channel mismatch"));
    }
    Ok((reference.site, target.site))
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
    ]
    .into_iter()
    .enumerate()
    {
        let nodes = graph.read(Site::root(name))?;
        profile(&nodes, &rules, i == 1)?;
        let (reference, target) = binding(&mut graph, &scenes, &rules, &nodes, scene, name)?;
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
        if i == 1 {
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
    }
    let mut result = definitions.remove(0);
    result["format"] = json!("nfs-waiting-sequence");
    result["version"] = json!(2);
    result["build_sha256"] = json!(nfs_services::SUPPORTED_BUILD_SHA256);
    result["arrive"] = definitions.remove(0);
    SequenceContent::from_json(&result)
        .map_err(|_| bad("generated sequence content failed validation"))?;
    let bytes = serde_json::to_vec(&result).map_err(|_| bad("sequence encoding"))?;
    if bytes.len() > 16384 {
        return Err(bad("sequence output size"));
    }
    Ok(bytes)
}
