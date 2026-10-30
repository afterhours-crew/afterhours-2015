// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Mutually exclusive invitations selected by current versus stock horsepower.
//! Compile the activation, comparison, latch and delayed-default graph together;
//! message names or the presence of a vehicle-stat provider alone prove nothing.
use super::*;
use std::{collections::BTreeSet, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invitation {
    objective: u64,
    manager: usize,
    hashes: [u32; 3],
    increments: [u32; 2],
    default_after: Duration,
}
impl Invitation {
    pub fn new(
        objective: u64,
        manager: usize,
        hashes: [u32; 3],
        increments: [u32; 2],
        default_after: Duration,
    ) -> Result<Self, Error> {
        if objective == 0
            || hashes.into_iter().collect::<BTreeSet<_>>().len() != 3
            || increments[0] == 0
            || increments[0] >= increments[1]
            || increments[1] > 1500
            || default_after.is_zero()
            || default_after > Duration::from_secs(60)
        {
            return Err(bad("power invitation bounds"));
        }
        Ok(Self {
            objective,
            manager,
            hashes,
            increments,
            default_after,
        })
    }
    pub fn objective(&self) -> u64 {
        self.objective
    }
    pub fn manager(&self) -> usize {
        self.manager
    }
    pub fn hashes(&self) -> [u32; 3] {
        self.hashes
    }
    pub fn default_after(&self) -> Duration {
        self.default_after
    }
    /// Strict less-than tiers; equality belongs to the next tier. Missing
    /// vehicle state is distinct from a real zero and from the authored delay.
    pub fn choose(&self, current: u32, stock: u32) -> Result<u32, Error> {
        if current > 1500 || stock > 1500 {
            return Err(bad("power invitation vehicle bound"));
        }
        Ok(self.hashes[if current < stock + self.increments[0] {
            0
        } else if current < stock + self.increments[1] {
            1
        } else {
            2
        }])
    }
}

impl AssetIndex {
    /// The three selected assets are ordered by increasing horsepower tier.
    /// Resolve all three as one restrictor group, including the default delay.
    pub fn power_objective_messages(&mut self, selected: [&str; 3]) -> Result<Invitation, Error> {
        if selected.iter().any(|s| s.is_empty() || s.len() > 512)
            || selected.into_iter().collect::<BTreeSet<_>>().len() != 3
        {
            return Err(bad("power invitation selection"));
        }
        let rules = self.identity.progression.clone();
        let bindings = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == MANAGER)
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(&rules.main))?;
        let world = graph.read(Site::root(GAMEPLAY))?;
        read(
            &mut graph,
            &nodes,
            &world,
            &bindings,
            rules.ports["stable_id"],
            selected,
        )
    }
}

mod reader;
use reader::read;
