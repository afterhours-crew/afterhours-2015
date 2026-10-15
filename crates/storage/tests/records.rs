// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_storage::{
    AccountId, Error, InventoryRepository, SqliteRepository,
    records::{
        MAX_ACCOUNT_BYTES, MAX_RECORD_BYTES, MAX_RECORDS, Repository, SqliteRecords, UnixMicros,
    },
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nfs-livery-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn setup() -> (Temp, SqliteRecords) {
    let temp = Temp::new();
    let repo = SqliteRepository::open_owned_directory(&temp.0).unwrap();
    repo.open(account(1)).unwrap();
    repo.open(account(2)).unwrap();
    (temp, SqliteRecords(repo))
}

#[test]
fn exact_bytes_replace_retry_reopen_and_account_isolation() {
    let (temp, records) = setup();
    let a = account(1);
    let before = records.0.snapshot(a).unwrap();
    assert_eq!(records.get(a, "Header_0X01").unwrap(), None);
    let one = records
        .put(
            a,
            "Header_0X01",
            "image/jpeg",
            b"\0opaque\xff",
            UnixMicros(10),
        )
        .unwrap();
    assert_eq!(
        records
            .put(
                a,
                "Header_0X01",
                "image/jpeg",
                b"\0opaque\xff",
                UnixMicros(20)
            )
            .unwrap(),
        one
    );
    assert_eq!(records.get(account(2), "Header_0X01").unwrap(), None);
    let updated = records
        .put(
            a,
            "Header_0X01",
            "application/octet-stream",
            b"next",
            UnixMicros(30),
        )
        .unwrap();
    assert_eq!(
        (updated.created, updated.updated),
        (UnixMicros(10), UnixMicros(30))
    );
    let reopened = SqliteRecords(SqliteRepository::open_owned_directory(&temp.0).unwrap());
    assert_eq!(reopened.get(a, "Header_0X01").unwrap(), Some(updated));
    let regressed = reopened
        .put(a, "Header_0X01", "image/jpeg", b"different", UnixMicros(1))
        .unwrap();
    assert_eq!(
        (regressed.created, regressed.updated),
        (UnixMicros(10), UnixMicros(30))
    );
    assert_eq!(records.0.snapshot(a).unwrap(), before);
    reopened.delete(a, "Header_0X01").unwrap();
    reopened.delete(a, "Header_0X01").unwrap();
    assert_eq!(reopened.get(a, "Header_0X01").unwrap(), None);
}

#[test]
fn malformed_records_and_count_and_byte_quotas_leave_prior_records_intact() {
    let (_temp, r) = setup();
    let a = account(1);
    for name in ["", "../other", "a/b", "a%2Fb", "a\0b", &"a".repeat(129)] {
        assert_eq!(
            r.put(a, name, "image/jpeg", b"x", UnixMicros(1)),
            Err(Error::Invalid)
        );
        assert_eq!(r.get(a, name), Err(Error::Invalid));
        assert_eq!(r.delete(a, name), Err(Error::Invalid));
    }
    for mime in [
        "",
        "image/jpeg\r\nHeader:x",
        "image/jpeg;bad",
        "plain",
        &"a".repeat(129),
    ] {
        assert_eq!(
            r.put(a, "A", mime, b"x", UnixMicros(1)),
            Err(Error::Invalid)
        );
    }
    assert_eq!(
        r.put(a, "A", "image/jpeg", b"x", UnixMicros(0)),
        Err(Error::Bounds)
    );
    assert_eq!(
        r.put(
            a,
            "A",
            "image/jpeg",
            &vec![0; MAX_RECORD_BYTES + 1],
            UnixMicros(1)
        ),
        Err(Error::Bounds)
    );
    for i in 0..MAX_RECORDS {
        r.put(a, &format!("R{i}"), "image/jpeg", &[], UnixMicros(1))
            .unwrap();
    }
    assert_eq!(
        r.put(a, "overflow", "image/jpeg", b"x", UnixMicros(1)),
        Err(Error::Bounds)
    );
    r.put(a, "R0", "image/jpeg", b"existing", UnixMicros(2))
        .unwrap();
    assert_eq!(r.get(a, "R0").unwrap().unwrap().bytes, b"existing");
    r.delete(a, "R1").unwrap();
    r.put(a, "overflow", "image/jpeg", b"x", UnixMicros(3))
        .unwrap();
    let payload = vec![7; MAX_RECORD_BYTES];
    let b = account(2);
    for i in 0..MAX_ACCOUNT_BYTES / MAX_RECORD_BYTES {
        r.put(b, &format!("R{i}"), "image/jpeg", &payload, UnixMicros(1))
            .unwrap();
    }
    assert_eq!(
        r.put(b, "overflow", "image/jpeg", b"x", UnixMicros(1)),
        Err(Error::Bounds)
    );
    r.put(b, "R0", "image/jpeg", b"small", UnixMicros(2))
        .unwrap();
    r.put(b, "overflow", "image/jpeg", b"x", UnixMicros(3))
        .unwrap();
    assert_eq!(r.get(b, "R1").unwrap().unwrap().bytes, payload);
}

