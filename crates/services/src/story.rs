// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned intro checkpoints and transactional progression plans.
//!
//! This module never plays a movie, trusts a client-reported success or invents
//! rewards. The owning game service validates completion and supplies any reward
//! operations; those join objective state and durable receipts in one batch.
use crate::{
    persistent::key,
    progression::{Branch, Catalog, Settings},
};
use nfs_storage::{
    Batch, Error, Op, Snapshot,
    tables::{Row, Table, Value},
};
use std::collections::{BTreeMap, BTreeSet};

const VERSION: i32 = 1;
const HEADER: u64 = 0;
const FORMAT: u32 = 0;
const OPERATION: u32 = 1;
const BEFORE: u32 = 2;
const TARGET: u32 = 3;
const ACTION: u32 = 4;
const PLAYED: u32 = 0;
const EDITED: u32 = 1;
const CLAIMED: u32 = 2;
const EXISTING: u32 = 3;
const BATCH_NAMESPACE: u64 = 3 << 60;
const OPERATION_MASK: u64 = (1 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Checkpoint {
    Objective(u64),
    AfterIntro,
}
impl Checkpoint {
    fn id(self) -> u64 {
        match self {
            Self::Objective(id) => id,
            Self::AfterIntro => 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub id: u64,
    pub active: bool,
    pub completed: bool,
    pub played: bool,
    pub edited: bool,
    pub existing: bool,
    pub reward_claimed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct View {
    pub generation: u64,
    pub position: usize,
    pub milestones: Vec<Milestone>,
    /// Older post-intro profiles may not have stored the summary cell. The
    /// objective rows remain authoritative; the next mutation reconciles it.
    pub stats_consistent: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Prepared {
    Mutation(Batch),
    /// Exact retry of the latest committed story operation. No effects run again.
    Replayed,
}

pub struct Policy<'a> {
    pub catalog: &'a Catalog,
    pub settings: Settings,
}

struct Receipt {
    operation: u64,
    target: Checkpoint,
    action: i32,
}

fn flag(row: Option<&Row>, column: u32) -> Result<bool, Error> {
    match row.and_then(|row| row.get(&column)) {
        None | Some(Value::Int(0)) => Ok(false),
        Some(Value::Int(1)) => Ok(true),
        _ => Err(Error::Config),
    }
}

fn decimal(row: &Row, column: u32) -> Result<u64, Error> {
    let Some(Value::String(text)) = row.get(&column) else {
        return Err(Error::Config);
    };
    let value = text.parse::<u64>().map_err(|_| Error::Config)?;
    if value.to_string() != *text {
        return Err(Error::Config);
    }
    Ok(value)
}

impl Policy<'_> {
    fn ledger(&self, snapshot: &Snapshot) -> Result<Table, Error> {
        let Some(table) = snapshot.tables.get(&key("AfterhoursStory")) else {
            return Ok(Table::default());
        };
        table.validate()?;
        let header = table.rows.get(&HEADER).ok_or(Error::Config)?;
        if header.get(&FORMAT) != Some(&Value::Int(VERSION)) || header.len() != 5 {
            return Err(Error::Config);
        }
        let operation = decimal(header, OPERATION)?;
        let before = decimal(header, BEFORE)?;
        let target = decimal(header, TARGET)?;
        let ids = self.catalog.objective_ids(Branch::Intro);
        if operation == 0
            || operation > OPERATION_MASK
            || before >= snapshot.generation
            || (target != 0 && !ids.contains(&target))
            || (target == 0 && header.get(&ACTION) == Some(&Value::Int(1)))
            || !matches!(header.get(&ACTION), Some(Value::Int(0 | 1)))
            || table.rows.len() != ids.len() + 1
        {
            return Err(Error::Config);
        }
        for (id, row) in &table.rows {
            if *id == HEADER {
                continue;
            }
            if !ids.contains(id)
                || row.len() != 4
                || [PLAYED, EDITED, CLAIMED, EXISTING]
                    .iter()
                    .any(|column| !row.contains_key(column))
            {
                return Err(Error::Config);
            }
            for column in [PLAYED, EDITED, CLAIMED, EXISTING] {
                flag(Some(row), column)?;
            }
        }
        Ok(table.clone())
    }

    fn objectives(&self, snapshot: &Snapshot) -> Result<BTreeMap<u64, (bool, bool)>, Error> {
        let table = snapshot
            .tables
            .get(&key("ProgressionObjective"))
            .ok_or(Error::Config)?;
        table.validate()?;
        let mut values = BTreeMap::new();
        for definition in self.catalog.branches().values() {
            for (id, expression) in definition.objectives() {
                let row = table.rows.get(id);
                let active = row.and_then(|r| r.get(&key("Active")));
                let completed = row.and_then(|r| r.get(&key("Completed")));
                let flags = match (active, completed) {
                    (None, None) => (expression.initial(self.settings), false),
                    (Some(Value::Int(a)), Some(Value::Int(c))) => (*a > 0, *c > 0),
                    _ => return Err(Error::Config),
                };
                values.insert(*id, flags);
            }
        }
        Ok(values)
    }

    pub fn inspect(&self, snapshot: &Snapshot) -> Result<View, Error> {
        let order = self
            .catalog
            .intro_order(self.settings)
            .map_err(|_| Error::Config)?;
        let ledger = self.ledger(snapshot)?;
        let objectives = self.objectives(snapshot)?;
        let position = order.iter().take_while(|id| objectives[id].1).count();
        if order[position..].iter().any(|id| objectives[id].1) {
            return Err(Error::Config);
        }
        let milestones = order
            .into_iter()
            .map(|id| {
                let (active, completed) = objectives[&id];
                let row = ledger.rows.get(&id);
                Ok(Milestone {
                    id,
                    active: active && !completed,
                    completed,
                    played: flag(row, PLAYED)?,
                    edited: flag(row, EDITED)?,
                    existing: flag(row, EXISTING)? || (completed && row.is_none()),
                    reward_claimed: flag(row, CLAIMED)?,
                })
            })
            .collect::<Result<_, Error>>()?;
        let stats = snapshot
            .tables
            .get(&key("ProgressionStats"))
            .ok_or(Error::Config)?;
        let summary = stats
            .rows
            .get(&0)
            .and_then(|row| row.get(&key("IntroCompleted")));
        let stats_consistent = match summary {
            None => position == 0,
            Some(Value::Int(value)) => {
                (*value > 0) == (position == self.catalog.objective_ids(Branch::Intro).len())
            }
            _ => return Err(Error::Config),
        };
        if !stats_consistent && !ledger.rows.is_empty() {
            return Err(Error::Config);
        }
        Ok(View {
            generation: snapshot.generation,
            position,
            milestones,
            stats_consistent,
        })
    }

    fn replay(
        &self,
        snapshot: &Snapshot,
        expected: u64,
        operation: u64,
        target: Checkpoint,
        action: i32,
    ) -> Result<bool, Error> {
        if operation == 0 || operation > OPERATION_MASK {
            return Err(Error::Bounds);
        }
        if let Checkpoint::Objective(id) = target
            && !self.catalog.objective_ids(Branch::Intro).contains(&id)
        {
            return Err(Error::Invalid);
        }
        let table = self.ledger(snapshot)?;
        let Some(header) = table.rows.get(&HEADER) else {
            return Ok(false);
        };
        let last = decimal(header, OPERATION)?;
        if operation < last {
            return Err(Error::Conflict);
        }
        if operation > last {
            return Ok(false);
        }
        if decimal(header, BEFORE)? != expected
            || decimal(header, TARGET)? != target.id()
            || header.get(&ACTION) != Some(&Value::Int(action))
        {
            return Err(Error::Conflict);
        }
        Ok(true)
    }

    /// Preview a checkpoint change without writing or granting anything. The
    /// caller must hold the idle-account lock and recheck expected generation at
    /// apply. Existing main-story completions and unrelated tables are retained.
    pub fn jump(
        &self,
        snapshot: &Snapshot,
        expected: u64,
        operation: u64,
        target: Checkpoint,
    ) -> Result<Prepared, Error> {
        if self.replay(snapshot, expected, operation, target, 0)? {
            return Ok(Prepared::Replayed);
        }
        if snapshot.generation != expected {
            return Err(Error::Conflict);
        }
        let view = self.inspect(snapshot)?;
        let position = match target {
            Checkpoint::AfterIntro => view.milestones.len(),
            Checkpoint::Objective(id) => view
                .milestones
                .iter()
                .position(|m| m.id == id)
                .ok_or(Error::Invalid)?,
        };
        self.prepare(
            snapshot,
            Receipt {
                operation,
                target,
                action: 0,
            },
            position,
            None,
            Vec::new(),
        )
    }

    /// The caller has established success for this active authored objective.
    /// Optional rewards must already be calculated and validated by their domain
    /// services; duplicate claims and conflicting story-table effects are refused.
    pub fn complete(
        &self,
        snapshot: &Snapshot,
        expected: u64,
        operation: u64,
        objective: u64,
        effects: Vec<Op>,
    ) -> Result<Prepared, Error> {
        let target = Checkpoint::Objective(objective);
        if self.replay(snapshot, expected, operation, target, 1)? {
            return Ok(Prepared::Replayed);
        }
        if snapshot.generation != expected {
            return Err(Error::Conflict);
        }
        let view = self.inspect(snapshot)?;
        let milestone = view
            .milestones
            .get(view.position)
            .filter(|m| m.id == objective && m.active && !m.completed)
            .ok_or(Error::Invalid)?;
        if !effects.is_empty() && (milestone.reward_claimed || milestone.existing) {
            return Err(Error::Conflict);
        }
        self.prepare(
            snapshot,
            Receipt {
                operation,
                target,
                action: 1,
            },
            view.position + 1,
            Some(objective),
            effects,
        )
    }

    fn prepare(
        &self,
        snapshot: &Snapshot,
        receipt: Receipt,
        position: usize,
        played: Option<u64>,
        effects: Vec<Op>,
    ) -> Result<Prepared, Error> {
        let Receipt {
            operation,
            target,
            action,
        } = receipt;
        let view = self.inspect(snapshot)?;
        let mut ledger = self.ledger(snapshot)?;
        for milestone in &view.milestones {
            ledger.rows.entry(milestone.id).or_insert_with(|| {
                BTreeMap::from([
                    (PLAYED, Value::Int(milestone.played.into())),
                    (EDITED, Value::Int(milestone.edited.into())),
                    (CLAIMED, Value::Int(milestone.reward_claimed.into())),
                    (EXISTING, Value::Int(milestone.existing.into())),
                ])
            });
        }
        let mut completed = self
            .objectives(snapshot)?
            .into_iter()
            .filter_map(|(id, (_, complete))| complete.then_some(id))
            .collect::<BTreeSet<_>>();
        for (index, milestone) in view.milestones.iter().enumerate() {
            if index < position {
                completed.insert(milestone.id);
            } else {
                completed.remove(&milestone.id);
            }
            let row = ledger.rows.get_mut(&milestone.id).ok_or(Error::Config)?;
            if played == Some(milestone.id) {
                row.insert(PLAYED, Value::Int(1));
                if !effects.is_empty() {
                    row.insert(CLAIMED, Value::Int(1));
                }
            } else if index < position && !milestone.completed {
                row.insert(EDITED, Value::Int(1));
            }
        }
        ledger.rows.insert(
            HEADER,
            BTreeMap::from([
                (FORMAT, Value::Int(VERSION)),
                (OPERATION, Value::String(operation.to_string())),
                (BEFORE, Value::String(snapshot.generation.to_string())),
                (TARGET, Value::String(target.id().to_string())),
                (ACTION, Value::Int(action)),
            ]),
        );
        let mut objectives = snapshot.tables[&key("ProgressionObjective")].clone();
        for objective in self
            .catalog
            .project(self.settings, &completed)
            .map_err(|_| Error::Config)?
        {
            let row = objectives.rows.entry(objective.id).or_default();
            row.insert(key("Active"), Value::Int(objective.active.into()));
            row.insert(key("Completed"), Value::Int(objective.completed.into()));
        }
        let mut stats = snapshot
            .tables
            .get(&key("ProgressionStats"))
            .ok_or(Error::Config)?
            .clone();
        stats.rows.entry(0).or_default().insert(
            key("IntroCompleted"),
            Value::Int((position == view.milestones.len()).into()),
        );
        let mut ops = vec![
            Op::SetTable(key("ProgressionObjective"), objectives),
            Op::SetTable(key("AfterhoursStory"), ledger),
            Op::SetTable(key("ProgressionStats"), stats),
        ];
        ops.extend(effects);
        let batch = Batch {
            id: BATCH_NAMESPACE | operation,
            expected_generation: snapshot.generation,
            ops,
        };
        batch.validate()?;
        Ok(Prepared::Mutation(batch))
    }
}

#[cfg(test)]
mod tests;
