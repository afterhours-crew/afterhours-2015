// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One contract suite run against both adapters, plus SQLite file checks and
//! an adapter-equivalence run under random batches. Temporary directories are
//! unique per test and removed on drop; no EA services, no game.
use nfs_storage::{
    AccountId, Applied, Batch, DefinitionGuid, Error, InventoryRepository, ItemId, ItemRecord,
    MAX_BATCH_HISTORY, MAX_DERIVED, MAX_ITEMS, MAX_NESTED, MAX_OPS, MemoryRepository, Op,
    SqliteRepository, Timestamp,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct TempDir(PathBuf);
impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "nfs-storage-{label}-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temporary directory");
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn account(seed: u8) -> AccountId {
    AccountId::from_owned_config([seed; 16]).expect("nonzero account")
}
fn id(v: u64) -> ItemId {
    ItemId::new(v).expect("nonzero id")
}
fn record(v: u64, defaults: &[u64], subs: &[u64]) -> ItemRecord {
    ItemRecord {
        id: id(v),
        definition: DefinitionGuid([u8::try_from(v % 251).unwrap_or(0); 16]),
        owner: None,
        state: u8::try_from(v % 7).unwrap_or(0),
        buy_price: u32::try_from(v * 100).unwrap_or(0),
        sell_price: u32::try_from(v * 50).unwrap_or(0),
        default_items: defaults.iter().map(|d| id(*d)).collect(),
        sub_items: subs.iter().map(|s| id(*s)).collect(),
        derived: vec![u8::try_from(v % 256).unwrap_or(0); 4],
    }
}
fn batch(bid: u64, generation: u64, ops: Vec<Op>) -> Batch {
    Batch {
        id: bid,
        expected_generation: generation,
        ops,
    }
}
fn now(ms: u64) -> Timestamp {
    Timestamp(ms)
}

fn garage_contract(repo: &dyn InventoryRepository) {
    use nfs_storage::GarageSlots;
    let a = account(61);
    let b = account(62);
    repo.open(a).unwrap();
    repo.open(b).unwrap();
    let mut car = record(10, &[], &[]);
    car.id = id(u64::MAX);
    let slots = GarageSlots([Some(car.id), None, Some(id(11)), None, None]);
    let initial = batch(
        1,
        0,
        vec![
            Op::SetGarage(slots),
            Op::Insert(car.clone()),
            Op::Insert(record(11, &[], &[])),
        ],
    );
    repo.apply(a, &initial, now(1)).unwrap();
    assert_eq!(repo.snapshot(a).unwrap().garage, Some(slots));
    assert_eq!(repo.snapshot(b).unwrap().garage, None);
    assert!(repo.apply(a, &initial, now(2)).unwrap().replayed);
    assert_eq!(
        repo.apply(b, &batch(1, 0, vec![Op::SetGarage(slots)]), now(1)),
        Err(Error::DanglingReference)
    );
    let saved = repo.snapshot(a).unwrap();
    assert_eq!(
        repo.apply(a, &batch(2, 1, vec![Op::Remove(car.id)]), now(3)),
        Err(Error::DanglingReference)
    );
    let duplicate = GarageSlots([Some(car.id), Some(car.id), None, None, None]);
    assert_eq!(
        repo.apply(a, &batch(2, 1, vec![Op::SetGarage(duplicate)]), now(3)),
        Err(Error::Invalid)
    );
    assert_eq!(
        repo.apply(
            a,
            &batch(2, 1, vec![Op::SetGarage(slots), Op::SetGarage(slots)]),
            now(3)
        ),
        Err(Error::Invalid)
    );
    assert_eq!(repo.snapshot(a).unwrap(), saved);
    repo.apply(
        a,
        &batch(
            2,
            1,
            vec![Op::Remove(car.id), Op::SetGarage(GarageSlots::default())],
        ),
        now(4),
    )
    .unwrap();
    assert_eq!(
        repo.snapshot(a).unwrap().garage,
        Some(GarageSlots::default())
    );
}

#[test]
fn garage_is_atomic_retry_safe_and_account_owned_in_both_adapters() {
    garage_contract(&MemoryRepository::new());
    let dir = TempDir::new("garage");
    let repo = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    garage_contract(&repo);
    let saved = repo.snapshot(account(61)).unwrap();
    let reopened = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    assert_eq!(reopened.open(account(61)).unwrap(), saved);
}

