// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned map POIs already seen by the player. This controls the map's
//! new-marker decoration, not whether progression makes an event available.
use crate::persistent::key;
use nfs_storage::{
    Batch, Error, Op, Snapshot,
    tables::{MAX_ROWS, Table, Value},
};
use std::collections::BTreeSet;

pub const TABLE: &str = "PoiIdTable";
const COLUMN: &str = "PoiId";
const NAMESPACE: u64 = 1 << 52;
pub const MAX_IDS: usize = MAX_ROWS;

/// The single int cell preserves all 32 ID bits, including the sign bit.
pub fn seen(table: Option<&Table>) -> Result<BTreeSet<u32>, Error> {
    let Some(table) = table else {
        return Ok(BTreeSet::new());
    };
    if table.rows.len() > MAX_IDS {
        return Err(Error::Bounds);
    }
    table
        .rows
        .iter()
        .map(|(&row, cells)| {
            let id = u32::try_from(row).map_err(|_| Error::Invalid)?;
            match (cells.len(), cells.get(&key(COLUMN))) {
                (1, Some(Value::Int(value))) if *value as u32 == id => Ok(id),
                _ => Err(Error::Invalid),
            }
        })
        .collect()
}

/// Prepare one idempotent write. The worker must reload after apply, including
/// a replayed generation receipt, and acknowledge only the committed seen set.
pub fn mark(snapshot: &Snapshot, id: u32) -> Result<Option<Batch>, Error> {
    let table = snapshot.tables.get(&key(TABLE));
    let old = seen(table)?;
    if old.contains(&id) {
        return Ok(None);
    }
    if old.len() >= MAX_IDS {
        return Err(Error::Bounds);
    }
    if snapshot.generation >= NAMESPACE {
        return Err(Error::Conflict);
    }
    let mut next = table.cloned().unwrap_or_default();
    next.rows
        .insert(u64::from(id), [(key(COLUMN), Value::Int(id as i32))].into());
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
    use nfs_storage::{AccountId, InventoryRepository, MemoryRepository, Timestamp};

    fn stored(snapshot: &Snapshot) -> BTreeSet<u32> {
        seen(snapshot.tables.get(&key(TABLE))).unwrap()
    }

    #[test]
    fn actual_commits_preserve_unsigned_ids_retries_and_account_isolation() {
        let repo = MemoryRepository::new();
        let owner = AccountId::from_owned_config([3; 16]).unwrap();
        let other = AccountId::from_owned_config([4; 16]).unwrap();
        let initial = repo.open(owner).unwrap();
        let untouched = repo.open(other).unwrap();
        let first = mark(&initial, u32::MAX).unwrap().unwrap();
        assert!(stored(&repo.snapshot(owner).unwrap()).is_empty());
        repo.apply(owner, &first, Timestamp(1)).unwrap();
        assert!(repo.apply(owner, &first, Timestamp(2)).unwrap().replayed);
        let committed = repo.snapshot(owner).unwrap();
        assert_eq!(stored(&committed), BTreeSet::from([u32::MAX]));
        assert!(mark(&committed, u32::MAX).unwrap().is_none());
        let second = mark(&committed, 0).unwrap().unwrap();
        repo.apply(owner, &second, Timestamp(3)).unwrap();
        assert_eq!(
            stored(&repo.snapshot(owner).unwrap()),
            BTreeSet::from([0, u32::MAX])
        );
        assert_eq!(repo.snapshot(other).unwrap(), untouched);
    }

    #[test]
    fn a_stale_generation_receipt_does_not_prove_a_different_id_committed() {
        let repo = MemoryRepository::new();
        let owner = AccountId::from_owned_config([5; 16]).unwrap();
        let initial = repo.open(owner).unwrap();
        let first = mark(&initial, 31).unwrap().unwrap();
        let competing = mark(&initial, 32).unwrap().unwrap();
        repo.apply(owner, &first, Timestamp(1)).unwrap();
        assert!(
            repo.apply(owner, &competing, Timestamp(2))
                .unwrap()
                .replayed
        );
        let current = repo.snapshot(owner).unwrap();
        assert_eq!(stored(&current), BTreeSet::from([31]));
        let retry = mark(&current, 32).unwrap().unwrap();
        repo.apply(owner, &retry, Timestamp(3)).unwrap();
        assert_eq!(
            stored(&repo.snapshot(owner).unwrap()),
            BTreeSet::from([31, 32])
        );
    }

    #[test]
    fn malformed_full_and_exhausted_state_cannot_prepare_a_partial_write() {
        let table = |row, cells| Table {
            rows: [(row, cells)].into(),
        };
        for bad in [
            table(
                u64::from(u32::MAX) + 1,
                [(key(COLUMN), Value::Int(0))].into(),
            ),
            table(8, [(key(COLUMN), Value::Int(9))].into()),
            table(8, [(key(COLUMN), Value::String("8".into()))].into()),
            table(
                8,
                [(key(COLUMN), Value::Int(8)), (91, Value::Int(1))].into(),
            ),
        ] {
            assert!(seen(Some(&bad)).is_err());
        }
        let full = Table {
            rows: (0..MAX_IDS as u64)
                .map(|id| (id, [(key(COLUMN), Value::Int(id as i32))].into()))
                .collect(),
        };
        let snapshot = Snapshot {
            tables: [(key(TABLE), full)].into(),
            ..Default::default()
        };
        assert!(mark(&snapshot, 0).unwrap().is_none());
        assert_eq!(mark(&snapshot, MAX_IDS as u32), Err(Error::Bounds));
        let exhausted = Snapshot {
            generation: NAMESPACE,
            ..Default::default()
        };
        assert_eq!(mark(&exhausted, 1), Err(Error::Conflict));
    }
}
