// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_storage::{
    AccountId, Batch, DefinitionGuid, Error, GarageSlots, InventoryRepository, ItemId, ItemRecord,
    Op, SqliteRepository, Timestamp,
    account::{Document, Kind, Repository},
    records::{Repository as _, SqliteRecords, UnixMicros},
    tables::{Row, Table, Value},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "account-maintenance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn repository(&self, name: &str) -> SqliteRepository {
        SqliteRepository::open_owned_directory(&self.0.join(name)).unwrap()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn counter(value: i32) -> Op {
    Op::SetTable(
        7,
        Table {
            rows: BTreeMap::from([(0, Row::from([(1, Value::Int(value))]))]),
        },
    )
}
fn populated(root: &Root) -> SqliteRepository {
    let path = root.0.join("source");
    let documents = Kind::ALL
        .into_iter()
        .map(|k| {
            (
                k,
                Document {
                    revision: 1,
                    bytes: format!("{k:?}").into_bytes(),
                },
            )
        })
        .collect::<Vec<_>>();
    let item = ItemRecord {
        id: ItemId::new(3).unwrap(),
        definition: DefinitionGuid([9; 16]),
        owner: None,
        state: 2,
        buy_price: 500,
        sell_price: 300,
        default_items: vec![],
        sub_items: vec![],
        derived: vec![4, 5, 6],
    };
    Repository::open_owned_directory(&path)
        .unwrap()
        .initialize_fresh(
            account(1),
            &documents,
            &Batch {
                id: 1,
                expected_generation: 0,
                ops: vec![
                    Op::Insert(item.clone()),
                    Op::SetGarage(GarageSlots([Some(item.id), None, None, None, None])),
                    counter(1),
                ],
            },
            Timestamp(100),
        )
        .unwrap();
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    SqliteRecords(repo.clone())
        .put(
            account(1),
            "wrap",
            "application/octet-stream",
            &[3; 300],
            UnixMicros(7),
        )
        .unwrap();
    repo
}

#[test]
fn a_writer_lease_is_exclusive_per_account_and_readers_remain_available() {
    let root = Root::new();
    let repo = populated(&root);
    let reader =
        SqliteRepository::open_existing_directory(repo.path(account(1)).parent().unwrap()).unwrap();
    let before = repo.inspect(account(1)).unwrap();
    assert!(!reader.writer_is_leased(account(1)).unwrap());
    assert!(
        !reader
            .path(account(1))
            .with_extension("writer.lock")
            .exists()
    );
    let held = repo.writer_lease(account(1)).unwrap();
    assert!(reader.writer_is_leased(account(1)).unwrap());
    assert!(!reader.writer_is_leased(account(2)).unwrap());
    assert_eq!(reader.writer_lease(account(1)).unwrap_err(), Error::Busy);
    let independent = reader.writer_lease(account(2)).unwrap();
    assert_eq!(
        reader.inspect(account(1)).unwrap().snapshot,
        before.snapshot
    );
    drop(held);
    assert!(!reader.writer_is_leased(account(1)).unwrap());
    drop(reader.writer_lease(account(1)).unwrap());
    drop(independent);
}

#[test]
fn backup_includes_committed_wal_domains_livery_and_retry_history_and_reopens_alone() {
    let root = Root::new();
    let repo = populated(&root);
    let who = account(1);
    // Keep a writer connection open so the newest committed generation stays
    // in WAL rather than being checkpointed by the last writer's close.
    let hold = rusqlite::Connection::open(repo.path(who)).unwrap();
    hold.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let latest = Batch {
        id: 81,
        expected_generation: repo.snapshot(who).unwrap().generation,
        ops: vec![counter(42)],
    };
    repo.apply(who, &latest, Timestamp(200)).unwrap();
    let wal = repo.path(who).with_extension("sqlite-wal");
    let main_bytes = std::fs::read(repo.path(who)).unwrap();
    let wal_bytes = std::fs::read(&wal).unwrap();
    assert!(!wal_bytes.is_empty());
    let before = repo.inspect(who).unwrap();
    let lease = repo.writer_lease(who).unwrap();
    let backup = root.repository("backup");
    let saved = repo
        .backup_into(&lease, before.snapshot.generation, &backup)
        .unwrap();
    assert_eq!(saved.snapshot, before.snapshot);
    assert_eq!(saved.documents, before.documents);
    assert_eq!(saved.livery, before.livery);
    assert_eq!(std::fs::read(repo.path(who)).unwrap(), main_bytes);
    assert_eq!(std::fs::read(&wal).unwrap(), wal_bytes);
    assert!(!backup.path(who).with_extension("sqlite-wal").exists());
    assert!(!backup.path(who).with_extension("sqlite-shm").exists());
    drop(hold);
    drop(lease);
    let restored = root.repository("restored");
    std::fs::copy(backup.path(who), restored.path(who)).unwrap();
    let view = restored.inspect(who).unwrap();
    assert_eq!(view.snapshot, before.snapshot);
    assert_eq!(view.documents, before.documents);
    assert_eq!(view.livery, before.livery);
    assert!(
        restored
            .apply(who, &latest, Timestamp(300))
            .unwrap()
            .replayed
    );
    assert_eq!(restored.snapshot(who).unwrap(), before.snapshot);
}