#[test]
fn v1_migration_preserves_inventory_and_rejects_wrong_account_before_writing() {
    let dir = TempDir::new("garage-migrate");
    let repo = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    let a = account(63);
    repo.open(a).unwrap();
    repo.apply(
        a,
        &batch(4, 0, vec![Op::Insert(record(1, &[], &[]))]),
        now(3),
    )
    .unwrap();
    let saved = repo.snapshot(a).unwrap();
    let conn = rusqlite::Connection::open(repo.path(a)).unwrap();
    conn.execute_batch(
        "DROP TABLE identity_state; DROP TABLE entitlement_state; DROP TABLE kickback_state; DROP TABLE speedwall_state; DROP TABLE user_settings; DROP TABLE persistent; ALTER TABLE meta DROP COLUMN garage; PRAGMA user_version=1;",
    )
    .unwrap();
    conn.execute(
        "UPDATE meta SET account=?1",
        [account(64).bytes().as_slice()],
    )
    .unwrap();
    assert_eq!(repo.open(a), Err(Error::Identity));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    conn.execute("UPDATE meta SET account=?1", [a.bytes().as_slice()])
        .unwrap();
    drop(conn);
    assert_eq!(repo.open(a).unwrap(), saved);
    assert_eq!(repo.open(a).unwrap(), saved);
}

fn table_contract(repo: &dyn InventoryRepository) {
    use nfs_storage::{
        GarageSlots,
        tables::{MAX_TABLES, Row, Table, Value},
    };
    use std::collections::BTreeMap;
    let a = account(71);
    let b = account(72);
    repo.open(a).unwrap();
    repo.open(b).unwrap();
    let table = Table {
        rows: BTreeMap::from([(
            u64::MAX,
            Row::from([
                (0, Value::Int(-7)),
                (u32::MAX, Value::String("state".into())),
            ]),
        )]),
    };
    let first = batch(
        1,
        0,
        vec![
            Op::Insert(record(1, &[], &[])),
            Op::SetGarage(GarageSlots([Some(id(1)), None, None, None, None])),
            Op::SetTable(u32::MAX, table.clone()),
        ],
    );
    repo.apply(a, &first, now(1)).unwrap();
    let saved = repo.snapshot(a).unwrap();
    assert_eq!(saved.tables[&u32::MAX], table);
    assert!(repo.snapshot(b).unwrap().tables.is_empty());
    assert!(repo.apply(a, &first, now(2)).unwrap().replayed);
    // A failed inventory/garage change cannot consume the table write or retry id.
    let invalid = batch(
        2,
        1,
        vec![Op::SetTable(u32::MAX, Table::default()), Op::Remove(id(1))],
    );
    assert_eq!(
        repo.apply(a, &invalid, now(3)),
        Err(Error::DanglingReference)
    );
    assert_eq!(repo.snapshot(a).unwrap(), saved);
    let repeated_target = batch(
        2,
        1,
        vec![Op::SetTable(7, table), Op::SetTable(7, Table::default())],
    );
    assert_eq!(repo.apply(a, &repeated_target, now(3)), Err(Error::Invalid));
    assert_eq!(
        repo.apply(
            a,
            &batch(2, 0, vec![Op::SetTable(7, Table::default())]),
            now(3)
        ),
        Err(Error::Conflict)
    );
    let too_many = batch(
        2,
        1,
        (0..MAX_TABLES as u32)
            .map(|k| Op::SetTable(k, Table::default()))
            .collect(),
    );
    assert_eq!(repo.apply(a, &too_many, now(3)), Err(Error::Bounds));
    assert_eq!(repo.snapshot(a).unwrap(), saved);
    repo.apply(
        a,
        &batch(2, 1, vec![Op::SetTable(u32::MAX, Table::default())]),
        now(4),
    )
    .unwrap();
    assert_eq!(
        repo.snapshot(a).unwrap().tables.get(&u32::MAX),
        Some(&Table::default())
    );
    assert_eq!(repo.open(a).unwrap().generation, 2);
}

#[test]
fn tables_share_atomicity_retry_and_account_boundaries_and_survive_restart() {
    table_contract(&MemoryRepository::new());
    let dir = TempDir::new("tables");
    let repo = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    table_contract(&repo);
    let saved = repo.snapshot(account(71)).unwrap();
    drop(repo);
    let reopened = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    assert_eq!(reopened.open(account(71)).unwrap(), saved);
}

