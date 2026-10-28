// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Seen tutorials. The installed seen Bools persist through the game's
//! `TutorialTable` (`Tutorial_finished`, a dynamic secondary key), whose cell
//! layout is not traced, so this Afterhours-owned table keeps the seen set
//! instead and the game table stays empty. The client never receives it: only
//! schema tables join the Items load. Row key: the tutorial's stable key (the
//! name hash of its logic blueprint); one `Seen = 1` column.
use crate::persistent::key;
use nfs_storage::{
    Batch, Error, Op, Snapshot,
    tables::{Table, Value},
};
use std::collections::BTreeSet;

pub const TABLE: &str = "AfterhoursTutorials";
const COLUMN: &str = "Seen";
const NAMESPACE: u64 = 1 << 56;
const MAX_ROWS: usize = 64;

/// The seen keys (an unwritten table is empty).
pub fn seen(table: Option<&Table>) -> Result<BTreeSet<u32>, Error> {
    let Some(table) = table else {
        return Ok(BTreeSet::new());
    };
    if table.rows.len() > MAX_ROWS {
        return Err(Error::Bounds);
    }
    table
        .rows
        .iter()
        .map(|(&row, cells)| {
            let id = u32::try_from(row).map_err(|_| Error::Invalid)?;
            match (cells.len(), cells.get(&key(COLUMN))) {
                (1, Some(Value::Int(1))) => Ok(id),
                _ => Err(Error::Invalid),
            }
        })
        .collect()
}

/// Mark one tutorial seen: `None` when it already is (a replayed accept).
pub fn mark(snapshot: &Snapshot, tutorial: u32) -> Result<Option<Batch>, Error> {
    let table = snapshot.tables.get(&key(TABLE));
    let existing = seen(table)?;
    if existing.contains(&tutorial) {
        return Ok(None);
    }
    if existing.len() >= MAX_ROWS {
        return Err(Error::Bounds);
    }
    if snapshot.generation >= NAMESPACE {
        return Err(Error::Conflict);
    }
    let mut next = table.cloned().unwrap_or_default();
    next.rows
        .insert(u64::from(tutorial), [(key(COLUMN), Value::Int(1))].into());
    let batch = Batch {
        id: NAMESPACE | snapshot.generation,
        expected_generation: snapshot.generation,
        ops: vec![Op::SetTable(key(TABLE), next)],
    };
    batch.validate()?;
    Ok(Some(batch))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(rows: &[(u64, i32)]) -> Snapshot {
        let mut table = Table::default();
        for &(row, value) in rows {
            table
                .rows
                .insert(row, [(key(COLUMN), Value::Int(value))].into());
        }
        Snapshot {
            generation: 9,
            updated_at: Default::default(),
            items: Default::default(),
            garage: None,
            tables: [(key(TABLE), table)].into(),
        }
    }

    #[test]
    fn marking_is_idempotent_and_keyed_by_the_tutorial() {
        let empty = Snapshot {
            tables: Default::default(),
            ..snapshot(&[])
        };
        assert!(seen(empty.tables.get(&key(TABLE))).unwrap().is_empty());
        let batch = mark(&empty, 77).unwrap().unwrap();
        assert_eq!(batch.id, NAMESPACE | 9);
        assert_eq!(batch.expected_generation, 9);
        let [Op::SetTable(table, next)] = batch.ops.as_slice() else {
            panic!("one table write");
        };
        assert_eq!(*table, key(TABLE));
        assert_eq!(seen(Some(next)).unwrap(), BTreeSet::from([77]));
        let marked = snapshot(&[(77, 1)]);
        assert!(mark(&marked, 77).unwrap().is_none());
        let two = mark(&marked, 5).unwrap().unwrap();
        let [Op::SetTable(_, next)] = two.ops.as_slice() else {
            panic!("one table write");
        };
        assert_eq!(seen(Some(next)).unwrap(), BTreeSet::from([5, 77]));
    }

    #[test]
    fn malformed_or_oversized_rows_are_rejected() {
        for rows in [
            vec![(1, 0)],
            vec![(1, 2)],
            vec![(u64::from(u32::MAX) + 1, 1)],
        ] {
            assert!(seen(snapshot(&rows).tables.get(&key(TABLE))).is_err());
        }
        let full: Vec<_> = (0..MAX_ROWS as u64).map(|r| (r, 1)).collect();
        assert!(mark(&snapshot(&full), 999).is_err());
        let late = Snapshot {
            generation: NAMESPACE,
            ..snapshot(&[])
        };
        assert!(mark(&late, 1).is_err());
    }
}
