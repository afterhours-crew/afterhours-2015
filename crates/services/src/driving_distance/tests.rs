// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{Applied, MemoryRepository, SqliteRepository};
use std::sync::atomic::{AtomicBool, Ordering};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn catalog() -> Catalog {
    let mut data = crate::persistent::tests::content();
    let table = data["tables"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == STATS)
        .unwrap();
    table["columns"][0]["name"] = COLUMN.into();
    table["columns"][0]["key"] = key(COLUMN).into();
    Catalog::from_json(&data).unwrap()
}
fn initialize(repo: &dyn InventoryRepository, who: AccountId, catalog: &Catalog) -> Snapshot {
    let snapshot = repo.open(who).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 3,
            expected_generation: snapshot.generation,
            ops: vec![Op::SetGarage(nfs_storage::GarageSlots::default())],
        },
        Timestamp(1),
    )
    .unwrap();
    let snapshot = repo.snapshot(who).unwrap();
    catalog
        .ensure_loaded(repo, who, snapshot, Timestamp(1))
        .unwrap()
        .0
}
fn begin(repo: &dyn InventoryRepository, who: AccountId, nonce: u8) -> Stream {
    let stream = Stream::new(who, repo.snapshot(who).unwrap().generation, [nonce; 16]).unwrap();
    open(repo, &stream, Timestamp(1)).unwrap();
    stream
}

#[test]
fn cumulative_checkpoints_are_atomic_retry_safe_and_preserve_other_accounts_and_columns() {
    let repo = MemoryRepository::default();
    let catalog = catalog();
    let initial = initialize(&repo, account(1), &catalog);
    let other = initialize(&repo, account(2), &catalog);
    let stream = begin(&repo, account(1), 1);
    let first = commit(&repo, &catalog, &stream, 17, Timestamp(2)).unwrap();
    assert_eq!(metres(&catalog, &first), Ok(17));
    for total in [17, 0, 16, 17] {
        assert_eq!(
            commit(&repo, &catalog, &stream, total, Timestamp(3)),
            Ok(first.clone())
        );
    }
    let final_state = commit(&repo, &catalog, &stream, 25, Timestamp(4)).unwrap();
    assert_eq!(metres(&catalog, &final_state), Ok(25));
    let mut independent = final_state.tables.clone();
    independent.remove(&key(TABLE));
    independent
        .get_mut(&key(STATS))
        .unwrap()
        .rows
        .get_mut(&0)
        .unwrap()
        .insert(key(COLUMN), Value::Int(0));
    assert_eq!(independent, initial.tables);
    assert_eq!(repo.snapshot(account(2)).unwrap(), other);
}

#[test]
fn competing_streams_rebase_without_losing_or_duplicating_distance() {
    let repo = MemoryRepository::default();
    let catalog = catalog();
    initialize(&repo, account(1), &catalog);
    let a = begin(&repo, account(1), 1);
    let b = begin(&repo, account(1), 2);
    let before = repo.snapshot(account(1)).unwrap();
    let first = prepare(&catalog, &before, &a, 19).unwrap().unwrap();
    let competing = prepare(&catalog, &before, &b, 7).unwrap().unwrap();
    repo.apply(account(1), &first, Timestamp(2)).unwrap();
    // Storage's generation-derived receipt can belong to the other operation.
    assert!(
        repo.apply(account(1), &competing, Timestamp(2))
            .unwrap()
            .replayed
    );
    let result = commit(&repo, &catalog, &b, 7, Timestamp(3)).unwrap();
    assert_eq!(metres(&catalog, &result), Ok(26));
    assert_eq!(watermark(&journal(&result).unwrap(), &a), Ok(Some(19)));
    assert_eq!(watermark(&journal(&result).unwrap(), &b), Ok(Some(7)));
    assert_eq!(
        commit(&repo, &catalog, &a, 19, Timestamp(4)),
        Ok(result.clone())
    );
    assert_eq!(open(&repo, &a, Timestamp(4)), Ok(result));
}

struct Failing {
    inner: MemoryRepository,
    fail: AtomicBool,
    after_commit: bool,
}
impl InventoryRepository for Failing {
    fn open(&self, a: AccountId) -> Result<Snapshot, Error> {
        self.inner.open(a)
    }
    fn snapshot(&self, a: AccountId) -> Result<Snapshot, Error> {
        self.inner.snapshot(a)
    }
    fn receipt(&self, a: AccountId, id: u64) -> Result<Option<Applied>, Error> {
        self.inner.receipt(a, id)
    }
    fn apply(&self, a: AccountId, batch: &Batch, now: Timestamp) -> Result<Applied, Error> {
        let fail = self.fail.swap(false, Ordering::SeqCst);
        if fail && !self.after_commit {
            return Err(Error::Storage);
        }
        let result = self.inner.apply(a, batch, now)?;
        if fail {
            Err(Error::Storage)
        } else {
            Ok(result)
        }
    }
}

