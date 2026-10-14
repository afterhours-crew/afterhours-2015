// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local account cash, separate from earned-cash statistics and item ownership.
//! The versioned local table is never sent as a game persistent table. Callers
//! include its update in the same repository batch as the purchased items.
use nfs_storage::{
    Error, Op, Snapshot,
    tables::{Table, Value},
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Balance(pub u32);

impl Balance {
    /// Existing accounts without a bank record have zero cash. A malformed
    /// record fails closed instead of resetting saved money.
    pub fn from_snapshot(snapshot: &Snapshot) -> Result<Self, Error> {
        let Some(table) = snapshot
            .tables
            .get(&crate::persistent::key("AfterhoursBank"))
        else {
            return Ok(Self(0));
        };
        let row = table.rows.get(&0).ok_or(Error::Config)?;
        if table.rows.len() != 1 || row.len() != 2 || row.get(&0) != Some(&Value::Int(1)) {
            return Err(Error::Config);
        }
        let Some(Value::String(text)) = row.get(&1) else {
            return Err(Error::Config);
        };
        let amount = text.parse::<u32>().map_err(|_| Error::Config)?;
        if amount.to_string() != *text {
            return Err(Error::Config);
        }
        Ok(Self(amount))
    }

    /// None means insufficient funds; no state is changed.
    pub fn debit(self, price: u32) -> Option<Self> {
        self.0.checked_sub(price).map(Self)
    }

    /// Produces an operation only. It cannot commit a balance independently
    /// of its caller's inventory transaction or choose an account from input.
    pub fn store(self) -> Op {
        Op::SetTable(
            crate::persistent::key("AfterhoursBank"),
            Table {
                rows: BTreeMap::from([(
                    0,
                    BTreeMap::from([(0, Value::Int(1)), (1, Value::String(self.0.to_string()))]),
                )]),
            },
        )
    }
}

/// Explicit local operator adjustment, never a gameplay reward. The caller
/// must stop the account's server first and supply the inspected generation.
/// A stale generation fails instead of silently rebasing an absolute amount.
pub fn set_for_operator(
    repository: &dyn nfs_storage::InventoryRepository,
    account: nfs_storage::AccountId,
    expected: u64,
    amount: Balance,
    now: nfs_storage::Timestamp,
) -> Result<Snapshot, Error> {
    const NAMESPACE: u64 = 1 << 60;
    let snapshot = repository.snapshot(account)?;
    Balance::from_snapshot(&snapshot)?;
    if snapshot.generation != expected || expected >= NAMESPACE {
        return Err(Error::Conflict);
    }
    let applied = repository.apply(
        account,
        &nfs_storage::Batch {
            id: NAMESPACE | expected,
            expected_generation: expected,
            ops: vec![amount.store()],
        },
        now,
    )?;
    if applied.replayed {
        let current = repository.snapshot(account)?;
        if Balance::from_snapshot(&current)? != amount {
            return Err(Error::Conflict);
        }
        return Ok(current);
    }
    let mut result = snapshot;
    let Op::SetTable(key, table) = amount.store() else {
        unreachable!()
    };
    result.tables.insert(key, table);
    result.generation = applied.generation;
    result.updated_at = now;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_storage::{AccountId, Batch, InventoryRepository, MemoryRepository, Timestamp};

    #[test]
    fn explicit_operator_set_requires_current_generation_and_keeps_other_accounts() {
        let repo = MemoryRepository::default();
        let account = AccountId::from_owned_config([1; 16]).unwrap();
        let other = AccountId::from_owned_config([2; 16]).unwrap();
        repo.open(account).unwrap();
        let other_before = repo.open(other).unwrap();
        let saved = set_for_operator(&repo, account, 0, Balance(100), Timestamp(1)).unwrap();
        assert_eq!(Balance::from_snapshot(&saved), Ok(Balance(100)));
        assert_eq!(
            set_for_operator(&repo, account, 0, Balance(200), Timestamp(2)),
            Err(Error::Conflict)
        );
        assert_eq!(repo.snapshot(account).unwrap(), saved);
        assert_eq!(repo.snapshot(other).unwrap(), other_before);
        let removed = set_for_operator(&repo, account, 1, Balance(0), Timestamp(3)).unwrap();
        assert_eq!(Balance::from_snapshot(&removed), Ok(Balance(0)));
    }

    #[test]
    fn balance_is_account_owned_atomic_and_guarded_by_generation() {
        let repository = MemoryRepository::default();
        let account = AccountId::from_owned_config([1; 16]).unwrap();
        let other = AccountId::from_owned_config([2; 16]).unwrap();
        let initial = repository.open(account).unwrap();
        repository.open(other).unwrap();
        assert_eq!(Balance::from_snapshot(&initial), Ok(Balance(0)));
        assert_eq!(Balance(369).debit(370), None);
        assert_eq!(Balance(370).debit(370), Some(Balance(0)));
        let batch = Batch {
            id: 100,
            expected_generation: initial.generation,
            ops: vec![Balance(u32::MAX).store()],
        };
        repository.apply(account, &batch, Timestamp(1)).unwrap();
        assert!(
            repository
                .apply(account, &batch, Timestamp(2))
                .unwrap()
                .replayed
        );
        assert_eq!(
            Balance::from_snapshot(&repository.snapshot(account).unwrap()),
            Ok(Balance(u32::MAX))
        );
        assert_eq!(
            Balance::from_snapshot(&repository.snapshot(other).unwrap()),
            Ok(Balance(0))
        );
        let stale = Batch {
            id: 101,
            ops: vec![Balance(1).store()],
            ..batch
        };
        assert_eq!(
            repository.apply(account, &stale, Timestamp(3)),
            Err(Error::Conflict)
        );
        assert_eq!(
            Balance::from_snapshot(&repository.snapshot(account).unwrap()),
            Ok(Balance(u32::MAX))
        );
    }

    #[test]
    fn malformed_bank_records_do_not_erase_money() {
        let account = AccountId::from_owned_config([1; 16]).unwrap();
        let initial = MemoryRepository::default().open(account).unwrap();
        for text in ["", "-1", "+1", "01", "4294967296", "1.0"] {
            let mut snapshot = initial.clone();
            let Op::SetTable(key, mut table) = Balance(370).store() else {
                unreachable!()
            };
            table
                .rows
                .get_mut(&0)
                .unwrap()
                .insert(1, Value::String(text.into()));
            snapshot.tables.insert(key, table);
            assert_eq!(Balance::from_snapshot(&snapshot), Err(Error::Config));
        }
        for version in [0, 2, -1] {
            let mut snapshot = initial.clone();
            let Op::SetTable(key, mut table) = Balance(370).store() else {
                unreachable!()
            };
            table
                .rows
                .get_mut(&0)
                .unwrap()
                .insert(0, Value::Int(version));
            snapshot.tables.insert(key, table);
            assert_eq!(Balance::from_snapshot(&snapshot), Err(Error::Config));
        }
    }
}
