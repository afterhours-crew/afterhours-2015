// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Persistent input ownership for map conditions beyond the active gameplay
//! branches. Initial expressions are used only when an objective has no save row.
use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateOwners {
    pub objectives: BTreeMap<u64, crate::progression::Expr>,
    pub integers: BTreeSet<u64>,
}
impl AssetIndex {
    pub fn condition_state_owners(&mut self, facts: &BTreeSet<Fact>) -> Result<StateOwners, Error> {
        let rules = self.identity.progression.clone();
        let mut graph = Graph::new(&mut self.assets);
        let mut roots = owner_roots(&mut graph, &rules.main, Some(&rules.scene))?;
        roots.insert(rules.intro.clone());
        let mut branches = Vec::new();
        let mut count = 0;
        for root in roots {
            let nodes = graph.read(Site::root(&root))?;
            count += nodes.len();
            if count > crate::scenes::MAX_GRAPH_OBJECTS {
                return Err(bad("map progression owner node bound"));
            }
            branches.push(nodes);
        }
        crate::progression::extract::condition_owners(&mut graph, &rules, &branches, facts)
    }
}