#[test]
fn v2_migration_checks_identity_and_preserves_committed_garage() {
    use nfs_storage::GarageSlots;
    let dir = TempDir::new("tables-migrate");
    let repo = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    let a = account(73);
    repo.open(a).unwrap();
    repo.apply(
        a,
        &batch(
            9,
            0,
            vec![
                Op::Insert(record(1, &[], &[])),
                Op::SetGarage(GarageSlots([Some(id(1)), None, None, None, None])),
            ],
        ),
        now(6),
    )
    .unwrap();
    let saved = repo.snapshot(a).unwrap();
    let conn = rusqlite::Connection::open(repo.path(a)).unwrap();
    conn.execute_batch("DROP TABLE identity_state; DROP TABLE entitlement_state; DROP TABLE kickback_state; DROP TABLE speedwall_state; DROP TABLE user_settings; DROP TABLE persistent; PRAGMA user_version=2;")
        .unwrap();
    conn.execute(
        "UPDATE meta SET account=?1",
        [account(74).bytes().as_slice()],
    )
    .unwrap();
    assert_eq!(repo.open(a), Err(Error::Identity));
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    conn.execute("UPDATE meta SET account=?1", [a.bytes().as_slice()])
        .unwrap();
    drop(conn);
    assert_eq!(repo.open(a).unwrap(), saved);
    assert_eq!(repo.open(a).unwrap(), saved);
    assert!(
        repo.apply(
            a,
            &batch(9, 0, vec![Op::SetGarage(GarageSlots::default())]),
            now(8)
        )
        .unwrap()
        .replayed
    );
    assert_eq!(repo.snapshot(a).unwrap(), saved);
}

