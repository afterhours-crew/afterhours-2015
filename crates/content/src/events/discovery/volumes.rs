// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored input sets for selected world-volume families. Resolve every
//! instance through its actual scene bus, including SubWorld interfaces.
use super::*;

const MAX_FILTERS: usize = 1024;
const PARTICIPANTS: u32 = 0x5fd7_137a;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    pub endpoint: Endpoint,
    pub program: membership::Program,
}

fn read<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    endpoint: &mut impl FnMut(&mut Graph<S>, &Site) -> Result<Endpoint, Error>,
) -> Result<membership::Program, Error> {
    if graph.object(site)?.class != START_VOLUME {
        return Err(bad("world volume filter class"));
    }
    let inputs = graph.links(site, PARTICIPANTS)?;
    let [input] = inputs.as_slice() else {
        return Err(bad("world volume input missing or ambiguous"));
    };
    if input.port != 0 {
        return Err(bad("world volume input port"));
    }
    membership::read_set(graph, input.site.clone(), &BTreeMap::new(), endpoint)
}

impl AssetIndex {
    /// Compile memberships only for the requested authored prefab families.
    /// Unsupported predicates fail preparation; a partial catalog cannot stand
    /// in for the requested inputs. This does not create live subscriptions.
    pub fn world_volume_memberships(
        &mut self,
        families: &BTreeSet<String>,
    ) -> Result<Vec<Binding>, Error> {
        if families.is_empty()
            || families.len() > 32
            || families.iter().any(|f| f.is_empty() || f.len() > 512)
        {
            return Err(bad("world volume family bound"));
        }
        let rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&rules.level)?;
        let mut graph = Graph::new(&mut self.assets);
        let mut scenes = SceneLookup {
            roots: scene_roots(&mut graph, &rules.level, &startup)?,
            rules: &rules,
            endpoints: BTreeMap::new(),
            loaded: BTreeSet::new(),
        };
        let roots: Vec<_> = scenes.roots.values().cloned().collect();
        let mut found = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for root in roots {
            for node in graph.read(root)? {
                if node.object.class != START_VOLUME || !families.contains(&node.site.key.0) {
                    continue;
                }
                seen.insert(node.site.key.0.clone());
                let (scene_key, serializer) = scenes.endpoint(&mut graph, &node.site)?;
                let endpoint = Endpoint {
                    scene_key,
                    serializer,
                };
                let program = read(&mut graph, &node.site, &mut |g, s| {
                    let (scene_key, serializer) = scenes.endpoint(g, s)?;
                    Ok(Endpoint {
                        scene_key,
                        serializer,
                    })
                })?;
                if found.insert(endpoint, program).is_some() || found.len() > MAX_FILTERS {
                    return Err(bad("world volume duplicate endpoint or bound"));
                }
            }
        }
        if &seen != families {
            return Err(bad("world volume family missing"));
        }
        Ok(found
            .into_iter()
            .map(|(endpoint, program)| Binding { endpoint, program })
            .collect())
    }
}

#[cfg(test)]
mod tests;