#[test]
fn failed_and_lost_commit_results_retry_the_same_cumulative_value() {
    for after_commit in [false, true] {
        let repo = Failing {
            inner: MemoryRepository::default(),
            fail: AtomicBool::new(false),
            after_commit,
        };
        let catalog = catalog();
        initialize(&repo, account(1), &catalog);
        let stream = begin(&repo, account(1), 1);
        repo.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            commit(&repo, &catalog, &stream, 35, Timestamp(2)),
            Err(Error::Storage)
        );
        assert_eq!(
            metres(&catalog, &repo.snapshot(account(1)).unwrap()),
            Ok(if after_commit { 35 } else { 0 })
        );
        let result = commit(&repo, &catalog, &stream, 35, Timestamp(2)).unwrap();
        assert_eq!(metres(&catalog, &result), Ok(35));
        assert_eq!(watermark(&journal(&result).unwrap(), &stream), Ok(Some(35)));
    }
}

#[test]
fn host_claim_preserves_identity_after_an_uncertain_open_or_checkpoint() {
    for after_commit in [false, true] {
        let repo = Failing {
            inner: MemoryRepository::default(),
            fail: AtomicBool::new(false),
            after_commit,
        };
        let catalog = catalog();
        initialize(&repo, account(1), &catalog);
        let mut claim = Claim::new(account(1), [32; 16]).unwrap();
        repo.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            claim.checkpoint(&repo, &catalog, 12, Timestamp(3)),
            Err(Error::Storage)
        );
        let uncertain = claim.stream.clone().unwrap();
        let done = claim.checkpoint(&repo, &catalog, 12, Timestamp(4)).unwrap();
        assert_eq!(claim.stream, Some(uncertain));
        assert_eq!(metres(&catalog, &done), Ok(12));
        repo.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            claim.checkpoint(&repo, &catalog, 31, Timestamp(5)),
            Err(Error::Storage)
        );
        let done = claim.checkpoint(&repo, &catalog, 31, Timestamp(6)).unwrap();
        assert_eq!(metres(&catalog, &done), Ok(31));
        assert_eq!(
            claim.checkpoint(&repo, &catalog, 12, Timestamp(7)),
            Ok(done)
        );
        // A previously used claim cannot turn a retired writer into a new one.
        for i in 0..MAX_STREAMS {
            begin(&repo, account(1), 40 + i as u8);
        }
        let before = repo.snapshot(account(1)).unwrap();
        assert_eq!(
            claim.checkpoint(&repo, &catalog, 32, Timestamp(8)),
            Err(Error::Conflict)
        );
        assert_eq!(repo.snapshot(account(1)).unwrap(), before);
    }
}

struct Racing {
    inner: MemoryRepository,
    race: AtomicBool,
    same_receipt: bool,
}
impl InventoryRepository for Racing {
    fn open(&self, a: AccountId) -> Result<Snapshot, Error> {
        self.inner.open(a)
    }
    fn snapshot(&self, a: AccountId) -> Result<Snapshot, Error> {
        self.inner.snapshot(a)
    }
    fn receipt(&self, a: AccountId, id: u64) -> Result<Option<Applied>, Error> {
        self.inner.receipt(a, id)
    }
    fn apply(&self, a: AccountId, batch: &Batch, now: Timestamp) -> Result<Applied, Error> {
        if self.race.swap(false, Ordering::SeqCst) {
            self.inner.apply(
                a,
                &Batch {
                    id: if self.same_receipt { batch.id } else { 6789 },
                    expected_generation: batch.expected_generation,
                    ops: vec![Op::SetTable(key("Independent"), Table::default())],
                },
                now,
            )?;
        }
        self.inner.apply(a, batch, now)
    }
}

#[test]
fn fresh_claim_retries_definite_cas_loss_and_competing_receipts() {
    for same_receipt in [false, true] {
        let repo = Racing {
            inner: MemoryRepository::default(),
            race: AtomicBool::new(false),
            same_receipt,
        };
        let catalog = catalog();
        initialize(&repo, account(1), &catalog);
        let mut claim = Claim::new(account(1), [11; 16]).unwrap();
        repo.race.store(true, Ordering::SeqCst);
        let saved = claim.checkpoint(&repo, &catalog, 17, Timestamp(2)).unwrap();
        assert_eq!(metres(&catalog, &saved), Ok(17));
        assert_eq!(journal(&saved).unwrap().rows.len(), 2);
        assert!(saved.tables.contains_key(&key("Independent")));
        assert_eq!(
            claim.checkpoint(&repo, &catalog, 17, Timestamp(3)),
            Ok(saved)
        );
    }
}