#[test]
fn sqlite_rejects_corrupt_persistent_rows_before_other_writes() {
    let dir = TempDir::new("tables-corrupt");
    let repo = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    let a = account(75);
    repo.open(a).unwrap();
    let conn = rusqlite::Connection::open(repo.path(a)).unwrap();
    conn.execute(
        "INSERT INTO persistent VALUES(7,?1)",
        [b"TBL1\0\0\0\xff".as_slice()],
    )
    .unwrap();
    assert_eq!(repo.snapshot(a), Err(Error::Config));
    assert_eq!(
        repo.apply(
            a,
            &batch(1, 0, vec![Op::Insert(record(1, &[], &[]))]),
            now(1)
        ),
        Err(Error::Config)
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM item", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn v3_migration_preserves_inventory_tables_slots_and_retry_history() {
    let dir = TempDir::new("account-migrate");
    let repo = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    table_contract(&repo);
    let a = account(71);
    let saved = repo.snapshot(a).unwrap();
    let conn = rusqlite::Connection::open(repo.path(a)).unwrap();
    conn.execute_batch("DROP TABLE identity_state; DROP TABLE entitlement_state; DROP TABLE kickback_state; DROP TABLE speedwall_state; DROP TABLE user_settings; PRAGMA user_version=3;").unwrap();
    drop(conn);
    assert_eq!(repo.open(a).unwrap(), saved);
    assert!(
        repo.apply(
            a,
            &batch(
                2,
                1,
                vec![Op::SetTable(
                    u32::MAX,
                    nfs_storage::tables::Table::default()
                )]
            ),
            now(4)
        )
        .unwrap()
        .replayed
    );
    let services = nfs_storage::account::Repository::open_owned_directory(&dir.0).unwrap();
    for kind in nfs_storage::account::Kind::ALL {
        assert!(services.read(a, kind).unwrap().is_none());
    }
}

fn owner_graph(repo: &dyn InventoryRepository) {
    let a = account(21);
    let b = account(22);
    repo.open(a).unwrap();
    repo.open(b).unwrap();
    let mut vehicle = record(10, &[], &[11]);
    vehicle.id = id(u64::MAX);
    let mut part = record(11, &[], &[]);
    part.owner = Some(vehicle.id);
    // The parent id exists only in a, so it cannot satisfy an owner in b.
    repo.apply(
        a,
        &batch(
            1,
            0,
            vec![Op::Insert(vehicle.clone()), Op::Insert(part.clone())],
        ),
        now(1),
    )
    .unwrap();
    let saved = repo.open(a).unwrap();
    assert_eq!(saved.items[&part.id].owner, Some(id(u64::MAX)));
    assert_eq!(saved.items[&vehicle.id].owner, None);
    assert_eq!(
        repo.apply(b, &batch(1, 0, vec![Op::Insert(part.clone())]), now(2)),
        Err(Error::DanglingReference)
    );
    assert_eq!(
        repo.apply(a, &batch(2, 1, vec![Op::Remove(vehicle.id)]), now(2)),
        Err(Error::DanglingReference)
    );
    assert_eq!(repo.snapshot(a).unwrap(), saved);
    part.owner = Some(part.id);
    assert_eq!(
        repo.apply(a, &batch(2, 1, vec![Op::Update(part.clone())]), now(2)),
        Err(Error::Invalid)
    );
    part.owner = None;
    repo.apply(
        a,
        &batch(2, 1, vec![Op::Remove(vehicle.id), Op::Update(part)]),
        now(3),
    )
    .unwrap();
    assert_eq!(repo.snapshot(a).unwrap().items.len(), 1);
}

#[test]
fn parent_item_ownership_round_trips_and_remains_account_scoped() {
    owner_graph(&MemoryRepository::new());
    let dir = TempDir::new("owner-graph");
    let sqlite = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    owner_graph(&sqlite);
    let reopened = SqliteRepository::open_owned_directory(&dir.0).unwrap();
    assert_eq!(reopened.snapshot(account(21)), sqlite.snapshot(account(21)));
}

#[test]
fn both_adapters_share_the_signed_generation_ceiling() {
    let change = batch(1, i64::MAX as u64, vec![Op::Insert(record(1, &[], &[]))]);
    assert_eq!(
        nfs_storage::port::decide(i64::MAX as u64, None, &change, now(0)),
        Err(Error::Bounds)
    );
}

fn suite(repo: &dyn InventoryRepository) {
    let a = account(1);
    // Snapshot before open is Absent; open yields an empty generation 0.
    assert_eq!(repo.snapshot(a).err(), Some(Error::Absent));
    let fresh = repo.open(a).expect("open");
    assert_eq!(fresh.generation, 0);
    assert!(fresh.items.is_empty());
    assert_eq!(repo.open(a).expect("reopen"), fresh);

    // Insert two items; the second references the first.
    let applied = repo
        .apply(
            a,
            &batch(
                1,
                0,
                vec![
                    Op::Insert(record(10, &[], &[])),
                    Op::Insert(record(11, &[10], &[])),
                ],
            ),
            now(1_000),
        )
        .expect("insert");
    assert_eq!(
        applied,
        Applied {
            generation: 1,
            replayed: false
        }
    );
    let s = repo.snapshot(a).expect("snapshot");
    assert_eq!(s.generation, 1);
    assert_eq!(s.updated_at, now(1_000));
    assert_eq!(s.items.len(), 2);
    assert_eq!(s.items[&id(11)].default_items, vec![id(10)]);
    assert_eq!(repo.open(a).expect("open keeps state"), s);

    // Replay of the same batch id returns the recorded result without change,
    // whatever generation the retry claims.
    for expected in [0, 1, 99] {
        let replay = repo
            .apply(
                a,
                &batch(1, expected, vec![Op::Insert(record(10, &[], &[]))]),
                now(2_000),
            )
            .expect("replay");
        assert_eq!(
            replay,
            Applied {
                generation: 1,
                replayed: true
            }
        );
    }
    assert_eq!(repo.snapshot(a).expect("unchanged"), s);

    // Stale generation is a conflict and changes nothing.
    assert_eq!(
        repo.apply(
            a,
            &batch(2, 0, vec![Op::Insert(record(12, &[], &[]))]),
            now(3_000)
        ),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(a).expect("unchanged"), s);

    // A batch is atomic: a valid insert alongside an invalid update is dropped.
    assert_eq!(
        repo.apply(
            a,
            &batch(
                2,
                1,
                vec![
                    Op::Insert(record(12, &[], &[])),
                    Op::Update(record(99, &[], &[])),
                ],
            ),
            now(3_000)
        ),
        Err(Error::UnknownItem)
    );
    assert_eq!(repo.snapshot(a).expect("unchanged"), s);
    // The failed batch id was not recorded, so it can be applied correctly.
    assert_eq!(
        repo.apply(
            a,
            &batch(2, 1, vec![Op::Insert(record(12, &[], &[]))]),
            now(3_000)
        ),
        Ok(Applied {
            generation: 2,
            replayed: false
        })
    );

    // Duplicate insert, dangling references on insert and on remove.
    assert_eq!(
        repo.apply(
            a,
            &batch(3, 2, vec![Op::Insert(record(12, &[], &[]))]),
            now(4_000)
        ),
        Err(Error::DuplicateItem)
    );
    assert_eq!(
        repo.apply(
            a,
            &batch(3, 2, vec![Op::Insert(record(13, &[], &[500]))]),
            now(4_000)
        ),
        Err(Error::DanglingReference)
    );
    assert_eq!(
        repo.apply(a, &batch(3, 2, vec![Op::Remove(id(10))]), now(4_000)),
        Err(Error::DanglingReference)
    );
    // Removing the referrer first, then the referenced item, works across
    // batches and within one batch.
    assert_eq!(
        repo.apply(
            a,
            &batch(3, 2, vec![Op::Remove(id(11)), Op::Remove(id(10))]),
            now(4_000)
        ),
        Ok(Applied {
            generation: 3,
            replayed: false
        })
    );
    assert_eq!(
        repo.apply(a, &batch(4, 3, vec![Op::Remove(id(10))]), now(5_000)),
        Err(Error::UnknownItem)
    );

    // Update replaces the whole record.
    let mut updated = record(12, &[], &[]);
    updated.state = 5;
    updated.derived = vec![9; 70];
    assert_eq!(
        repo.apply(
            a,
            &batch(4, 3, vec![Op::Update(updated.clone())]),
            now(5_000)
        ),
        Ok(Applied {
            generation: 4,
            replayed: false
        })
    );
    let s = repo.snapshot(a).expect("snapshot");
    assert_eq!(s.items.len(), 1);
    assert_eq!(s.items[&id(12)], updated);
    assert_eq!(s.updated_at, now(5_000));

    // Malformed batches and policy bounds.
    let invalid = [
        batch(0, 4, vec![Op::Remove(id(12))]),
        batch(5, 4, vec![]),
        batch(
            5,
            4,
            vec![Op::Remove(id(12)), Op::Update(record(12, &[], &[]))],
        ),
        batch(5, 4, vec![Op::Insert(record(14, &[14], &[]))]),
        batch(5, 4, vec![Op::Insert(record(14, &[12, 12], &[]))]),
    ];
    for b in &invalid {
        assert_eq!(repo.apply(a, b, now(6_000)), Err(Error::Invalid), "{b:?}");
    }
    let mut big = record(14, &[], &[]);
    big.derived = vec![0; MAX_DERIVED + 1];
    assert_eq!(
        repo.apply(a, &batch(5, 4, vec![Op::Insert(big)]), now(6_000)),
        Err(Error::Bounds)
    );
    let mut wide = record(14, &[], &[]);
    wide.sub_items = (100..100 + u64::try_from(MAX_NESTED + 1).expect("fits"))
        .map(id)
        .collect();
    assert_eq!(
        repo.apply(a, &batch(5, 4, vec![Op::Insert(wide)]), now(6_000)),
        Err(Error::Bounds)
    );
    let many = (1000..1000 + u64::try_from(MAX_OPS + 1).expect("fits"))
        .map(|v| Op::Insert(record(v, &[], &[])))
        .collect();
    assert_eq!(
        repo.apply(a, &batch(5, 4, many), now(6_000)),
        Err(Error::Bounds)
    );
    assert_eq!(
        repo.apply(
            a,
            &batch(5, 4, vec![Op::Remove(id(12))]),
            Timestamp(u64::MAX)
        ),
        Err(Error::Bounds)
    );
    assert_eq!(repo.snapshot(a).expect("unchanged"), s);

    // Accounts are isolated.
    let b = account(2);
    assert_eq!(repo.snapshot(b).err(), Some(Error::Absent));
    assert!(repo.open(b).expect("open b").items.is_empty());
    assert_eq!(
        repo.apply(
            b,
            &batch(1, 0, vec![Op::Insert(record(12, &[], &[]))]),
            now(7_000)
        ),
        Ok(Applied {
            generation: 1,
            replayed: false
        })
    );
    assert_eq!(repo.snapshot(a).expect("a unchanged"), s);
    assert_eq!(repo.snapshot(b).expect("b").generation, 1);
}

fn fill_to_limit(repo: &dyn InventoryRepository, a: AccountId) {
    // MAX_ITEMS fits in MAX_ITEMS / MAX_OPS batches; one more insert is Bounds.
    let mut generation = repo.open(a).expect("open").generation;
    let mut next = 1u64;
    let mut bid = 1_000u64;
    while next <= u64::try_from(MAX_ITEMS).expect("fits") {
        let end = (next + u64::try_from(MAX_OPS).expect("fits"))
            .min(u64::try_from(MAX_ITEMS).expect("fits") + 1);
        let ops = (next..end)
            .map(|v| Op::Insert(record(v, &[], &[])))
            .collect();
        let applied = repo
            .apply(a, &batch(bid, generation, ops), now(bid))
            .expect("fill");
        generation = applied.generation;
        next = end;
        bid += 1;
    }
    assert_eq!(repo.snapshot(a).expect("full").items.len(), MAX_ITEMS);
    assert_eq!(
        repo.apply(
            a,
            &batch(bid, generation, vec![Op::Insert(record(next, &[], &[]))]),
            now(bid)
        ),
        Err(Error::Bounds)
    );
}

#[test]
fn memory_contract() {
    suite(&MemoryRepository::new());
}

#[test]
fn sqlite_contract() {
    let dir = TempDir::new("contract");
    let repo = SqliteRepository::open_owned_directory(&dir.0).expect("repository");
    suite(&repo);
}

#[test]
fn memory_item_limit() {
    fill_to_limit(&MemoryRepository::new(), account(3));
}

#[test]
fn sqlite_item_limit() {
    let dir = TempDir::new("limit");
    let repo = SqliteRepository::open_owned_directory(&dir.0).expect("repository");
    fill_to_limit(&repo, account(3));
}

#[test]
fn memory_history_is_bounded() {
    // Once a batch id leaves the history its retry is an ordinary conflict.
    let repo = MemoryRepository::new();
    let a = account(4);
    repo.open(a).expect("open");
    let first = batch(1, 0, vec![Op::Insert(record(1, &[], &[]))]);
    repo.apply(a, &first, now(1)).expect("first");
    for n in 1..=u64::try_from(MAX_BATCH_HISTORY).expect("fits") {
        let ops = vec![Op::Update(record(1, &[], &[]))];
        repo.apply(a, &batch(n + 1, n, ops), now(n)).expect("fill");
    }
    assert_eq!(repo.apply(a, &first, now(2)), Err(Error::Conflict));
    assert_eq!(
        repo.apply(
            a,
            &batch(2, 1, vec![Op::Update(record(1, &[], &[]))]),
            now(2)
        )
        .map(|a| a.replayed),
        Ok(true)
    );
}

#[test]
fn sqlite_persists_across_repository_instances() {
    let dir = TempDir::new("reopen");
    let a = account(5);
    let expected = {
        let repo = SqliteRepository::open_owned_directory(&dir.0).expect("repository");
        repo.open(a).expect("open");
        repo.apply(
            a,
            &batch(
                7,
                0,
                vec![
                    Op::Insert(record(1, &[], &[])),
                    Op::Insert(record(2, &[], &[1])),
                ],
            ),
            now(42),
        )
        .expect("apply");
        repo.snapshot(a).expect("snapshot")
    };
    let repo = SqliteRepository::open_owned_directory(&dir.0).expect("repository again");
    assert_eq!(repo.snapshot(a).expect("persisted"), expected);
    assert_eq!(repo.open(a).expect("open again"), expected);
    // The replay history persisted too.
    let replay = repo
        .apply(a, &batch(7, 0, vec![Op::Remove(id(1))]), now(43))
        .expect("replay");
    assert!(replay.replayed);
    assert_eq!(repo.snapshot(a).expect("unchanged"), expected);
}

#[test]
fn sqlite_rejects_foreign_unsupported_and_garbage_files() {
    let dir = TempDir::new("files");
    let repo = SqliteRepository::open_owned_directory(&dir.0).expect("repository");
    let a = account(6);
    let b = account(7);
    repo.open(a).expect("open a");
    repo.apply(
        a,
        &batch(1, 0, vec![Op::Insert(record(1, &[], &[]))]),
        now(1),
    )
    .expect("apply");

    // A copy of A's database under B's name is another account's store.
    fs::copy(repo.path(a), repo.path(b)).expect("copy");
    assert_eq!(repo.open(b), Err(Error::Identity));
    assert_eq!(repo.snapshot(b), Err(Error::Identity));
    assert_eq!(
        repo.apply(b, &batch(1, 0, vec![Op::Remove(id(1))]), now(2)),
        Err(Error::Identity)
    );

    // A newer schema version is not adopted.
    let c = account(8);
    repo.open(c).expect("open c");
    {
        let conn = rusqlite::Connection::open(repo.path(c)).expect("direct");
        conn.pragma_update(None, "user_version", 99).expect("bump");
    }
    assert_eq!(repo.open(c), Err(Error::Version));
    assert_eq!(repo.snapshot(c), Err(Error::Version));

    // A file that is not a database is rejected without adoption.
    let d = account(9);
    fs::write(repo.path(d), b"not a database at all").expect("garbage");
    assert!(repo.open(d).is_err());
    assert!(repo.snapshot(d).is_err());
    assert_eq!(
        fs::read(repo.path(d)).expect("read"),
        b"not a database at all"
    );

    // A directory in place of the file is a configuration error.
    let e = account(10);
    fs::create_dir(repo.path(e)).expect("dir");
    assert_eq!(repo.open(e), Err(Error::Config));
    assert_eq!(repo.snapshot(e), Err(Error::Config));

    // A still works.
    assert_eq!(repo.snapshot(a).expect("a").items.len(), 1);
}

#[test]
fn sqlite_root_must_be_a_directory() {
    let dir = TempDir::new("root");
    let file = dir.0.join("file");
    fs::write(&file, b"x").expect("file");
    assert!(SqliteRepository::open_owned_directory(&file).is_err());
    let nested = dir.0.join("a").join("b");
    assert!(SqliteRepository::open_owned_directory(&nested).is_ok());
}

/// Small deterministic generator; no third-party dependency.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn adapters_agree_under_random_batches() {
    let dir = TempDir::new("equivalence");
    let sqlite = SqliteRepository::open_owned_directory(&dir.0).expect("repository");
    let memory = MemoryRepository::new();
    let a = account(11);
    assert_eq!(sqlite.open(a), memory.open(a));
    let mut rng = Lcg(0x5eed_e603);
    let mut generation = 0u64;
    let mut applied_ids = Vec::new();
    for step in 1..=160u64 {
        let ids_in_play = 12u64;
        let ops_len = 1 + rng.below(4);
        let mut ops = Vec::new();
        for _ in 0..ops_len {
            let target = 1 + rng.below(ids_in_play);
            let make = |rng: &mut Lcg| {
                let mut r = record(target, &[], &[]);
                for _ in 0..rng.below(3) {
                    let other = 1 + rng.below(ids_in_play);
                    if other != target && !r.sub_items.contains(&id(other)) {
                        r.sub_items.push(id(other));
                    }
                }
                r.derived = vec![
                    u8::try_from(rng.below(256)).expect("byte");
                    usize::try_from(rng.below(16)).expect("fits")
                ];
                r
            };
            ops.push(match rng.below(3) {
                0 => Op::Insert(make(&mut rng)),
                1 => Op::Update(make(&mut rng)),
                _ => Op::Remove(id(target)),
            });
        }
        // Occasionally retry an old batch id or claim a stale generation.
        let (bid, expected) = match rng.below(10) {
            0 if !applied_ids.is_empty() => {
                let i = usize::try_from(rng.below(applied_ids.len() as u64)).expect("fits");
                (applied_ids[i], generation)
            }
            1 => (step + 10_000, generation.wrapping_sub(1)),
            _ => (step + 10_000, generation),
        };
        let b = batch(bid, expected, ops);
        let t = now(step);
        let left = sqlite.apply(a, &b, t);
        let right = memory.apply(a, &b, t);
        assert_eq!(left, right, "step {step}: {b:?}");
        if let Ok(Applied {
            generation: g,
            replayed: false,
        }) = left
        {
            generation = g;
            applied_ids.push(bid);
        }
        assert_eq!(sqlite.snapshot(a), memory.snapshot(a), "step {step}");
    }
    assert!(generation > 10, "the run should apply many batches");
    // Fresh handle sees the same final state.
    let again = SqliteRepository::open_owned_directory(&dir.0).expect("repository");
    assert_eq!(again.snapshot(a), memory.snapshot(a));
}
