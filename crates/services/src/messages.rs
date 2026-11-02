// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Durable phone messages: the rows the message managers restore at session
//! start and append as messages arrive. The normal manager's table is
//! `StaticMessage {Index, Asset, Read}`: the row key is the message number,
//! `Asset` the message's HashId (official E1026 906.624 s appends
//! `intro_01_obj05_pho_reminder01_01`, HashId 1422063331). The underground
//! manager's `UgStaticMessages {Index, MessageHash, Read}` holds the same
//! rows under another hash column (installed persistent content; E1026
//! 1247.925 s announces `preorderclaimparts` there, E912 32.336 s restores it).
use crate::persistent::key;
use nfs_storage::{Batch, Error, Op, Snapshot, tables::Value};

pub mod dynamic;
pub mod unlocks;

pub const STATIC: &str = "StaticMessage";
pub const UNDERGROUND: &str = "UgStaticMessages";
const NAMESPACE: u64 = 1 << 53;
const MAX_ROWS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Row {
    pub index: u64,
    pub hash: u32,
    pub read: bool,
}

/// The column holding a table's message HashId.
fn hash_column(table: &str) -> &'static str {
    if table == UNDERGROUND {
        "MessageHash"
    } else {
        "Asset"
    }
}

/// One table's rows in message-number order (an unwritten table is empty).
pub fn rows(
    tables: &std::collections::BTreeMap<u32, nfs_storage::tables::Table>,
    name: &str,
) -> Result<Vec<Row>, Error> {
    let Some(table) = tables.get(&key(name)) else {
        return Ok(vec![]);
    };
    if table.rows.len() > MAX_ROWS {
        return Err(Error::Bounds);
    }
    table
        .rows
        .iter()
        .map(|(&index, row)| {
            let int = |column: &str| match row.get(&key(column)) {
                Some(Value::Int(v)) => Ok(*v),
                _ => Err(Error::Invalid),
            };
            Ok(Row {
                index,
                hash: int(hash_column(name))? as u32,
                read: match int("Read")? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Invalid),
                },
            })
        })
        .collect()
}

/// Append a unique message: `None` when the table already holds it (a
/// unique sender sends once per account; a replayed request is a no-op).
/// The next number is the largest existing one plus one.
pub fn append(snapshot: &Snapshot, table: &str, hash: u32) -> Result<Option<Batch>, Error> {
    let existing = rows(&snapshot.tables, table)?;
    if existing.iter().any(|r| r.hash == hash) {
        return Ok(None);
    }
    if existing.len() >= MAX_ROWS {
        return Err(Error::Bounds);
    }
    let index = existing.last().map_or(0, |r| r.index + 1);
    let mut next = snapshot
        .tables
        .get(&key(table))
        .cloned()
        .unwrap_or_default();
    next.rows.insert(
        index,
        [
            (key(hash_column(table)), Value::Int(hash as i32)),
            (key("Read"), Value::Int(0)),
        ]
        .into(),
    );
    batch(snapshot, table, next).map(Some)
}

/// A restrictor group permits one durable message across competing requests.
/// The caller applies this generation-checked batch and reevaluates after any
/// conflict. An existing member wins even if a later request selects another.
pub fn append_exclusive(
    snapshot: &Snapshot,
    table: &str,
    hash: u32,
    group: &[u32],
) -> Result<Option<Batch>, Error> {
    if group.is_empty()
        || group.len() > 16
        || !group.contains(&hash)
        || group
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != group.len()
    {
        return Err(Error::Invalid);
    }
    let held = rows(&snapshot.tables, table)?
        .iter()
        .filter(|r| group.contains(&r.hash))
        .count();
    match held {
        0 => append(snapshot, table, hash),
        1 => Ok(None),
        _ => Err(Error::Invalid),
    }
}

/// Mark the most recent row of a message read: `None` when there is no such
/// row or it is already read.
pub fn mark_read(snapshot: &Snapshot, table: &str, hash: u32) -> Result<Option<Batch>, Error> {
    set_read(snapshot, table, hash, true)
}

/// Apply the reported read flag, including an unread report. Retries are inert;
/// an unknown message cannot create a row or unlock a message-dependent gate.
pub fn set_read(
    snapshot: &Snapshot,
    table: &str,
    hash: u32,
    read: bool,
) -> Result<Option<Batch>, Error> {
    let Some(row) = rows(&snapshot.tables, table)?
        .into_iter()
        .rev()
        .find(|r| r.hash == hash)
    else {
        return Ok(None);
    };
    if row.read == read {
        return Ok(None);
    }
    let mut next = snapshot.tables[&key(table)].clone();
    next.rows
        .get_mut(&row.index)
        .ok_or(Error::Invalid)?
        .insert(key("Read"), Value::Int(i32::from(read)));
    batch(snapshot, table, next).map(Some)
}

