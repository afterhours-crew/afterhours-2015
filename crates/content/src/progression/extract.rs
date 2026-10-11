// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Content, Rules, graph::*};
use crate::{
    Error,
    assets::AssetIndex,
    scan::Assets,
    scenes,
    world::{StartupEntry, StartupKind},
};
use nfs_frostbite::ebx::Value;
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

fn unique<'a>(nodes: &'a [Node], class: &str) -> Result<&'a Site, Error> {
    let mut found = nodes.iter().filter(|n| n.object.class == class);
    let site = &found
        .next()
        .ok_or_else(|| bad(&format!("missing {class}")))?
        .site;
    if found.next().is_some() {
        return Err(bad(&format!("multiple {class}")));
    }
    Ok(site)
}
fn one(mut endpoints: Vec<Endpoint>) -> Result<Endpoint, Error> {
    if endpoints.len() != 1 {
        return Err(bad("expected exactly one link endpoint"));
    }
    Ok(endpoints.remove(0))
}
fn stable<S: Source>(graph: &mut Graph<S>, rules: &Rules, site: &Site) -> Result<u64, Error> {
    let endpoint = one(graph.links(site, rules.ports["stable_id"])?)?;
    let object = graph.object(&endpoint.site)?;
    if endpoint.port != 0 || object.class != "StableUniqueIdEntityData" {
        return Err(bad("stable ID endpoint"));
    }
    let id = match object.fields.get("StableUniqueId64") {
        Some(Value::Int64(n)) => *n as u64,
        Some(Value::UInt64(n)) => *n,
        _ => return Err(bad("stable ID field")),
    };
    if id == 0 {
        return Err(bad("zero stable ID"));
    }
    Ok(id)
}
type Publishers = BTreeMap<(Key, u32), Site>;
fn registered_bus(value: Option<&Value>) -> Result<bool, Error> {
    let flags = match value {
        Some(Value::UInt16(value)) => u32::from(*value),
        Some(value) => bits32(value)?,
        None => return Err(bad("blueprint flags")),
    };
    Ok(flags & 1 != 0)
}
fn channel<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<Key, Error> {
    graph.target_field(site, "ChannelAsset")
}
fn participant_channel<S: Source>(
    graph: &mut Graph<S>,
    rules: &Rules,
    site: &Site,
) -> Result<(Key, u32), Error> {
    let endpoint = one(graph.links(site, rules.ports["participant_channel"])?)?;
    if endpoint.port == 0
        || graph.object(&endpoint.site)?.class != "LinkedSchematicChannelReferenceEntityData"
    {
        return Err(bad("participant channel endpoint"));
    }
    Ok((channel(graph, &endpoint.site)?, endpoint.port))
}
fn publishers<S: Source>(
    graph: &mut Graph<S>,
    rules: &Rules,
    branches: &[Vec<Node>],
) -> Result<Publishers, Error> {
    let mut out = BTreeMap::new();
    for node in branches
        .iter()
        .flatten()
        .filter(|n| n.object.class == "BoolParticipantPropertyAccessorEntityData")
    {
        if !graph
            .input_ports(&node.site)?
            .contains(&rules.ports["bool_value"])
        {
            continue;
        }
        let key = participant_channel(graph, rules, &node.site)?;
        if out.insert(key, node.site.clone()).is_some() {
            return Err(bad("ambiguous participant property publisher"));
        }
    }
    Ok(out)
}
struct Expressions<'a, 'b, S: Source> {
    graph: &'a mut Graph<'b, S>,
    rules: &'a Rules,
    publishers: &'a Publishers,
    active: BTreeSet<Endpoint>,
    count: usize,
}
impl<S: Source> Expressions<'_, '_, S> {
    fn input(&mut self, site: &Site, port: u32) -> Result<Json, Error> {
        let endpoint = self.graph.input(site, port)?;
        self.expression(endpoint)
    }
    fn expression(&mut self, endpoint: Endpoint) -> Result<Json, Error> {
        self.count += 1;
        if self.active.len() >= 16 || self.count > super::MAX_EXPRESSIONS {
            return Err(bad("activation expression bound"));
        }
        if !self.active.insert(endpoint.clone()) {
            return Err(bad("activation expression cycle"));
        }
        let object = self.graph.object(&endpoint.site)?;
        let site = &endpoint.site;
        let port = endpoint.port;
        let p = &self.rules.ports;
        let value = match object.class.as_str() {
            "RaceObjectiveEntityData" if port == p["completed"] => {
                json!({"completed":stable(self.graph, self.rules, site)?})
            }
            "BoolEntityData" if port == p["bool_value"] => {
                json!({"constant":object.fields.get("DefaultValue").and_then(Value::as_bool).ok_or_else(|| bad("boolean default"))?})
            }
            "NotEntityData" if port == p["logic_output"] => {
                json!({"negate":self.input(site, p["not_input"])?})
            }
            "AndEntityData" if port == p["logic_output"] => {
                let count = bits32(
                    object
                        .fields
                        .get("InputCount")
                        .ok_or_else(|| bad("and input count"))?,
                )? as usize;
                let ports = self.graph.input_ports(site)?;
                if count == 0
                    || count > 16
                    || ports.len() != count
                    || ports.iter().collect::<BTreeSet<_>>().len() != count
                {
                    return Err(bad("and input count or duplicate"));
                }
                json!({"all":ports.into_iter().map(|port| self.input(site, port)).collect::<Result<Vec<_>, _>>()?})
            }
            "SettingEntityData"
                if port == p["setting_output"]
                    && object.fields.get("BoolSettingName").and_then(Value::as_str)
                        == Some("Gameplay.NFS16ConsumerBeta") =>
            {
                json!({"setting":"Gameplay.NFS16ConsumerBeta"})
            }
            "BoolParticipantPropertyAccessorEntityData" if port == p["participant_output"] => {
                let key = participant_channel(self.graph, self.rules, site)?;
                let publisher = self
                    .publishers
                    .get(&key)
                    .ok_or_else(|| bad("missing participant property publisher"))?
                    .clone();
                self.input(&publisher, p["bool_value"])?
            }
            _ => {
                return Err(bad(&format!(
                    "unsupported activation output {}:{port:08x}",
                    object.class
                )));
            }
        };
        self.active.remove(&endpoint);
        Ok(value)
    }
}
fn restoration<S: Source>(
    graph: &mut Graph<S>,
    rules: &Rules,
    branches: &[Vec<Node>],
) -> Result<Vec<Json>, Error> {
    let publishers = publishers(graph, rules, branches)?;
    let mut expressions = Expressions {
        graph,
        rules,
        publishers: &publishers,
        active: BTreeSet::new(),
        count: 0,
    };
    let mut out = Vec::new();
    for (name, nodes) in ["main", "intro"].into_iter().zip(branches) {
        let objectives = unique(nodes, "ObjectivePersistenceEntityData")?;
        let values = unique(nodes, "ValuesPersistenceEntityData")?;
        let objectives = expressions
            .graph
            .links(objectives, rules.ports["objectives"])?;
        let ints = expressions.graph.links(values, rules.ports["ints"])?;
        for port in ["floats", "dynamic_floats"] {
            if !expressions
                .graph
                .links(values, rules.ports[port])?
                .is_empty()
            {
                return Err(bad("float progression persistence is unsupported"));
            }
        }
        let mut rows = Vec::new();
        for endpoint in objectives {
            if endpoint.port != 0
                || expressions.graph.object(&endpoint.site)?.class != "RaceObjectiveEntityData"
            {
                return Err(bad("objective persistence endpoint"));
            }
            let id = stable(expressions.graph, rules, &endpoint.site)?;
            let active = expressions.input(&endpoint.site, rules.ports["active"])?;
            rows.push(json!({"id":id,"active":active}));
        }
        let ints = ints
            .into_iter()
            .map(|endpoint| {
                if endpoint.port != 0 {
                    return Err(bad("int persistence endpoint port"));
                }
                stable(expressions.graph, rules, &endpoint.site)
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(json!({"name":name,"objectives":rows,"ints":ints}));
    }
    Ok(out)
}

/// Parent references preserve the instance context when a channel link leaves
/// a subworld through its interface and enters a sibling or ancestor scene.
fn scene_roots(
    graph: &mut Graph<Assets>,
    level: &str,
    startup: &[StartupEntry],
) -> Result<BTreeMap<String, Site>, Error> {
    let level = level.to_lowercase();
    let mut roots = BTreeMap::from([(level.clone(), Site::root(&level))]);
    for entry in startup.iter().filter(|e| e.kind == StartupKind::SubLevel) {
        let parent = entry
            .parent
            .map(|i| startup[i].name.to_lowercase())
            .unwrap_or_else(|| level.clone());
        let parent_site = roots
            .get(&parent)
            .ok_or_else(|| bad("startup parent order"))?
            .clone();
        let objects = graph.source.objects(&parent)?.to_vec();
        let mut found = objects.iter().enumerate().filter(|(_, o)| {
            o.class == "SubWorldReferenceObjectData"
                && o.fields
                    .get("BundleName")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.eq_ignore_ascii_case(&entry.name))
                && o.fields.get("AutoLoad").and_then(Value::as_bool) == Some(true)
                && o.fields.get("IsWin32SubLevel").and_then(Value::as_bool) == Some(true)
        });
        let (index, _) = found
            .next()
            .ok_or_else(|| bad("startup parent reference missing"))?;
        if found.next().is_some() {
            return Err(bad("ambiguous startup parent reference"));
        }
        let mut site = Site::root(&entry.name);
        site.parents = parent_site.parents;
        site.parents.push(Frame {
            bus: parent_site.bus,
            reference: (parent, index),
        });
        if roots.insert(entry.name.to_lowercase(), site).is_some() {
            return Err(bad("duplicate startup scene"));
        }
    }
    Ok(roots)
}
struct SceneLookup<'a> {
    roots: BTreeMap<String, Site>,
    rules: &'a scenes::Rules,
    endpoints: BTreeMap<Site, (u32, usize)>,
    loaded: BTreeSet<String>,
}
impl SceneLookup<'_> {
    fn endpoint(&mut self, graph: &mut Graph<Assets>, site: &Site) -> Result<(u32, usize), Error> {
        let scene = std::iter::once(&site.bus)
            .chain(site.parents.iter().rev().map(|f| &f.bus))
            .find_map(|bus| self.roots.contains_key(&bus.0).then(|| bus.0.clone()))
            .ok_or_else(|| bad("channel outside startup scenes"))?;
        if self.loaded.insert(scene.clone()) {
            let nodes = graph.read(self.roots[&scene].clone())?;
            // Compare both builders' occurrence order before using an index.
            let (_, instances) = scenes::construct(graph.source, self.rules, &scene, true)?;
            if nodes.len() != instances.len() {
                return Err(bad("scene occurrence count differs"));
            }
            let key = if scene == self.rules.level.to_lowercase() {
                1
            } else {
                crate::assets::scene_key(&scene)
            };
            for (node, instance) in nodes.into_iter().zip(instances) {
                if node.site.key != (instance.asset, instance.index) {
                    return Err(bad("scene occurrence order differs"));
                }
                if let Some(serializer) = instance.serializer
                    && self
                        .endpoints
                        .insert(node.site, (key, serializer))
                        .is_some()
                {
                    return Err(bad("ambiguous scene occurrence"));
                }
            }
        }
        self.endpoints
            .get(site)
            .copied()
            .ok_or_else(|| bad("channel is not a scene serializer"))
    }
}
type SceneBindings = BTreeMap<Site, (u32, usize)>;
fn bindings(
    graph: &mut Graph<Assets>,
    rules: &Rules,
    branches: &[Vec<Node>],
    scenes: &mut SceneLookup<'_>,
) -> Result<Vec<SceneBindings>, Error> {
    let root = scenes
        .roots
        .get(&rules.scene)
        .ok_or_else(|| bad("progression scene is not loaded"))?
        .clone();
    let scene_nodes = graph.read(root)?;
    let mut result = Vec::new();
    for (name, nodes) in [&rules.main, &rules.intro].into_iter().zip(branches) {
        let spawners = scene_nodes
            .iter()
            .filter(|n| n.object.class == "ParticipantLogicSpawnerEntityData")
            .map(|n| {
                Ok((
                    graph.target_field(&n.site, "ParticipantLogicBlueprint")?,
                    n.site.clone(),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut spawners = spawners.iter().filter(|(key, _)| key == &(name.clone(), 0));
        let spawner = &spawners
            .next()
            .ok_or_else(|| bad("missing progression spawner"))?
            .1;
        if spawners.next().is_some() {
            return Err(bad("ambiguous progression spawner"));
        }
        let root = unique(nodes, "ParticipantLogicEntityData")?;
        let references = graph.links(root, rules.ports["root_channels"])?;
        let targets = graph.links(spawner, rules.ports["spawner_channels"])?;
        let mut refs = Vec::new();
        for endpoint in references {
            if endpoint.port != 0
                || graph.object(&endpoint.site)?.class
                    != "LinkedSchematicChannelReferenceEntityData"
            {
                return Err(bad("root channel reference endpoint"));
            }
            refs.push((channel(graph, &endpoint.site)?, endpoint.site));
        }
        let mut bound = BTreeMap::new();
        // Match in connection order: each target binds the first compatible,
        // still-unbound reference. Unrelated occurrences of an asset are not candidates.
        for endpoint in targets {
            if endpoint.port != 0
                || graph.object(&endpoint.site)?.class != "LinkedSchematicChannelEntityData"
            {
                return Err(bad("spawner channel endpoint"));
            }
            let asset = channel(graph, &endpoint.site)?;
            let reference = refs
                .iter()
                .find(|(a, site)| *a == asset && !bound.contains_key(site))
                .ok_or_else(|| bad("unmatched spawner channel"))?;
            bound.insert(reference.1.clone(), scenes.endpoint(graph, &endpoint.site)?);
        }
        let all: BTreeSet<_> = nodes
            .iter()
            .filter(|n| n.object.class == "LinkedSchematicChannelReferenceEntityData")
            .map(|n| n.site.clone())
            .collect();
        if bound.keys().cloned().collect::<BTreeSet<_>>() != all || bound.len() != refs.len() {
            return Err(bad("unbound or duplicate progression channel reference"));
        }
        result.push(bound);
    }
    Ok(result)
}
pub(crate) fn build(index: &mut AssetIndex, fingerprint: &str) -> Result<Vec<u8>, Error> {
    let rules = Rules::from_json(&index.identity.progression.to_json())?;
    let scene_rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&scene_rules.level)?;
    let bundle = startup
        .iter()
        .position(|e| e.name.eq_ignore_ascii_case(&rules.scene) && e.kind == StartupKind::SubLevel)
        .map(|i| i + 1)
        .ok_or_else(|| bad("progression bundle missing"))?;
    let catalog = index.catalog(&rules.scene)?;
    let asset_refs = [&rules.main, &rules.intro]
        .into_iter()
        .map(|name| index.reference(&rules.scene, name))
        .collect::<Result<Vec<_>, _>>()?;
    let mut graph = Graph::new(&mut index.assets);
    let branches = [&rules.main, &rules.intro]
        .into_iter()
        .map(|name| graph.read(Site::root(name)))
        .collect::<Result<Vec<_>, _>>()?;
    for nodes in &branches {
        if nodes
            .first()
            .is_none_or(|n| n.object.class != "ParticipantLogicBlueprint")
        {
            return Err(bad("progression root class"));
        }
        for node in nodes {
            if !rules.classes.contains_key(&node.object.class) {
                return Err(bad(&format!(
                    "unsupported composition class {}",
                    node.object.class
                )));
            }
        }
    }
    let restored = restoration(&mut graph, &rules, &branches)?;
    let mut scenes = SceneLookup {
        roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
        rules: &scene_rules,
        endpoints: BTreeMap::new(),
        loaded: BTreeSet::new(),
    };
    let bindings = bindings(&mut graph, &rules, &branches, &mut scenes)?;
    let mut profiles = Vec::new();
    let mut entities = Vec::new();
    for (((name, nodes), reference), bindings) in ["main", "intro"]
        .into_iter()
        .zip(branches)
        .zip(asset_refs)
        .zip(bindings)
    {
        let asset = json!({"bundle":bundle,"type_id":reference.class_id,"local_index":reference.local_index});
        let mut roles = Vec::new();
        let mut kinds = Vec::new();
        let mut buses = 0u16;
        for node in nodes {
            if node.object.class.ends_with("Blueprint")
                && registered_bus(node.object.fields.get("Flags"))?
            {
                buses = buses
                    .checked_add(1)
                    .ok_or_else(|| bad("bus count overflow"))?;
            }
            let Some(role) = &rules.classes[&node.object.class] else {
                continue;
            };
            let (role, kind) = match role.as_str() {
                "root" => (json!({"kind":"root"}), "root"),
                "objective" => (
                    json!({"kind":"objective","id":stable(&mut graph, &rules, &node.site)?}),
                    "rpc_flags",
                ),
                "channel" => {
                    let (key, serializer) = bindings
                        .get(&node.site)
                        .ok_or_else(|| bad("missing channel binding"))?;
                    (
                        json!({"kind":"channel","scene_content_key":key,"scene_serializer":serializer}),
                        "rpc_reference",
                    )
                }
                "rpc" | "launcher" => (json!({"kind":role}), "rpc"),
                "bool" => (json!({"kind":"bool"}), "bool_property"),
                "int" => (json!({"kind":"int"}), "i32_property"),
                _ => return Err(bad("unsupported serializer role")),
            };
            roles.push(role);
            kinds.push(kind);
        }
        profiles.push(json!({"asset":asset,"serializers":kinds}));
        entities.push(json!({"name":name,"asset":asset,"bus_count":buses,"roles":roles}));
    }
    let source = json!({"path":"installation:ParticipantLogicBlueprint","sha256":fingerprint});
    let value = json!({"format":super::FORMAT,"version":super::VERSION,"build_sha256":super::BUILD,
        "sources":[source],"restoration":{"version":1,"build_sha256":super::BUILD,"source":source,"branches":restored},
        "entities":{"content":{"catalogs":[{"bundle":bundle,"entries":catalog}],"profiles":profiles},"branches":entities},
        "settings":{"consumer_beta":false,"enable_speed_lists":false}});
    Content::from_json(&value).map_err(|_| bad("generated content failed validation"))?;
    let bytes = serde_json::to_vec(&value).map_err(|_| bad("encoding"))?;
    if bytes.len() as u64 > super::MAX_BYTES {
        return Err(bad("generated content size bound"));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "expression_tests.rs"]
mod tests;
