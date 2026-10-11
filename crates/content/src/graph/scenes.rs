// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    scenes,
    world::{StartupEntry, StartupKind},
};

/// Parent references preserve the instance context when a channel link leaves
/// a subworld through its interface and enters a sibling or ancestor scene.
pub(crate) fn scene_roots(
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
pub(crate) struct SceneLookup<'a> {
    pub roots: BTreeMap<String, Site>,
    pub rules: &'a scenes::Rules,
    pub endpoints: BTreeMap<Site, (u32, usize)>,
    pub loaded: BTreeSet<String>,
}
impl SceneLookup<'_> {
    pub fn scene_name(&self, site: &Site) -> Result<String, Error> {
        std::iter::once(&site.bus)
            .chain(site.parents.iter().rev().map(|f| &f.bus))
            .find_map(|bus| self.roots.contains_key(&bus.0).then(|| bus.0.clone()))
            .ok_or_else(|| bad("endpoint outside startup scenes"))
    }
    /// One-based position of the embedded bus that owns an endpoint. The bus
    /// belongs to the manager's scene, independently of a spawned blueprint.
    pub fn embedded_bus(&self, graph: &mut Graph<Assets>, site: &Site) -> Result<u32, Error> {
        let scene = self.scene_name(site)?;
        embedded_bus(&graph.read(self.roots[&scene].clone())?, site)
    }
    pub fn endpoint(
        &mut self,
        graph: &mut Graph<Assets>,
        site: &Site,
    ) -> Result<(u32, usize), Error> {
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

pub(crate) fn embedded_bus(nodes: &[Node], site: &Site) -> Result<u32, Error> {
    let mut count = 0u32;
    let mut found = None;
    for node in nodes {
        let object = &node.object;
        if !object.class.ends_with("Blueprint")
            && !matches!(object.class.as_str(), "LevelData" | "SubWorldData")
        {
            continue;
        }
        let flags = match object.fields.get("Flags") {
            Some(Value::UInt16(v)) => u32::from(*v),
            Some(v) => bits32(v)?,
            None => return Err(bad("embedded bus flags")),
        };
        if flags & 1 == 0 {
            continue;
        }
        count = count
            .checked_add(1)
            .ok_or_else(|| bad("embedded bus count"))?;
        if node.site.key == site.bus
            && node.site.parents == site.parents
            && found.replace(count).is_some()
        {
            return Err(bad("ambiguous embedded bus"));
        }
    }
    found.ok_or_else(|| bad("unregistered endpoint bus"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local, ptr};
    #[test]
    fn embedded_bus_counts_registered_occurrences_and_preserves_parent_context() {
        let mut memory = Memory::default();
        memory.put(
            "scene",
            0,
            "SubWorldData",
            &[
                ("Flags", Value::UInt16(1)),
                ("Objects", Value::Array(vec![local(1), local(2)])),
            ],
        );
        memory.put(
            "scene",
            1,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("p", 0))],
        );
        memory.put(
            "scene",
            2,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("p", 0))],
        );
        memory.put(
            "p",
            0,
            "LogicPrefabBlueprint",
            &[
                ("Flags", Value::UInt16(1)),
                ("Objects", Value::Array(vec![local(1)])),
            ],
        );
        memory.put("p", 1, "StateSequenceManagerEntityData", &[]);
        let mut graph = Graph::new(&mut memory);
        let nodes = graph.read(Site::root("scene")).unwrap();
        let managers: Vec<_> = nodes
            .iter()
            .filter(|n| n.object.class == "StateSequenceManagerEntityData")
            .collect();
        assert_eq!(embedded_bus(&nodes, &managers[0].site).unwrap(), 2);
        assert_eq!(embedded_bus(&nodes, &managers[1].site).unwrap(), 3);
        let mut foreign = managers[0].site.clone();
        foreign.parents[0].reference.1 = 99;
        assert!(embedded_bus(&nodes, &foreign).is_err());
        let mut duplicate = nodes.clone();
        duplicate.push(nodes[2].clone());
        assert!(embedded_bus(&duplicate, &managers[0].site).is_err());
    }
}