fn batch(
    snapshot: &Snapshot,
    table: &str,
    next: nfs_storage::tables::Table,
) -> Result<Batch, Error> {
    if snapshot.generation >= NAMESPACE {
        return Err(Error::Conflict);
    }
    let batch = Batch {
        id: NAMESPACE | snapshot.generation,
        expected_generation: snapshot.generation,
        ops: vec![Op::SetTable(key(table), next)],
    };
    batch.validate()?;
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_storage::tables::Table;
    #[test]
    fn exclusive_messages_recheck_after_competing_generation_and_do_not_cross_managers() {
        use nfs_storage::{AccountId, InventoryRepository, MemoryRepository, Timestamp};
        let repo = MemoryRepository::default();
        let account = AccountId::from_owned_config([53; 16]).unwrap();
        repo.open(account).unwrap();
        let initial = repo.snapshot(account).unwrap();
        let group = [101, 102, 103];
        let first = append_exclusive(&initial, STATIC, 102, &group)
            .unwrap()
            .unwrap();
        let competing = append_exclusive(&initial, STATIC, 103, &group)
            .unwrap()
            .unwrap();
        repo.apply(account, &first, Timestamp(1)).unwrap();
        // Same-generation retry keys are allowed to resolve to the first
        // receipt; the worker must reload and reevaluate the semantic group.
        let _ = repo.apply(account, &competing, Timestamp(2));
        let saved = repo.snapshot(account).unwrap();
        assert!(
            append_exclusive(&saved, STATIC, 103, &group)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            rows(&saved.tables, STATIC)
                .unwrap()
                .iter()
                .map(|r| r.hash)
                .collect::<Vec<_>>(),
            [102]
        );
        assert!(
            append_exclusive(&saved, UNDERGROUND, 103, &group)
                .unwrap()
                .is_some()
        );
        for bad in [vec![], vec![102, 102], vec![101, 103]] {
            assert!(append_exclusive(&saved, STATIC, 102, &bad).is_err());
        }
        let corrupt = snapshot(&[(0, 101, 0), (1, 103, 1)]);
        assert!(append_exclusive(&corrupt, STATIC, 102, &group).is_err());
        let other = AccountId::from_owned_config([54; 16]).unwrap();
        repo.open(other).unwrap();
        assert!(
            append_exclusive(&repo.snapshot(other).unwrap(), STATIC, 103, &group)
                .unwrap()
                .is_some()
        );
    }
    fn snapshot(rows: &[(u64, u32, i32)]) -> Snapshot {
        let mut table = Table::default();
        for &(index, hash, read) in rows {
            table.rows.insert(
                index,
                [
                    (key("Asset"), Value::Int(hash as i32)),
                    (key("Read"), Value::Int(read)),
                ]
                .into(),
            );
        }
        Snapshot {
            generation: 7,
            updated_at: Default::default(),
            items: Default::default(),
            garage: None,
            tables: [(key(STATIC), table)].into(),
        }
    }
    fn table(batch: &Batch) -> &Table {
        let [Op::SetTable(k, t)] = batch.ops.as_slice() else {
            panic!()
        };
        assert_eq!(*k, key(STATIC));
        t
    }
    #[test]
    fn append_numbers_after_the_largest_row_and_sends_a_unique_message_once() {
        let empty = snapshot(&[]);
        let batch = append(&empty, STATIC, 1_422_063_331).unwrap().unwrap();
        assert_eq!(batch.expected_generation, 7);
        assert_eq!(batch.id, NAMESPACE | 7);
        assert_eq!(
            table(&batch).rows[&0][&key("Asset")],
            Value::Int(1_422_063_331)
        );
        let held = snapshot(&[(3, 9, 1), (5, 4_000_000_000, 0)]);
        assert_eq!(rows(&held.tables, STATIC).unwrap()[1].hash, 4_000_000_000);
        let batch = append(&held, STATIC, 11).unwrap().unwrap();
        assert!(table(&batch).rows.contains_key(&6));
        assert!(append(&held, STATIC, 9).unwrap().is_none());
    }
    #[test]
    fn read_marks_the_latest_row_once_and_ignores_unknown_messages() {
        let held = snapshot(&[(0, 9, 0), (1, 9, 0), (2, 5, 1)]);
        let batch = mark_read(&held, STATIC, 9).unwrap().unwrap();
        assert_eq!(table(&batch).rows[&1][&key("Read")], Value::Int(1));
        assert_eq!(table(&batch).rows[&0][&key("Read")], Value::Int(0));
        assert!(mark_read(&held, STATIC, 5).unwrap().is_none());
        assert!(mark_read(&held, STATIC, 77).unwrap().is_none());
        let batch = set_read(&held, STATIC, 5, false).unwrap().unwrap();
        assert_eq!(table(&batch).rows[&2][&key("Read")], Value::Int(0));
        assert_eq!(table(&batch).rows[&2][&key("Asset")], Value::Int(5));
        assert!(set_read(&held, STATIC, 9, false).unwrap().is_none());
        assert!(set_read(&held, STATIC, 77, false).unwrap().is_none());
    }
    #[test]
    fn the_underground_table_keeps_its_hash_in_message_hash() {
        let empty = snapshot(&[]);
        let batch = append(&empty, UNDERGROUND, 985_423_157).unwrap().unwrap();
        let [Op::SetTable(k, t)] = batch.ops.as_slice() else {
            panic!()
        };
        assert_eq!(*k, key(UNDERGROUND));
        assert_eq!(t.rows[&0][&key("MessageHash")], Value::Int(985_423_157));
        assert!(!t.rows[&0].contains_key(&key("Asset")));
        let tables = [(key(UNDERGROUND), t.clone())].into();
        assert_eq!(
            rows(&tables, UNDERGROUND).unwrap(),
            [Row {
                index: 0,
                hash: 985_423_157,
                read: false
            }]
        );
        assert!(
            append(
                &Snapshot {
                    tables,
                    ..empty.clone()
                },
                UNDERGROUND,
                985_423_157
            )
            .unwrap()
            .is_none()
        );
        // The normal table's column does not stand in for it.
        let normal = snapshot(&[(0, 9, 0)]).tables[&key(STATIC)].clone();
        assert!(rows(&[(key(UNDERGROUND), normal)].into(), UNDERGROUND).is_err());
    }
    #[test]
    fn malformed_rows_are_refused_and_an_unwritten_table_is_empty() {
        let mut bad = snapshot(&[(0, 9, 2)]);
        assert!(rows(&bad.tables, STATIC).is_err());
        bad.tables.clear();
        let batch = append(&bad, STATIC, 1).unwrap().unwrap();
        assert!(table(&batch).rows.contains_key(&0));
    }
}