#[test]
fn semantic_watermark_outlives_generic_batch_receipt_eviction() {
    let repo = MemoryRepository::default();
    let catalog = catalog();
    initialize(&repo, account(1), &catalog);
    let stream = begin(&repo, account(1), 1);
    let initial = repo.snapshot(account(1)).unwrap();
    let batch = prepare(&catalog, &initial, &stream, 11).unwrap().unwrap();
    repo.apply(account(1), &batch, Timestamp(2)).unwrap();
    for i in 0..nfs_storage::MAX_BATCH_HISTORY {
        let current = repo.snapshot(account(1)).unwrap();
        repo.apply(
            account(1),
            &Batch {
                id: 10000 + i as u64,
                expected_generation: current.generation,
                ops: vec![Op::SetTable(key("Independent"), Table::default())],
            },
            Timestamp(3),
        )
        .unwrap();
    }
    assert_eq!(repo.receipt(account(1), batch.id), Ok(None));
    let saved = commit(&repo, &catalog, &stream, 11, Timestamp(4)).unwrap();
    assert_eq!(metres(&catalog, &saved), Ok(11));
    let next = commit(&repo, &catalog, &stream, 14, Timestamp(5)).unwrap();
    assert_eq!(metres(&catalog, &next), Ok(14));
}

#[test]
fn retired_streams_cannot_reopen_after_bounded_eviction_or_career_replacement() {
    let repo = MemoryRepository::default();
    let catalog = catalog();
    initialize(&repo, account(1), &catalog);
    let old = begin(&repo, account(1), 1);
    commit(&repo, &catalog, &old, 8, Timestamp(2)).unwrap();
    for i in 0..MAX_STREAMS {
        begin(&repo, account(1), 2 + i as u8);
    }
    let before = repo.snapshot(account(1)).unwrap();
    assert_eq!(journal(&before).unwrap().rows.len(), MAX_STREAMS + 1);
    assert_eq!(open(&repo, &old, Timestamp(3)), Err(Error::Conflict));
    assert_eq!(
        commit(&repo, &catalog, &old, 9, Timestamp(3)),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), before);
    // Whole-career replacement has an advanced generation and no old journal.
    let mut replaced = before.clone();
    replaced.generation += 1;
    replaced.tables.remove(&key(TABLE));
    assert_eq!(prepare_open(&replaced, &old), Err(Error::Conflict));
    assert_eq!(prepare(&catalog, &replaced, &old, 9), Err(Error::Conflict));
}

#[test]
fn conflicting_open_identity_and_invalid_values_fail_without_partial_mutation() {
    let repo = MemoryRepository::default();
    let catalog = catalog();
    let initial = initialize(&repo, account(1), &catalog);
    let a = Stream::new(account(1), initial.generation, [1; 16]).unwrap();
    let b = Stream::new(account(1), initial.generation, [2; 16]).unwrap();
    open(&repo, &a, Timestamp(1)).unwrap();
    let before = repo.snapshot(account(1)).unwrap();
    assert_eq!(open(&repo, &b, Timestamp(1)), Err(Error::Conflict));
    assert_eq!(
        commit(&repo, &catalog, &b, 10, Timestamp(2)),
        Err(Error::Conflict)
    );
    assert_eq!(
        commit(&repo, &catalog, &a, -1, Timestamp(2)),
        Err(Error::Invalid)
    );
    let mut overflow = before.clone();
    overflow
        .tables
        .get_mut(&key(STATS))
        .unwrap()
        .rows
        .get_mut(&0)
        .unwrap()
        .insert(key(COLUMN), Value::Int(i32::MAX));
    assert_eq!(prepare(&catalog, &overflow, &a, 1), Err(Error::Bounds));
    let mut malformed = before.clone();
    malformed
        .tables
        .get_mut(&key(TABLE))
        .unwrap()
        .rows
        .get_mut(&a.row())
        .unwrap()
        .insert(METRES, Value::Int(-1));
    assert_eq!(prepare(&catalog, &malformed, &a, 1), Err(Error::Config));
    assert_eq!(repo.snapshot(account(1)).unwrap(), before);
}

#[test]
fn saved_watermark_survives_real_repository_reinitialization() {
    let path = std::env::temp_dir().join(format!(
        "afterhours-distance-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let catalog = catalog();
    let stream;
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        initialize(&repo, account(1), &catalog);
        stream = begin(&repo, account(1), 1);
        commit(&repo, &catalog, &stream, 81, Timestamp(2)).unwrap();
    }
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let saved = commit(&repo, &catalog, &stream, 81, Timestamp(3)).unwrap();
        assert_eq!(metres(&catalog, &saved), Ok(81));
        let next = begin(&repo, account(1), 2);
        let saved = commit(&repo, &catalog, &next, 4, Timestamp(4)).unwrap();
        assert_eq!(metres(&catalog, &saved), Ok(85));
    }
    std::fs::remove_dir_all(path).unwrap();
}
