// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One completed event's rewards as a single retry-safe account change: the REP
//! credit with its derived level and unlocks, the bank cash, the earned-cash
//! statistic and the account's race-event record. The caller establishes the
//! completion from owned event state and its rewards from installed content; this
//! module reads no wire request. A repeated batch ID is answered by the repository
//! from history, so a retried completion pays once.
use crate::{awards, bank::Balance, inventory, persistent, reputation};
use nfs_storage::{Batch, Op, Snapshot, tables::Value};

/// The earned-cash statistic (GameplayStatsTable store `CashEarned`, the 2050/71
/// `CEAR` value; E995 shows it rise by the event's cash).
const STATS_TABLE: &str = "GameplayStatsTable";
const CASH_EARNED: &str = "CashEarned";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    /// The race-event ID: the djb2-xor hash of the game-mode ID asset name.
    pub event_id: u32,
    /// The SpeedWall category of the event, the record's type string.
    pub kind: String,
    /// Zero-based finishing position (E995: a win is recorded as 0).
    pub position: u32,
    pub cash: u32,
    pub rep: reputation::Scores,
    /// The completion's retry key, stable across retries of this completion.
    pub batch_id: u64,
    /// Optional completed sprint statistics, committed with its rewards/record.
    pub speedwall: Option<crate::speedwall::Sprint>,
}

/// Prepare the completion from one initialized account snapshot (persistent tables
/// and the awards document present). The record keeps the best position and counts
/// completions in `attempts` (E995: one completion recorded attempts 1); its name is
/// the event ID in decimal, its screenshot ID 0 (E995's nonzero value is not
/// derived) and its modification time 0, as observed.
pub fn prepare(
    policy: &reputation::earned::Policy,
    snapshot: &Snapshot,
    completion: &Completion,
) -> Result<Batch, inventory::Error> {
    let r = completion.rep;
    let rep = [r.total, r.build, r.crew, r.outlaw, r.speed, r.style];
    if rep.iter().any(|v| *v < 0) || completion.kind.is_empty() || completion.kind.len() > 128 {
        return Err(nfs_storage::Error::Invalid.into());
    }
    let mut ops: Vec<Op> = if rep.iter().any(|v| *v != 0) {
        policy.prepare(snapshot, r, completion.batch_id)?.ops
    } else {
        Vec::new()
    };
    let balance = Balance::from_snapshot(snapshot)?;
    let cash = balance
        .0
        .checked_add(completion.cash)
        .ok_or(nfs_storage::Error::Bounds)?;
    ops.push(Balance(cash).store());
    let loaded = policy.tables.view(snapshot)?;
    let table_key = persistent::key(STATS_TABLE);
    let column = persistent::key(CASH_EARNED);
    let earned = *loaded
        .stores()
        .get(&(table_key, column))
        .ok_or(nfs_storage::Error::Config)?;
    let added = i32::try_from(completion.cash).map_err(|_| nfs_storage::Error::Bounds)?;
    let mut table = snapshot
        .tables
        .get(&table_key)
        .ok_or(nfs_storage::Error::Config)?
        .clone();
    table.rows.entry(0).or_default().insert(
        column,
        Value::Int(
            earned
                .checked_add(added)
                .ok_or(nfs_storage::Error::Bounds)?,
        ),
    );
    let mut proposed = snapshot.clone();
    proposed.tables.insert(table_key, table.clone());
    policy.tables.view(&proposed)?;
    ops.push(Op::SetTable(table_key, table));
    let mut state = awards::State::load(snapshot).map_err(|_| nfs_storage::Error::Config)?;
    match state
        .events
        .iter_mut()
        .find(|(id, _)| *id == completion.event_id)
    {
        Some((_, event)) => {
            if event.kind != completion.kind {
                return Err(nfs_storage::Error::Config.into());
            }
            event.attempts = event
                .attempts
                .checked_add(1)
                .ok_or(nfs_storage::Error::Bounds)?;
            event.position = event.position.min(completion.position);
        }
        None => state.events.push((
            completion.event_id,
            awards::Event {
                attempts: 1,
                position: completion.position,
                record: awards::Record {
                    name: completion.event_id.to_string(),
                    screenshot: 0,
                    modified: 0,
                },
                kind: completion.kind.clone(),
            },
        )),
    }
    ops.extend(state.operations().map_err(|_| nfs_storage::Error::Bounds)?);
    if let Some(record) = &completion.speedwall {
        if completion.kind != "SpeedWall_SprintRace" {
            return Err(nfs_storage::Error::Config.into());
        }
        ops.push(record.prepare(snapshot, completion.event_id)?);
    }
    let batch = Batch {
        id: completion.batch_id,
        expected_generation: snapshot.generation,
        ops,
    };
    batch.validate()?;
    Ok(batch)
}

#[cfg(test)]
mod tests;
