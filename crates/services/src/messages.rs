// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Durable phone messages: the rows the message managers restore at session
//! start and append as messages arrive. The normal manager's table is
//! `StaticMessage {Index, Asset, Read}`: the row key is the message number,
//! `Asset` the message's HashId (official E1026 906.624 s appends
//! `intro_01_obj05_pho_reminder01_01`, HashId 1422063331).
use crate::persistent::key;
use nfs_storage::{Batch, Error, Op, Snapshot, tables::Value};

pub const STATIC: &str = "StaticMessage";
const NAMESPACE: u64 = 1 << 53;
const MAX_ROWS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Row {
    pub index: u64,
    pub hash: u32,
    pub read: bool,
}

/// One table's rows in message-number order (an unwritten table is empty).
pub fn rows(
    tables: &std::collections::BTreeMap<u32, nfs_storage::tables::Table>,
    table: &str,
) -> Result<Vec<Row>, Error> {
    let Some(table) = tables.get(&key(table)) else {
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
                hash: int("Asset")? as u32,
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
            (key("Asset"), Value::Int(hash as i32)),
            (key("Read"), Value::Int(0)),
        ]
        .into(),
    );
    batch(snapshot, table, next).map(Some)
}

/// Mark the most recent row of a message read: `None` when there is no such
/// row or it is already read.
pub fn mark_read(snapshot: &Snapshot, table: &str, hash: u32) -> Result<Option<Batch>, Error> {
    let Some(row) = rows(&snapshot.tables, table)?
        .into_iter()
        .rev()
        .find(|r| r.hash == hash)
    else {
        return Ok(None);
    };
    if row.read {
        return Ok(None);
    }
    let mut next = snapshot.tables[&key(table)].clone();
    next.rows
        .get_mut(&row.index)
        .ok_or(Error::Invalid)?
        .insert(key("Read"), Value::Int(1));
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
