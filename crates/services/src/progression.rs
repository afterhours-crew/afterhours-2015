// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared static progression rules used by the server and local profile tools.
pub use crate::fresh_account::ObjectiveRow;
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};
const BUILD: &str = crate::SUPPORTED_BUILD_SHA256;
const MAX_BYTES: u64 = 512 * 1024;
pub const MAX_ENTRIES: usize = 2048;
pub const MAX_EXPRESSIONS: usize = 8192;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    Invalid,
    Limit,
    Io,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Branch {
    Main,
    Intro,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    Constant(bool),
    Completed(u64),
    ConsumerBeta,
    Not(Box<Expr>),
    All(Vec<Expr>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    objectives: Vec<(u64, Expr)>,
    ints: Vec<u64>,
    labels: BTreeMap<u64, String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    branches: BTreeMap<Branch, Definition>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub consumer_beta: bool,
}

/// The subset of prepared progression content used for checkpoint inspection
/// and editing. Runtime entity construction remains the content crate's job.
pub fn checkpoint_rules(value: &Json) -> Result<(Catalog, Settings), ParseError> {
    if value["format"] != "nfs-progression-entities"
        || value["version"] != 1
        || value["build_sha256"] != BUILD
    {
        return Err(ParseError::Invalid);
    }
    Ok((
        Catalog::from_json(&value["restoration"])?,
        Settings {
            consumer_beta: value["settings"]["consumer_beta"]
                .as_bool()
                .ok_or(ParseError::Invalid)?,
        },
    ))
}
fn fields(v: &Json, names: &[&str]) -> Result<(), ParseError> {
    let o = v.as_object().ok_or(ParseError::Invalid)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(ParseError::Invalid);
    }
    Ok(())
}
fn id(v: &Json) -> Result<u64, ParseError> {
    v.as_u64().filter(|v| *v != 0).ok_or(ParseError::Invalid)
}
fn expression(
    v: &Json,
    depth: usize,
    count: &mut usize,
    refs: &mut BTreeSet<u64>,
) -> Result<Expr, ParseError> {
    *count += 1;
    if depth > 16 || *count > MAX_EXPRESSIONS {
        return Err(ParseError::Limit);
    }
    let o = v
        .as_object()
        .filter(|o| o.len() == 1)
        .ok_or(ParseError::Invalid)?;
    let (key, v) = o.iter().next().ok_or(ParseError::Invalid)?;
    Ok(match key.as_str() {
        "constant" => Expr::Constant(v.as_bool().ok_or(ParseError::Invalid)?),
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
                .ok_or(ParseError::Invalid)?;
            Expr::All(
                values
                    .iter()
                    .map(|v| expression(v, depth + 1, count, refs))
                    .collect::<Result<_, _>>()?,
            )
        }
        _ => return Err(ParseError::Invalid),
    })
}
impl Expr {
    /// Parse one bounded authored activation expression independently of the
    /// two gameplay branches. Callers still validate its state ownership.
    pub fn from_json(value: &Json) -> Result<Self, ParseError> {
        expression(value, 0, &mut 0, &mut BTreeSet::new())
    }
    /// The activation value with no objective completed yet.
    pub fn initial(&self, settings: Settings) -> bool {
        self.evaluate(settings, &BTreeSet::new())
    }
    /// The activation value once the given objectives are completed.
    pub fn evaluate(&self, settings: Settings, completed: &BTreeSet<u64>) -> bool {
        match self {
            Self::Constant(v) => *v,
            Self::Completed(id) => completed.contains(id),
            Self::ConsumerBeta => settings.consumer_beta,
            Self::Not(v) => !v.evaluate(settings, completed),
            Self::All(v) => v.iter().all(|e| e.evaluate(settings, completed)),
        }
    }
}
impl Catalog {
    pub fn objective_label(&self, id: u64) -> Option<&str> {
        self.branches
            .values()
            .find_map(|d| d.labels.get(&id).map(String::as_str))
    }
    /// Derive a linear intro from its activation rules, rather than sorting IDs.
    /// A branching, cyclic or setting-disabled intro cannot be exposed as one slider.
    pub fn intro_order(&self, settings: Settings) -> Result<Vec<u64>, ParseError> {
        let definition = self
            .branches
            .get(&Branch::Intro)
            .ok_or(ParseError::Invalid)?;
        let mut completed = BTreeSet::new();
        let mut order = Vec::with_capacity(definition.objectives.len());
        while order.len() < definition.objectives.len() {
            let mut ready = definition.objectives.iter().filter(|(id, expression)| {
                !completed.contains(id) && expression.evaluate(settings, &completed)
            });
            let id = ready.next().ok_or(ParseError::Invalid)?.0;
            if ready.next().is_some() {
                return Err(ParseError::Invalid);
            }
            completed.insert(id);
            order.push(id);
        }
        Ok(order)
    }

