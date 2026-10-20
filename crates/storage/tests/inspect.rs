// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_storage::{
    AccountId, Batch, DefinitionGuid, Error, GarageSlots, InventoryRepository, ItemId, ItemRecord,
    Op, SqliteRepository, Timestamp,
    account::{Document, Kind, Repository},
    records::{Repository as _, SqliteRecords, Summary, UnixMicros},
    tables::{Row, Table, Value},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "inspect-storage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
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
fn item(id: u64) -> ItemRecord {
    ItemRecord {
        id: ItemId::new(id).unwrap(),
        definition: DefinitionGuid([id as u8; 16]),
        owner: None,
        state: 2,
        buy_price: 0,
        sell_price: 0,
        default_items: vec![],
        sub_items: vec![],
        derived: vec![],
    }
}
fn counter(value: u64) -> Op {
    Op::SetTable(
        1,
        Table {
            rows: BTreeMap::from([(0, Row::from([(0, Value::Int(value as i32))]))]),
        },
    )
}
/// A fully initialized store: every domain, one car in the primary slot, a
/// table and one livery record. Every writer connection is closed afterwards.
fn populated(root: &Path, a: AccountId) -> SqliteRepository {
    let documents: Vec<_> = Kind::ALL
        .into_iter()
        .map(|kind| {
            (
                kind,
                Document {
                    revision: 1,
                    bytes: format!("{kind:?}").into_bytes(),
                },
            )
        })
        .collect();
    let batch = Batch {
        id: 1,
        expected_generation: 0,
        ops: vec![
            Op::Insert(item(7)),
            Op::SetGarage(GarageSlots([
                Some(ItemId::new(7).unwrap()),
                None,
                None,
                None,
                None,
            ])),
            counter(1),
        ],
    };
    Repository::open_owned_directory(root)
        .unwrap()
        .initialize_fresh(a, &documents, &batch, Timestamp(1_760_000_000_000))
        .unwrap();
    let repo = SqliteRepository::open_owned_directory(root).unwrap();
    SqliteRecords(repo.clone())
        .put(
            a,
            "wrap-1",
            "application/octet-stream",
            &[9; 300],
            UnixMicros(5),
        )
        .unwrap();
    repo
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[test]
fn inspection_reads_every_domain_in_one_view_and_never_writes_the_database() {
    let root = Root::new();
    let a = account(1);
    let writer = populated(&root.0, a);
    let path = writer.path(a);
    let before = std::fs::read(&path).unwrap();
    let reader = SqliteRepository::open_existing_directory(&root.0).unwrap();
    let view = reader.inspect(a).unwrap();
    assert_eq!(view.schema_version, nfs_storage::sqlite::VERSION);
    assert_eq!(view.file_bytes, before.len() as u64);
    assert_eq!(view.snapshot.generation, 1);
    assert_eq!(view.snapshot.updated_at, Timestamp(1_760_000_000_000));
    assert_eq!(view.snapshot.items.len(), 1);
    assert_eq!(view.documents.len(), Kind::ALL.len());
    assert_eq!(view.documents[&Kind::Identity].revision, 1);
    assert_eq!(view.documents[&Kind::Settings].bytes, b"Settings");
    assert_eq!(
        view.livery,
        Summary {
            records: 1,
            bytes: 300
        }
    );
    assert_eq!(reader.inspect(a).unwrap(), view, "repeatable");
    // The database file is byte-for-byte unchanged and no log frame exists.
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let wal = sidecar(&path, "-wal");
    assert_eq!(std::fs::metadata(&wal).map_or(0, |m| m.len()), 0);
    assert_eq!(view.snapshot, writer.snapshot(a).unwrap());
    // The writer keeps working after a reader left its sidecar files behind.
    writer
        .apply(
            a,
            &Batch {
                id: 2,
                expected_generation: 1,
                ops: vec![counter(2)],
            },
            Timestamp(1_760_000_000_001),
        )
        .unwrap();
    assert_eq!(reader.inspect(a).unwrap().snapshot.generation, 2);
}

#[test]
fn absent_stores_are_refused_without_creating_anything() {
    let root = Root::new();
    let missing = root.0.join("missing");
    assert_eq!(
        SqliteRepository::open_existing_directory(&missing).err(),
        Some(Error::Absent)
    );
    assert!(!missing.exists());
    let reader = SqliteRepository::open_existing_directory(&root.0).unwrap();
    assert_eq!(reader.inspect(account(1)), Err(Error::Absent));
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
    std::fs::write(&missing, b"not a directory").unwrap();
    assert_eq!(
        SqliteRepository::open_existing_directory(&missing).err(),
        Some(Error::Config)
    );
}

#[test]
fn other_accounts_schema_versions_damage_and_oversized_files_are_refused() {
    let root = Root::new();
    let a = account(1);
    let repo = populated(&root.0, a);
    let reader = SqliteRepository::open_existing_directory(&root.0).unwrap();
    // A copy under another account's name is not that account's store.
    std::fs::copy(repo.path(a), repo.path(account(2))).unwrap();
    assert_eq!(reader.inspect(account(2)), Err(Error::Identity));
    // A newer schema is never read as the current one.
    let newer = account(3);
    std::fs::copy(repo.path(a), repo.path(newer)).unwrap();
    let conn = rusqlite::Connection::open(repo.path(newer)).unwrap();
    conn.execute("UPDATE meta SET account=?1", [newer.bytes().as_slice()])
        .unwrap();
    assert_eq!(reader.inspect(newer).unwrap().snapshot.generation, 1);
    conn.execute_batch("PRAGMA user_version=6;").unwrap();
    drop(conn);
    assert_eq!(reader.inspect(newer), Err(Error::Version));
    // Garbage and oversized files fail closed.
    std::fs::write(repo.path(account(4)), vec![0x5a; 8192]).unwrap();
    assert_eq!(reader.inspect(account(4)), Err(Error::Storage));
    let large = std::fs::File::create(repo.path(account(5))).unwrap();
    large.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(large);
    assert_eq!(reader.inspect(account(5)), Err(Error::Config));
    assert_eq!(reader.inspect(a).unwrap().snapshot.generation, 1);
}

#[test]
fn a_concurrent_writer_never_tears_or_blocks_the_view() {
    let root = Root::new();
    let a = account(1);
    let repo = populated(&root.0, a);
    let reader = SqliteRepository::open_existing_directory(&root.0).unwrap();
    // An open, uncommitted write transaction is invisible and does not block.
    let raw = rusqlite::Connection::open(repo.path(a)).unwrap();
    raw.execute_batch("BEGIN IMMEDIATE; UPDATE meta SET generation=generation+100;")
        .unwrap();
    let started = Instant::now();
    assert_eq!(reader.inspect(a).unwrap().snapshot.generation, 1);
    assert!(started.elapsed() < Duration::from_secs(1));
    raw.execute_batch("COMMIT;").unwrap();
    assert_eq!(reader.inspect(a).unwrap().snapshot.generation, 101);
    raw.execute_batch("UPDATE meta SET generation=1;").unwrap();
    drop(raw);
    // Every view taken during a stream of commits is one committed state:
    // the counter table always equals the generation that wrote it.
    let writer = std::thread::spawn({
        let repo = repo.clone();
        move || {
            for generation in 1..120u64 {
                repo.apply(
                    a,
                    &Batch {
                        id: generation + 1,
                        expected_generation: generation,
                        ops: vec![counter(generation + 1)],
                    },
                    Timestamp(generation),
                )
                .unwrap();
            }
        }
    });
    let mut views = 0;
    while !writer.is_finished() || views == 0 {
        let view = reader.inspect(a).unwrap();
        let stored = &view.snapshot.tables[&1].rows[&0][&0];
        assert_eq!(*stored, Value::Int(view.snapshot.generation as i32));
        views += 1;
    }
    writer.join().unwrap();
    assert_eq!(reader.inspect(a).unwrap().snapshot.generation, 120);
}

#[test]
fn an_exclusive_lock_is_waited_for_only_within_the_busy_bound() {
    let root = Root::new();
    let a = account(1);
    let repo = populated(&root.0, a);
    let reader = SqliteRepository::open_existing_directory(&root.0).unwrap();
    let raw = rusqlite::Connection::open(repo.path(a)).unwrap();
    raw.execute_batch(
        "PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; UPDATE meta SET generation=generation;",
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(reader.inspect(a), Err(Error::Busy));
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(500), "{waited:?}");
    assert!(waited < Duration::from_secs(10), "{waited:?}");
    raw.execute_batch("COMMIT; PRAGMA locking_mode=NORMAL;")
        .unwrap();
    drop(raw);
    assert_eq!(reader.inspect(a).unwrap().snapshot.generation, 1);
}

#[cfg(unix)]
#[test]
fn a_linked_database_file_is_refused() {
    let root = Root::new();
    let a = account(1);
    let repo = populated(&root.0, a);
    std::os::unix::fs::symlink(repo.path(a), repo.path(account(2))).unwrap();
    let reader = SqliteRepository::open_existing_directory(&root.0).unwrap();
    assert_eq!(reader.inspect(account(2)), Err(Error::Config));
}