#[test]
fn migration_preserves_inventory_and_failed_storage_never_claims_success() {
    let (_temp, r) = setup();
    let a = account(1);
    let prior = r.0.snapshot(a).unwrap();
    let conn = rusqlite::Connection::open(r.0.path(a)).unwrap();
    conn.execute_batch("DROP TABLE livery_record; PRAGMA user_version=4;")
        .unwrap();
    assert_eq!(r.get(a, "A"), Err(Error::Version));
    assert_eq!(r.0.open(a).unwrap(), prior);
    conn.execute_batch("CREATE TRIGGER reject_livery BEFORE INSERT ON livery_record BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert_eq!(
        r.put(a, "A", "image/jpeg", b"x", UnixMicros(1)),
        Err(Error::Storage)
    );
    assert_eq!(r.get(a, "A").unwrap(), None);
    conn.execute_batch("DROP TRIGGER reject_livery;").unwrap();
    r.put(a, "A", "image/jpeg", b"x", UnixMicros(1)).unwrap();
    let copied = account(3);
    conn.close().unwrap();
    // A different configured account cannot adopt this account's database.
    std::fs::copy(r.0.path(a), r.0.path(copied)).unwrap();
    assert_eq!(r.get(copied, "A"), Err(Error::Identity));
}

#[test]
fn concurrent_distinct_keys_are_not_lost() {
    let (_temp, r) = setup();
    let start = Arc::new(Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let r = r.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                r.put(
                    account(1),
                    &format!("R{i}"),
                    "image/jpeg",
                    &[i],
                    UnixMicros(1),
                )
                .unwrap()
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    for i in 0..4 {
        assert_eq!(
            r.get(account(1), &format!("R{i}")).unwrap().unwrap().bytes,
            vec![i]
        );
    }
}

#[test]
fn brief_account_lock_waits_for_commit_without_blocking_another_account() {
    let (_temp, r) = setup();
    let mut conn = rusqlite::Connection::open(r.0.path(account(1))).unwrap();
    let lock = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let writer = r.clone();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = writer.put(account(1), "Waited", "image/jpeg", b"saved", UnixMicros(1));
        result_tx.send(result).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // The original 100ms policy fails while this real SQLite writer lock is held.
    let early = result_rx.recv_timeout(Duration::from_millis(300));
    let independent = r.put(account(2), "Other", "image/jpeg", b"other", UnixMicros(1));
    lock.commit().unwrap();
    let result = match early {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            result_rx.recv_timeout(Duration::from_secs(5)).unwrap()
        }
        Err(e) => panic!("writer disconnected: {e}"),
    };
    thread.join().unwrap();
    assert_eq!(independent.unwrap().bytes, b"other");
    assert_eq!(result.unwrap().bytes, b"saved");
    assert_eq!(
        r.get(account(1), "Waited").unwrap().unwrap().bytes,
        b"saved"
    );
    assert_eq!(r.get(account(2), "Waited").unwrap(), None);
}

#[test]
fn persistent_lock_returns_busy_preserving_record_and_retry_identity() {
    let (_temp, r) = setup();
    let a = account(1);
    let prior = r
        .put(a, "A", "image/jpeg", b"prior", UnixMicros(1))
        .unwrap();
    let mut conn = rusqlite::Connection::open(r.0.path(a)).unwrap();
    let lock = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let started = Instant::now();
    let result = r.put(a, "A", "image/jpeg", b"next", UnixMicros(2));
    let elapsed = started.elapsed();
    lock.rollback().unwrap();
    assert_eq!(result, Err(Error::Busy));
    // Allow scheduling overhead; a lock cannot cause an unlimited retry loop.
    assert!(elapsed < Duration::from_secs(8), "{elapsed:?}");
    assert_eq!(r.get(a, "A").unwrap(), Some(prior));
    let saved = r.put(a, "A", "image/jpeg", b"next", UnixMicros(2)).unwrap();
    assert_eq!(
        r.put(a, "A", "image/jpeg", b"next", UnixMicros(3)).unwrap(),
        saved
    );
    assert_eq!(saved.created, UnixMicros(1));
    assert_eq!(saved.updated, UnixMicros(2));
}