#[test]
fn stale_absent_foreign_and_existing_destinations_never_replace_a_save() {
    let root = Root::new();
    let repo = populated(&root);
    let who = account(1);
    let lease = repo.writer_lease(who).unwrap();
    let target = root.repository("backup");
    let before = repo.inspect(who).unwrap();
    assert_eq!(
        repo.backup_into(&lease, before.snapshot.generation + 1, &target),
        Err(Error::Conflict)
    );
    assert!(!target.path(who).exists());
    let foreign = root.repository("foreign");
    assert_eq!(
        foreign.backup_into(&lease, before.snapshot.generation, &target),
        Err(Error::Config)
    );
    assert_eq!(
        repo.backup_into(&lease, before.snapshot.generation, &repo),
        Err(Error::Config)
    );
    let missing = repo.writer_lease(account(2)).unwrap();
    assert_eq!(repo.backup_into(&missing, 0, &target), Err(Error::Absent));
    assert!(!repo.path(account(2)).exists());
    assert!(!target.path(account(2)).exists());
    std::fs::write(target.path(who), b"retain destination").unwrap();
    assert_eq!(
        repo.backup_into(&lease, before.snapshot.generation, &target),
        Err(Error::Config)
    );
    assert_eq!(
        std::fs::read(target.path(who)).unwrap(),
        b"retain destination"
    );
    assert_eq!(repo.inspect(who).unwrap().snapshot, before.snapshot);
}

#[test]
fn a_failed_validation_removes_its_new_backup_without_changing_the_source() {
    let root = Root::new();
    let repo = populated(&root);
    let who = account(1);
    let corrupt = rusqlite::Connection::open(repo.path(who)).unwrap();
    // The inventory remains valid, but the independent account-domain table
    // has an invalid revision. Complete backup inspection must catch it.
    corrupt
        .execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE identity_state SET revision=0;")
        .unwrap();
    drop(corrupt);
    let before = std::fs::read(repo.path(who)).unwrap();
    let lease = repo.writer_lease(who).unwrap();
    let target = root.repository("backup");
    let generation = repo.snapshot(who).unwrap().generation;
    assert!(repo.backup_into(&lease, generation, &target).is_err());
    assert!(!target.path(who).exists());
    assert_eq!(std::fs::read(repo.path(who)).unwrap(), before);
}

#[test]
fn writer_lease_is_released_when_its_process_is_terminated() {
    use std::process::{Command, Stdio};
    let root = Root::new();
    let repo = root.repository("process");
    let ready = root.0.join("ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_holder_child", "--nocapture"])
        .env(
            "AFTERHOURS_TEST_LEASE_ROOT",
            repo.path(account(1)).parent().unwrap(),
        )
        .env("AFTERHOURS_TEST_LEASE_READY", &ready)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready.exists() && std::time::Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let observed = ready.exists();
    let refused = repo.writer_lease(account(1)).err();
    let _ = child.kill();
    child.wait().unwrap();
    assert!(observed, "lease child did not become ready");
    assert_eq!(refused, Some(Error::Busy));
    drop(repo.writer_lease(account(1)).unwrap());
}

#[test]
fn lease_holder_child() {
    use std::io::Read;
    let Some(path) = std::env::var_os("AFTERHOURS_TEST_LEASE_ROOT") else {
        return;
    };
    let repo = SqliteRepository::open_existing_directory(std::path::Path::new(&path)).unwrap();
    let _lease = repo.writer_lease(account(1)).unwrap();
    std::fs::write(
        std::env::var_os("AFTERHOURS_TEST_LEASE_READY").unwrap(),
        b"ready",
    )
    .unwrap();
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
}
