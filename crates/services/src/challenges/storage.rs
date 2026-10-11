// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::persistent::key;
use nfs_storage::{
    AccountId, Batch, InventoryRepository, Op, Timestamp,
    tables::{Row, Table, Value},
};
use serde_json::{Value as Json, json};

const NAMES: [&str; 4] = [
    "AfterhoursChallengesMeta",
    "AfterhoursChallengeProgress",
    "AfterhoursChallengeAwards",
    "AfterhoursChallengeRanks",
];
const INITIAL_BATCH: u64 = u64::MAX - 3;
fn fields<'a>(value: &'a Json, names: &[&str]) -> Result<&'a Json, Error> {
    let object = value.as_object().ok_or(Error::State)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(Error::State);
    }
    Ok(value)
}
fn unsigned(v: &Json) -> Result<u32, Error> {
    v.as_u64()
        .and_then(|v| v.try_into().ok())
        .ok_or(Error::State)
}
fn rows(snapshot: &Snapshot, index: usize) -> Result<Vec<Json>, Error> {
    let table = snapshot
        .tables
        .get(&key(NAMES[index]))
        .ok_or(Error::State)?;
    table.validate().map_err(|_| Error::State)?;
    if table.rows.len() > 512 {
        return Err(Error::State);
    }
    table
        .rows
        .iter()
        .enumerate()
        .map(|(ordinal, (&id, row))| {
            if id != ordinal as u64 || row.len() != 1 {
                return Err(Error::State);
            }
            let Some(Value::String(text)) = row.get(&key("Document")) else {
                return Err(Error::State);
            };
            if text.len() > 256 {
                return Err(Error::State);
            }
            let value: Json = serde_json::from_str(text).map_err(|_| Error::State)?;
            let canonical = serde_json::to_string(&value).map_err(|_| Error::State)?;
            if canonical != *text {
                return Err(Error::State);
            }
            Ok(value)
        })
        .collect()
}
impl State {
    pub(super) fn validate(&self) -> Result<(), Error> {
        if [
            self.progress.len(),
            self.obtained.len(),
            self.monthly_ranks.len(),
        ]
        .into_iter()
        .any(|n| n > 512)
        {
            return Err(Error::State);
        }
        Ok(())
    }
    pub fn load(snapshot: &Snapshot) -> Result<Self, Error> {
        let meta = rows(snapshot, 0)?;
        if meta.len() != 1 || fields(&meta[0], &["version"])?["version"] != 1 {
            return Err(Error::State);
        }
        let mut state = Self::default();
        for p in rows(snapshot, 1)? {
            fields(&p, &["day", "id", "complete", "one", "two"])?;
            let key = (unsigned(&p["day"])?, unsigned(&p["id"])?);
            let value = Progress {
                complete: p["complete"].as_bool().ok_or(Error::State)?,
                count_one: unsigned(&p["one"])?,
                count_two: unsigned(&p["two"])?,
            };
            if state.progress.insert(key, value).is_some() {
                return Err(Error::State);
            }
        }
        for a in rows(snapshot, 2)? {
            fields(&a, &["day", "challenge", "award"])?;
            if !state.obtained.insert((
                unsigned(&a["day"])?,
                unsigned(&a["challenge"])?,
                a["award"].as_u64().ok_or(Error::State)?,
            )) {
                return Err(Error::State);
            }
        }
        for r in rows(snapshot, 3)? {
            fields(&r, &["month", "rank"])?;
            if state
                .monthly_ranks
                .insert(
                    r["month"].as_i64().ok_or(Error::State)?,
                    unsigned(&r["rank"])?,
                )
                .is_some()
            {
                return Err(Error::State);
            }
        }
        state.validate()?;
        Ok(state)
    }
    /// Compose these validated changes with other progression operations in one
    /// caller-owned retry-safe batch. This API does not authorize game mutations.
    pub fn operations(&self) -> Result<Vec<Op>, Error> {
        self.validate()?;
        let values:[Vec<Json>;4]=[
            vec![json!({"version":1})],
            self.progress.iter().map(|(&(day,id),p)|json!({"day":day,"id":id,"complete":p.complete,"one":p.count_one,"two":p.count_two})).collect(),
            self.obtained.iter().map(|&(day,challenge,award)|json!({"day":day,"challenge":challenge,"award":award})).collect(),
            self.monthly_ranks.iter().map(|(&month,&rank)|json!({"month":month,"rank":rank})).collect(),
        ];
        let mut snapshot = Snapshot::default();
        for (name, values) in NAMES.into_iter().zip(values) {
            snapshot.tables.insert(
                key(name),
                Table {
                    rows: values
                        .into_iter()
                        .enumerate()
                        .map(|(id, value)| {
                            (
                                id as u64,
                                Row::from([(key("Document"), Value::String(value.to_string()))]),
                            )
                        })
                        .collect(),
                },
            );
        }
        Self::load(&snapshot)?;
        Ok(snapshot
            .tables
            .into_iter()
            .map(|(id, table)| Op::SetTable(id, table))
            .collect())
    }
}

/// Explicit empty local account initialization; no awards or history are granted.
/// Existing or partially initialized state is never overwritten with defaults.
pub fn ensure_empty(
    repository: &dyn InventoryRepository,
    account: AccountId,
    mut snapshot: Snapshot,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    if NAMES
        .iter()
        .all(|name| !snapshot.tables.contains_key(&key(name)))
    {
        match repository.apply(
            account,
            &Batch {
                id: INITIAL_BATCH,
                expected_generation: snapshot.generation,
                ops: State::default().operations()?,
            },
            now,
        ) {
            Ok(_) | Err(nfs_storage::Error::Conflict) => {}
            Err(_) => return Err(Error::State),
        }
        snapshot = repository.snapshot(account).map_err(|_| Error::State)?;
    }
    State::load(&snapshot)?;
    Ok(snapshot)
}