    /// Project one coherent completion set into flags for every known objective.
    /// Persistence and reward effects are composed by the story service.
    pub fn project(
        &self,
        settings: Settings,
        completed: &BTreeSet<u64>,
    ) -> Result<Vec<ObjectiveRow>, ParseError> {
        let ids = self
            .branches
            .values()
            .flat_map(|definition| definition.objectives.iter().map(|(id, _)| *id))
            .collect::<BTreeSet<_>>();
        if !completed.is_subset(&ids) {
            return Err(ParseError::Invalid);
        }
        Ok(self
            .branches
            .values()
            .flat_map(|definition| {
                definition
                    .objectives
                    .iter()
                    .map(|(id, expression)| ObjectiveRow {
                        id: *id,
                        active: !completed.contains(id) && expression.evaluate(settings, completed),
                        completed: completed.contains(id),
                    })
            })
            .collect())
    }

    pub fn branches(&self) -> &BTreeMap<Branch, Definition> {
        &self.branches
    }
    /// Saved objective rows of an account whose whole intro branch is complete
    /// and whose main objectives activated by that completion are active: the
    /// state after the official fresh career's prologue (E742: the intro
    /// objectives advance in order and the last one leaves at the garage exit
    /// after the player's PageDown entry), without the prologue's scripted
    /// events, which an owned host does not run. Every intro objective is
    /// stored inactive and completed; a main objective is stored active and
    /// incomplete when its activation holds with exactly the intro completed.
    /// Objectives without a row keep their wired initial state.
    pub fn post_intro(&self, settings: Settings) -> Vec<ObjectiveRow> {
        let intro = self.objective_ids(Branch::Intro);
        let mut rows: Vec<_> = intro
            .iter()
            .map(|&id| ObjectiveRow {
                id,
                active: false,
                completed: true,
            })
            .collect();
        if let Some(main) = self.branches.get(&Branch::Main) {
            rows.extend(
                main.objectives
                    .iter()
                    .filter(|(_, expr)| expr.evaluate(settings, &intro))
                    .map(|&(id, _)| ObjectiveRow {
                        id,
                        active: true,
                        completed: false,
                    }),
            );
        }
        rows
    }

    pub fn objective_ids(&self, branch: Branch) -> BTreeSet<u64> {
        self.branches
            .get(&branch)
            .map(|d| d.objectives.iter().map(|(id, _)| *id).collect())
            .unwrap_or_default()
    }
    pub fn load(path: &Path) -> Result<Self, ParseError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ParseError::Io)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ParseError::Io)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(ParseError::Limit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ParseError::Invalid)?)
    }
    pub fn from_json(v: &Json) -> Result<Self, ParseError> {
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
            return Err(ParseError::Invalid);
        }
        let raw = v["branches"]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or(ParseError::Invalid)?;
        let mut branches = BTreeMap::new();
        let (mut objective_ids, mut int_ids, mut refs) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut count = 0;
        for b in raw {
            fields(b, &["name", "objectives", "ints"])?;
            let branch = match b["name"].as_str() {
                Some("main") => Branch::Main,
                Some("intro") => Branch::Intro,
                _ => return Err(ParseError::Invalid),
            };
            let rows = b["objectives"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= MAX_ENTRIES)
                .ok_or(ParseError::Invalid)?;
            let mut objectives = Vec::new();
            let mut labels = BTreeMap::new();
            for r in rows {
                if r.get("label").is_some() {
                    fields(r, &["id", "active", "label"])?;
                } else {
                    fields(r, &["id", "active"])?;
                }
                let id = id(&r["id"])?;
                if !objective_ids.insert(id) || objective_ids.len() > MAX_ENTRIES {
                    return Err(ParseError::Invalid);
                }
                objectives.push((id, expression(&r["active"], 0, &mut count, &mut refs)?));
                if let Some(label) = r.get("label") {
                    let label = label
                        .as_str()
                        .filter(|s| {
                            !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
                        })
                        .ok_or(ParseError::Invalid)?;
                    labels.insert(id, label.to_owned());
                }
            }
            let rows = b["ints"]
                .as_array()
                .filter(|a| a.len() <= MAX_ENTRIES)
                .ok_or(ParseError::Invalid)?;
            let mut ints = Vec::new();
            for r in rows {
                let id = id(r)?;
                if !int_ids.insert(id) || int_ids.len() > MAX_ENTRIES {
                    return Err(ParseError::Invalid);
                }
                ints.push(id);
            }
            if branches
                .insert(
                    branch,
                    Definition {
                        objectives,
                        ints,
                        labels,
                    },
                )
                .is_some()
            {
                return Err(ParseError::Invalid);
            }
        }
        if !refs.is_subset(&objective_ids) {
            return Err(ParseError::Invalid);
        }
        Ok(Self { branches })
    }
}
impl Definition {
    pub fn objectives(&self) -> &[(u64, Expr)] {
        &self.objectives
    }
    pub fn ints(&self) -> &[u64] {
        &self.ints
    }
}
