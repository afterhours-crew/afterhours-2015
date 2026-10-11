// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local schema v1. Each bounded record is a JSON cell in an account-owned
//! storage table; these tables are not exposed as game persistent tables.
use super::*;
use nfs_storage::{AccountId, Batch, InventoryRepository, Op, Timestamp, tables::Table};
use serde_json::{Value as Json, json};

const NAMES: [&str; 6] = [
    "AfterhoursAwardsMeta",
    "AfterhoursActivityRecords",
    "AfterhoursCollectibleRecords",
    "AfterhoursRaceRecords",
    "AfterhoursObjectiveTimes",
    "AfterhoursAwardEntitlements",
];
const INITIAL_BATCH: u64 = u64::MAX - 2;
fn fields<'a>(value: &'a Json, names: &[&str]) -> Result<&'a Json, Error> {
    let object = value.as_object().ok_or(Error::State)?;
    if object.len() != names.len() || !names.iter().all(|k| object.contains_key(*k)) {
        return Err(Error::State);
    }
    Ok(value)
}
fn number(value: &Json) -> Result<u64, Error> {
    value.as_u64().ok_or(Error::State)
}
fn unsigned(value: &Json) -> Result<u32, Error> {
    number(value)?.try_into().map_err(|_| Error::State)
}
fn string(value: &Json) -> Result<String, Error> {
    value
        .as_str()
        .filter(|s| s.len() < 1024 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(Error::State)
}
fn record(value: &Json) -> Result<Record, Error> {
    fields(value, &["name", "screenshot", "modified"])?;
    Ok(Record {
        name: string(&value["name"])?,
        screenshot: number(&value["screenshot"])?,
        modified: number(&value["modified"])?,
    })
}
fn record_json(record: &Record) -> Json {
    json!({"name":record.name,"screenshot":record.screenshot,"modified":record.modified})
}
fn decode_rows(snapshot: &Snapshot, index: usize) -> Result<BTreeMap<u64, Json>, Error> {
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
        .map(|(&id, row)| {
            if row.len() != 1 {
                return Err(Error::State);
            }
            let Some(Value::String(text)) = row.get(&key("Document")) else {
                return Err(Error::State);
            };
            let value: Json = serde_json::from_str(text).map_err(|_| Error::State)?;
            // Comparing the serialized document (not Json's string value)
            // rejects duplicate keys and noncanonical local storage records.
            let canonical = serde_json::to_string(&value).map_err(|_| Error::State)?;
            if canonical != *text {
                return Err(Error::State);
            }
            Ok((id, value))
        })
        .collect()
}
fn records(rows: BTreeMap<u64, Json>) -> Result<BTreeMap<u32, Record>, Error> {
    rows.into_iter()
        .map(|(id, r)| Ok((id.try_into().map_err(|_| Error::State)?, record(&r)?)))
        .collect()
}
impl State {
    /// Missing/partial/unknown-version documents fail; initialization is explicit.
    pub fn load(snapshot: &Snapshot) -> Result<Self, Error> {
        let meta = decode_rows(snapshot, 0)?;
        if meta.len() != 1 || !meta.contains_key(&0) {
            return Err(Error::State);
        }
        let meta = fields(
            &meta[&0],
            &[
                "version",
                "rep_modified",
                "received",
                "reward",
                "sent",
                "screenshots",
            ],
        )?;
        if meta["version"] != 1 {
            return Err(Error::State);
        }
        let mut event_ids = std::collections::BTreeSet::new();
        let events = decode_rows(snapshot, 3)?
            .into_iter()
            .enumerate()
            .map(|(ordinal, (row, value))| {
                fields(&value, &["id", "attempts", "position", "record", "kind"])?;
                let id = unsigned(&value["id"])?;
                if row != ordinal as u64 || !event_ids.insert(id) {
                    return Err(Error::State);
                }
                Ok((
                    id,
                    Event {
                        attempts: unsigned(&value["attempts"])?,
                        position: unsigned(&value["position"])?,
                        record: record(&value["record"])?,
                        kind: string(&value["kind"])?,
                    },
                ))
            })
            .collect::<Result<_, Error>>()?;
        let objective_times = decode_rows(snapshot, 4)?
            .into_iter()
            .map(|(id, value)| Ok((id, number(&value)?)))
            .collect::<Result<_, Error>>()?;
        let entitlements = decode_rows(snapshot, 5)?
            .into_iter()
            .map(|(id, value)| {
                let id: u32 = id.try_into().map_err(|_| Error::State)?;
                Ok((id as i32, value.as_bool().ok_or(Error::State)?))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            activities: records(decode_rows(snapshot, 1)?)?,
            collectibles: records(decode_rows(snapshot, 2)?)?,
            events,
            objective_times,
            rep_modified: number(&meta["rep_modified"])?,
            kickbacks: [
                unsigned(&meta["received"])?,
                unsigned(&meta["reward"])?,
                unsigned(&meta["sent"])?,
                unsigned(&meta["screenshots"])?,
            ],
            entitlements,
        })
    }
    /// Validate and produce operations to commit with related progression writes
    /// in one expected-generation/retry-key repository batch. No I/O is hidden.
    pub fn operations(&self) -> Result<Vec<Op>, Error> {
        if [
            self.activities.len(),
            self.collectibles.len(),
            self.events.len(),
            self.objective_times.len(),
            self.entitlements.len(),
        ]
        .into_iter()
        .any(|n| n > 512)
        {
            return Err(Error::State);
        }
        let [received, reward, sent, screenshots] = self.kickbacks;
        let rows: [BTreeMap<u64, Json>; 6] = [
            BTreeMap::from([(0, json!({"version":1,"rep_modified":self.rep_modified,"received":received,"reward":reward,"sent":sent,"screenshots":screenshots}))]),
            self.activities.iter().map(|(&id, r)| (u64::from(id), record_json(r))).collect(),
            self.collectibles.iter().map(|(&id, r)| (u64::from(id), record_json(r))).collect(),
            self.events.iter().enumerate().map(|(ordinal, (id, e))| (ordinal as u64, json!({"id":id,"attempts":e.attempts,"position":e.position,"record":record_json(&e.record),"kind":e.kind}))).collect(),
            self.objective_times.iter().map(|(&id, &v)| (id, json!(v))).collect(),
            self.entitlements.iter().map(|(&id, &v)| (u64::from(id as u32), json!(v))).collect(),
        ];
        let mut snapshot = Snapshot::default();
        for (name, rows) in NAMES.into_iter().zip(rows) {
            if rows.len() > 512 {
                return Err(Error::State);
            }
            snapshot.tables.insert(
                key(name),
                Table {
                    rows: rows
                        .into_iter()
                        .map(|(id, value)| {
                            (
                                id,
                                Row::from([(key("Document"), Value::String(value.to_string()))]),
                            )
                        })
                        .collect(),
                },
            );
        }
        // A single strict reader defines v1, including ranges and string limits.
        Self::load(&snapshot)?;
        Ok(snapshot
            .tables
            .into_iter()
            .map(|(id, table)| Op::SetTable(id, table))
            .collect())
    }
}

/// Explicit local initialization: no records/social history/entitlement grants.
/// Existing state is never replaced, and partial state is never repaired with
/// zeros. Run only when this account's owned awards service is selected.
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
        let batch = Batch {
            id: INITIAL_BATCH,
            expected_generation: snapshot.generation,
            ops: State::default().operations()?,
        };
        match repository.apply(account, &batch, now) {
            Ok(_) | Err(nfs_storage::Error::Conflict) => {}
            Err(_) => return Err(Error::State),
        }
        snapshot = repository.snapshot(account).map_err(|_| Error::State)?;
    }
    State::load(&snapshot)?;
    Ok(snapshot)
}
