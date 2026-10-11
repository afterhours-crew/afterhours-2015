// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{Failure, content::BUILD, persistent::Loaded};
use nfs_storage::tables::Value;
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

pub mod entities;

pub const FORMAT: &str = "nfs-progression-entities";
pub const VERSION: u8 = 1;
const MAX_BYTES: u64 = 512 * 1024;
const MAX_ENTRIES: usize = 2048;
const MAX_SOURCES: usize = 16;
const MAX_EXPRESSIONS: usize = 8192;
const OBJECTIVES: u32 = 2251697951;
const VALUES: u32 = 52582350;
const ACTIVE: u32 = 2484178249;
const COMPLETED: u32 = 1928661128;
const INTS: u32 = 2087826853;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Branch {
    Main,
    Intro,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Expr {
    Constant(bool),
    Completed(u64),
    ConsumerBeta,
    Not(Box<Expr>),
    All(Vec<Expr>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Definition {
    objectives: Vec<(u64, Expr)>,
    ints: Vec<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    branches: BTreeMap<Branch, Definition>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub consumer_beta: bool,
}
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
fn fields(v: &Json, names: &[&str]) -> Result<(), Failure> {
    let o = v.as_object().ok_or(Failure::ProfileConfig)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
fn id(v: &Json) -> Result<u64, Failure> {
    v.as_u64().filter(|v| *v != 0).ok_or(Failure::ProfileConfig)
}
fn expression(
    v: &Json,
    depth: usize,
    count: &mut usize,
    refs: &mut BTreeSet<u64>,
) -> Result<Expr, Failure> {
    *count += 1;
    if depth > 16 || *count > MAX_EXPRESSIONS {
        return Err(Failure::BodyLimit);
    }
    let o = v
        .as_object()
        .filter(|o| o.len() == 1)
        .ok_or(Failure::ProfileConfig)?;
    let (key, v) = o.iter().next().ok_or(Failure::ProfileConfig)?;
    Ok(match key.as_str() {
        "constant" => Expr::Constant(v.as_bool().ok_or(Failure::ProfileConfig)?),
        "completed" => {
            let id = id(v)?;
            refs.insert(id);
            Expr::Completed(id)
        }
        "setting" if v == "Gameplay.NFS16ConsumerBeta" => Expr::ConsumerBeta,
        "negate" => Expr::Not(Box::new(expression(v, depth + 1, count, refs)?)),
        "all" => {
            let values = v
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= 16)
                .ok_or(Failure::ProfileConfig)?;
            Expr::All(
                values
                    .iter()
                    .map(|v| expression(v, depth + 1, count, refs))
                    .collect::<Result<_, _>>()?,
            )
        }
        _ => return Err(Failure::ProfileConfig),
    })
}
impl Expr {
    fn initial(&self, settings: Settings) -> bool {
        match self {
            Self::Constant(v) => *v,
            Self::Completed(_) => false,
            Self::ConsumerBeta => settings.consumer_beta,
            Self::Not(v) => !v.initial(settings),
            Self::All(v) => v.iter().all(|e| e.initial(settings)),
        }
    }
}
impl Catalog {
    pub fn objective_ids(&self, branch: Branch) -> BTreeSet<u64> {
        self.branches
            .get(&branch)
            .map(|d| d.objectives.iter().map(|(id, _)| *id).collect())
            .unwrap_or_default()
    }
    pub fn load(path: &Path) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?)
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        fields(v, &["version", "build_sha256", "source", "branches"])?;
        fields(&v["source"], &["path", "sha256"])?;
        if v["version"] != 1
            || v["build_sha256"] != BUILD
            || !v["source"]["path"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 512)
            || !v["source"]["sha256"]
                .as_str()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Failure::ProfileConfig);
        }
        let raw = v["branches"]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or(Failure::ProfileConfig)?;
        let mut branches = BTreeMap::new();
        let (mut objective_ids, mut int_ids, mut refs) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut count = 0;
        for b in raw {
            fields(b, &["name", "objectives", "ints"])?;
            let branch = match b["name"].as_str() {
                Some("main") => Branch::Main,
                Some("intro") => Branch::Intro,
                _ => return Err(Failure::ProfileConfig),
            };
            let rows = b["objectives"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= MAX_ENTRIES)
                .ok_or(Failure::ProfileConfig)?;
            let mut objectives = Vec::new();
            for r in rows {
                fields(r, &["id", "active"])?;
                let id = id(&r["id"])?;
                if !objective_ids.insert(id) || objective_ids.len() > MAX_ENTRIES {
                    return Err(Failure::ProfileConfig);
                }
                objectives.push((id, expression(&r["active"], 0, &mut count, &mut refs)?));
            }
            let rows = b["ints"]
                .as_array()
                .filter(|a| a.len() <= MAX_ENTRIES)
                .ok_or(Failure::ProfileConfig)?;
            let mut ints = Vec::new();
            for r in rows {
                let id = id(r)?;
                if !int_ids.insert(id) || int_ids.len() > MAX_ENTRIES {
                    return Err(Failure::ProfileConfig);
                }
                ints.push(id);
            }
            if branches
                .insert(branch, Definition { objectives, ints })
                .is_some()
            {
                return Err(Failure::ProfileConfig);
            }
        }
        if !refs.is_subset(&objective_ids) {
            return Err(Failure::ProfileConfig);
        }
        Ok(Self { branches })
    }
    pub fn restore(&self, loaded: &Loaded, settings: Settings) -> Result<Restored, Failure> {
        if !loaded.tables().contains_key(&OBJECTIVES) || !loaded.tables().contains_key(&VALUES) {
            return Err(Failure::ProfileConfig);
        }
        let mut branches = BTreeMap::new();
        for (&branch, definition) in &self.branches {
            let mut state = BranchState {
                objectives: BTreeMap::new(),
                ints: BTreeMap::new(),
                pending: Vec::new(),
            };
            for &id in &definition.ints {
                let cell = int_cell(loaded, VALUES, id, INTS)?;
                state.ints.insert(id, cell.unwrap_or(0));
                if let Some(value) = cell {
                    state.pending.push(Output::Int { id, value });
                }
            }
            state.pending.push(Output::ValuesRestored);
            for (id, expr) in &definition.objectives {
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
}
impl Content {
    pub fn load(path: &Path) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?)
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        fields(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "sources",
                "restoration",
                "entities",
                "settings",
            ],
        )?;
        if v["format"] != FORMAT || v["version"] != VERSION || v["build_sha256"] != BUILD {
            return Err(Failure::ProfileConfig);
        }
        let sources = v["sources"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= MAX_SOURCES)
            .ok_or(Failure::ProfileConfig)?;
        for source in sources {
            fields(source, &["path", "sha256"])?;
            if !source["path"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 512)
                || !source["sha256"]
                    .as_str()
                    .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(Failure::ProfileConfig);
            }
        }
        fields(&v["settings"], &["consumer_beta", "enable_speed_lists"])?;
        let setting = |name: &str| v["settings"][name].as_bool().ok_or(Failure::ProfileConfig);
        let catalog = Catalog::from_json(&v["restoration"])?;
        let construction = entities::Construction::from_json(&v["entities"])?;
        for branch in [Branch::Main, Branch::Intro] {
            construction.check_objectives(branch, &catalog.objective_ids(branch))?;
        }
        Ok(Self {
            catalog,
            construction,
            settings: Settings {
                consumer_beta: setting("consumer_beta")?,
            },
            speed_list_bypass: setting("enable_speed_lists")?,
        })
    }
}

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
