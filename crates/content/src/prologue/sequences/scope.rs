// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::events::Identity;

/// A movie may select a scene-owned referer through its channel. Its registry
/// and channel targets belong to that exact loaded occurrence, not the story
/// bundle or another scene that happens to register the same sequence asset.
pub(super) fn resolve<S: Source>(
    graph: &mut Graph<S>,
    branch: &[Node],
    roots: &BTreeMap<String, Site>,
    selected: Option<&Identity>,
    parent_registry: &str,
) -> Result<(Vec<Node>, String), Error> {
    let Some(selected) = selected else {
        return Ok((branch.to_vec(), parent_registry.into()));
    };
    let count = branch
        .iter()
        .filter(|n| Identity::from(&n.site) == *selected)
        .count();
    if count == 1 {
        return Ok((branch.to_vec(), parent_registry.into()));
    }
    if count > 1 {
        return Err(bad("ambiguous story sequence referer"));
    }
    let mut found = None;
    for (registry, root) in roots {
        let prefix = root
            .parents
            .iter()
            .map(|f| f.reference.clone())
            .collect::<Vec<_>>();
        if !selected.instances.starts_with(&prefix) {
            continue;
        }
        let nodes = graph.read(root.clone())?;
        let count = nodes
            .iter()
            .filter(|n| {
                Identity::from(&n.site) == *selected
                    && n.object.class == "StateSequenceRefererEntityData"
            })
            .count();
        if count == 0 {
            continue;
        }
        if count != 1 || found.is_some() {
            return Err(bad("ambiguous scene sequence referer"));
        }
        found = Some((nodes, registry.clone()));
    }
    found.ok_or_else(|| bad("sequence referer outside owned story and loaded scenes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local};
    #[test]
    fn scene_registry_follows_exact_occurrence_and_refuses_missing_or_ambiguous_matches() {
        let mut m = Memory::default();
        m.put(
            "scene",
            0,
            "SubWorldData",
            &[("Objects", Value::Array(vec![local(1)]))],
        );
        m.put("scene", 1, "StateSequenceRefererEntityData", &[]);
        let mut scene = Site::root("scene");
        scene.parents.push(crate::graph::Frame {
            bus: ("level".into(), 0),
            reference: ("level".into(), 3),
        });
        let mut graph = Graph::new(&mut m);
        let nodes = graph.read(scene.clone()).unwrap();
        let id = Identity::from(&nodes[1].site);
        let mut roots = BTreeMap::from([("scene".into(), scene.clone())]);
        let (selected, registry) = resolve(&mut graph, &[], &roots, Some(&id), "story").unwrap();
        assert_eq!(registry, "scene");
        assert_eq!(Identity::from(&selected[1].site), id);
        assert_eq!(
            resolve(&mut graph, &nodes, &roots, Some(&id), "story")
                .unwrap()
                .1,
            "story"
        );
        let mut foreign = id.clone();
        foreign.instances[0].1 = 4;
        assert!(resolve(&mut graph, &[], &roots, Some(&foreign), "story").is_err());
        roots.insert("another".into(), scene);
        assert!(resolve(&mut graph, &[], &roots, Some(&id), "story").is_err());
    }
}
