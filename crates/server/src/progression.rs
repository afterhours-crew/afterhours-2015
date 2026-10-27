// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(test)]
use crate::content::BUILD;
use crate::{Failure, persistent::Loaded};
pub use data::{Branch, FORMAT, ObjectiveRow, Settings, VERSION};
use nfs_content::progression as data;
use nfs_storage::tables::Value;
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
#[cfg(test)]
const MAX_ENTRIES: usize = 2048;
pub mod entities;
fn parse_error(e: data::ParseError) -> Failure {
    match e {
        data::ParseError::Invalid => Failure::ProfileConfig,
        data::ParseError::Limit => Failure::BodyLimit,
        data::ParseError::Io => Failure::Output,
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    definition: data::Catalog,
}
const OBJECTIVES: u32 = 2251697951;
const VALUES: u32 = 52582350;
const ACTIVE: u32 = 2484178249;
const COMPLETED: u32 = 1928661128;
const INTS: u32 = 2087826853;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Objective {
    pub active: bool,
    pub completed: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Output {
    Int { id: u64, value: i32 },
    StartObjective { id: u64 },
    ValuesRestored,
    ObjectivesRestored,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Restored {
    generation: u64,
    branches: BTreeMap<Branch, BranchState>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchState {
    objectives: BTreeMap<u64, Objective>,
    ints: BTreeMap<u64, i32>,
    pending: Vec<Output>,
}
impl BranchState {
    pub fn objectives(&self) -> &BTreeMap<u64, Objective> {
        &self.objectives
    }
    pub fn ints(&self) -> &BTreeMap<u64, i32> {
        &self.ints
    }
    pub fn pending(&self) -> &[Output] {
        &self.pending
    }
    pub fn take_outputs(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.pending)
    }
}
impl Restored {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn branch(&self, branch: Branch) -> &BranchState {
        &self.branches[&branch]
    }
    pub fn branch_mut(&mut self, branch: Branch) -> &mut BranchState {
        self.branches
            .get_mut(&branch)
            .expect("both validated branches")
    }
}
impl Catalog {
    pub fn story_policy(&self, settings: Settings) -> nfs_services::story::Policy<'_> {
        nfs_services::story::Policy {
            catalog: &self.definition,
            settings,
        }
    }
    pub fn objective_ids(&self, branch: Branch) -> BTreeSet<u64> {
        self.definition.objective_ids(branch)
    }
    /// Saved objective rows of a post-prologue account; see the content crate.
    pub fn post_intro(&self, settings: Settings) -> Vec<ObjectiveRow> {
        self.definition.post_intro(settings)
    }
    pub fn load(path: &Path) -> Result<Self, Failure> {
        Ok(Self {
            definition: data::Catalog::load(path).map_err(parse_error)?,
        })
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        Ok(Self {
            definition: data::Catalog::from_json(v).map_err(parse_error)?,
        })
    }
    pub fn restore(&self, loaded: &Loaded, settings: Settings) -> Result<Restored, Failure> {
        if !loaded.tables().contains_key(&OBJECTIVES) || !loaded.tables().contains_key(&VALUES) {
            return Err(Failure::ProfileConfig);
        }
        let mut branches = BTreeMap::new();
        for (&branch, definition) in self.definition.branches() {
            let mut state = BranchState {
                objectives: BTreeMap::new(),
                ints: BTreeMap::new(),
                pending: Vec::new(),
            };
            for &id in definition.ints() {
                let cell = int_cell(loaded, VALUES, id, INTS)?;
                state.ints.insert(id, cell.unwrap_or(0));
                if let Some(value) = cell {
                    state.pending.push(Output::Int { id, value });
                }
            }
            state.pending.push(Output::ValuesRestored);
            for (id, expr) in definition.objectives() {
                let initial = Objective {
                    active: expr.initial(settings),
                    completed: false,
                };
                let restored = match (
                    int_cell(loaded, OBJECTIVES, *id, ACTIVE)?,
                    int_cell(loaded, OBJECTIVES, *id, COMPLETED)?,
                ) {
                    (Some(active), Some(completed)) => Objective {
                        active: active > 0,
                        completed: completed > 0,
                    },
                    _ => initial,
                };
                state.objectives.insert(*id, restored);
                state.pending.push(Output::StartObjective { id: *id });
            }
            state.pending.push(Output::ObjectivesRestored);
            branches.insert(branch, state);
        }
        Ok(Restored {
            generation: loaded.generation(),
            branches,
        })
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Dispatched {
    pub values_restored: bool,
    pub objectives_restored: bool,
    pub ints_published: usize,
    pub activated: Vec<u64>,
}
impl Dispatched {
    pub fn restored(&self) -> bool {
        self.values_restored && self.objectives_restored
    }
}
pub fn dispatch(state: &mut Restored) -> Result<BTreeMap<Branch, Dispatched>, Failure> {
    let mut result = BTreeMap::new();
    for branch in [Branch::Main, Branch::Intro] {
        let branch_state = state.branch_mut(branch);
        let outputs = branch_state.take_outputs();
        let mut dispatched = Dispatched::default();
        for output in outputs {
            match output {
                Output::Int { .. } => dispatched.ints_published += 1,
                Output::ValuesRestored => dispatched.values_restored = true,
                Output::StartObjective { id } => {
                    let objective = branch_state
                        .objectives
                        .get(&id)
                        .ok_or(Failure::ProfileConfig)?;
                    if objective.active && !objective.completed {
                        dispatched.activated.push(id);
                    }
                }
                Output::ObjectivesRestored => dispatched.objectives_restored = true,
            }
        }
        result.insert(branch, dispatched);
    }
    Ok(result)
}
pub fn ready(dispatched: &BTreeMap<Branch, Dispatched>, speed_list_bypass: bool) -> bool {
    let restored = |branch| dispatched.get(&branch).is_some_and(Dispatched::restored);
    (speed_list_bypass || restored(Branch::Main)) && restored(Branch::Intro)
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Content {
    pub catalog: Catalog,
    pub construction: entities::Construction,
    pub settings: Settings,
    pub speed_list_bypass: bool,
    /// The main progression root's objective debug-print Hide events as
    /// (bus, ordinal), read from the installation at start (empty without it).
    /// Fired on that root once the participant has entered the game.
    pub debug_hides: Vec<(u32, u32)>,
}
impl Content {
    /// Attach the installed debug-print Hide events (bounded).
    pub fn with_debug_hides(mut self, hides: Vec<(u32, u32)>) -> Result<Self, Failure> {
        if hides.len() > MAX_DEBUG_HIDES {
            return Err(Failure::ProfileConfig);
        }
        self.debug_hides = hides;
        Ok(self)
    }
    pub fn load(path: &Path) -> Result<Self, Failure> {
        Ok(Self::from_definition(
            data::Content::load(path).map_err(parse_error)?,
        ))
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        Ok(Self::from_definition(
            data::Content::from_json(v).map_err(parse_error)?,
        ))
    }
    fn from_definition(value: data::Content) -> Self {
        Self {
            catalog: Catalog {
                definition: value.catalog,
            },
            construction: entities::Construction::from_definition(value.construction),
            settings: value.settings,
            speed_list_bypass: value.speed_list_bypass,
            debug_hides: Vec::new(),
        }
    }
}
/// Bound on debug-print Hide events (the supported build has 113).
const MAX_DEBUG_HIDES: usize = 1024;
fn int_cell(loaded: &Loaded, table: u32, id: u64, column: u32) -> Result<Option<i32>, Failure> {
    if !loaded.has_cell(table, id, column) {
        return Ok(None);
    }
    match loaded
        .tables()
        .get(&table)
        .and_then(|t| t.rows.get(&id))
        .and_then(|row| row.get(&column))
    {
        Some(Value::Int(v)) => Ok(Some(*v)),
        _ => Err(Failure::ProfileConfig),
    }
}

#[cfg(test)]
pub(crate) mod tests;
