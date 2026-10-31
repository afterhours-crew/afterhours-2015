// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Result/recommendation invitations. Race results are session latches; only
//! the selected message is durable. A best-ever saved race position is not a
//! substitute for a result from the qualified participant reporter.
use super::*;
use std::{collections::BTreeSet, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invitation {
    objective: u64,
    result_objective: u64,
    manager: usize,
    hashes: [u32; 3],
    recommendation_key: u32,
    recommendation_after: Duration,
    default_after: Duration,
}
impl Invitation {
    pub fn new(
        objective: u64,
        result_objective: u64,
        manager: usize,
        hashes: [u32; 3],
        recommendation_key: u32,
        recommendation_after: Duration,
        default_after: Duration,
    ) -> Result<Self, Error> {
        if objective == 0
            || result_objective == 0
            || objective == result_objective
            || hashes.into_iter().collect::<BTreeSet<_>>().len() != 3
            || recommendation_after.is_zero()
            || default_after < recommendation_after
            || default_after > Duration::from_secs(60)
        {
            return Err(bad("result invitation bounds"));
        }
        Ok(Self {
            objective,
            result_objective,
            manager,
            hashes,
            recommendation_key,
            recommendation_after,
            default_after,
        })
    }
    pub fn objective(&self) -> u64 {
        self.objective
    }
    pub fn result_objective(&self) -> u64 {
        self.result_objective
    }
    pub fn manager(&self) -> usize {
        self.manager
    }
    /// Good result/default, other result, recommended.
    pub fn hashes(&self) -> [u32; 3] {
        self.hashes
    }
    pub fn recommendation_key(&self) -> u32 {
        self.recommendation_key
    }
    pub fn recommendation_after(&self) -> Duration {
        self.recommendation_after
    }
    pub fn default_after(&self) -> Duration {
        self.default_after
    }
}

impl AssetIndex {
    /// Compile the whole group, then validate its source through the same
    /// result reporter policy used for transactional objective completion.
    pub fn result_objective_messages(&mut self, selected: [&str; 3]) -> Result<Invitation, Error> {
        if selected.iter().any(|s| s.is_empty() || s.len() > 512)
            || selected.into_iter().collect::<BTreeSet<_>>().len() != 3
        {
            return Err(bad("result invitation selection"));
        }
        let rules = self.identity.progression.clone();
        let bindings = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == MANAGER)
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let map = self.narrative_event_map()?;
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(&rules.main))?;
        let world = graph.read(Site::root(GAMEPLAY))?;
        let plan = reader::read(
            &mut graph,
            &nodes,
            &world,
            &bindings,
            rules.ports["stable_id"],
            selected,
            &map,
        )?;
        drop(graph);
        self.objective_results(&BTreeSet::from([plan.result_objective]))?;
        Ok(plan)
    }
}
mod reader;
