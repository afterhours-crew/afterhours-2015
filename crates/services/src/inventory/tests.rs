// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_storage::{MemoryRepository, SqliteRepository};
use nfs_world_core::items::{Definition, DefinitionClass, DefinitionFlags, OWNED, OwnershipLevel};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicU64, Ordering},
};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn catalog() -> InventoryCatalog {
    let definition = |sub_items| Definition {
        class: DefinitionClass::CurrencyItemData,
        buy_price: 120,
        sell_price: 70,
        quantity: u32::MAX,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags::default(),
        sub_items,
        additional_items: Vec::new(),
        default_derived: Some(Derived::Empty),
    };
    InventoryCatalog::new(
        [
            ([1; 16], definition(vec![[2; 16]])),
            ([2; 16], definition(vec![])),
        ],
        vec![[1; 16]],
    )
    .unwrap()
}
fn request(sequence: u32) -> [u8; 5] {
    Request {
        sequence,
        operation: items::LOAD_INVENTORY,
    }
    .encode()
}

fn vehicle_catalog() -> InventoryCatalog {
    let definition = Definition {
        class: DefinitionClass::RaceVehicleItemData,
        buy_price: 10,
        sell_price: 5,
        quantity: 1,
        ownership_level: OwnershipLevel::Owned,
        flags: DefinitionFlags::default(),
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::decode(items::Layout::RaceVehicle, &[0; 68]).unwrap()),
    };
    InventoryCatalog::new([([4; 16], definition)], vec![[4; 16]]).unwrap()
}

#[test]
fn table_load_and_inventory_reply_share_one_committed_snapshot_across_restart() {
    let directory = Temp::new();
    let repo = SqliteRepository::open_owned_directory(&directory.0).unwrap();
    let who = account(75);
    let items = vehicle_catalog();
    let tables = crate::persistent::tests::catalog();
    let reply = answer_load_with_tables(
        &request(1),
        &repo,
        who,
        &items,
        u64::MAX - 1,
        Timestamp(1),
        Some(&tables),
    )
    .unwrap()
    .unwrap();
    let view = reply.persistent.as_ref().unwrap();
    assert_eq!((reply.generation, view.generation()), (2, 2));
    assert_eq!(view.garage(), reply.garage);
    assert_eq!(reply.garage[0], Some(u64::MAX - 1));
    assert_eq!(view.tables().len(), 16);
    let reopened = SqliteRepository::open_owned_directory(&directory.0).unwrap();
    let again = answer_load_with_tables(
        &request(1),
        &reopened,
        who,
        &items,
        1,
        Timestamp(9),
        Some(&tables),
    )
    .unwrap()
    .unwrap();
    assert_eq!(reply.bytes, again.bytes);
    assert_eq!(reply.persistent, again.persistent);
    let snapshot = reopened.snapshot(who).unwrap();
    assert_eq!(snapshot.tables.len(), 15);
    assert_eq!(snapshot.updated_at, Timestamp(1));
}

#[test]
fn garage_uses_owned_vehicle_and_preserves_explicit_empty_state_across_restart() {
    let directory = Temp::new();
    let repo = SqliteRepository::open_owned_directory(&directory.0).unwrap();
    let who = account(71);
    let catalog = vehicle_catalog();
    let reply = answer_load(
        &request(9),
        &repo,
        who,
        &catalog,
        u64::MAX - 1,
        Timestamp(1),
    )
    .unwrap()
    .unwrap();
    assert_eq!(reply.garage, [Some(u64::MAX - 1), None, None, None, None]);
    assert_eq!(reply.generation, 1);
    let loaded = repo.snapshot(who).unwrap();
    assert_eq!(loaded.garage.unwrap().0[0].unwrap().get(), u64::MAX - 1);
    repo.apply(
        who,
        &Batch {
            id: 2,
            expected_generation: 1,
            ops: vec![Op::SetGarage(GarageSlots::default())],
        },
        Timestamp(2),
    )
    .unwrap();
    let reopened = SqliteRepository::open_owned_directory(&directory.0).unwrap();
    let reply = answer_load(&request(10), &reopened, who, &catalog, 1, Timestamp(3))
        .unwrap()
        .unwrap();
    assert_eq!(reply.garage, [None; 5]);
    assert_eq!(reply.generation, 2);
    assert_eq!(reopened.snapshot(who).unwrap().updated_at, Timestamp(2));
}

#[test]
fn migrated_garage_is_initialized_once_and_invalid_vehicle_bindings_are_rejected() {
    let repo = MemoryRepository::default();
    let who = account(72);
    let catalog = vehicle_catalog();
    let collection = catalog.instantiate_initial(123).unwrap().collection;
    repo.open(who).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops: records(&collection, catalog.bindings())
                .unwrap()
                .into_iter()
                .map(Op::Insert)
                .collect(),
        },
        Timestamp(1),
    )
    .unwrap();
    let loaded = load(&repo, who, &catalog, 1, Timestamp(2)).unwrap();
    assert_eq!(loaded.generation, 2);
    assert_eq!(loaded.garage.unwrap().0[0].unwrap().get(), 123);
    assert_eq!(load(&repo, who, &catalog, 1, Timestamp(3)).unwrap(), loaded);
    let mut bad = loaded.clone();
    bad.items.values_mut().next().unwrap().state = items::PURCHASABLE;
    assert!(super::collection(&bad, catalog.bindings()).is_err());
    let mut bad = loaded;
    bad.garage = Some(GarageSlots([
        Some(ItemId::new(99).unwrap()),
        None,
        None,
        None,
        None,
    ]));
    assert!(super::collection(&bad, catalog.bindings()).is_err());
}

#[test]
fn loads_commit_before_reply_and_repeats_preserve_progress() {
    let repository = MemoryRepository::default();
    let catalog = catalog();
    let who = account(1);
    let first = (1u64 << 63) + 42;
    let reply = answer_load(
        &request(11),
        &repository,
        who,
        &catalog,
        first,
        Timestamp(1),
    )
    .unwrap()
    .unwrap();
    assert_eq!(reply.generation, 1);
    let state = repository.snapshot(who).unwrap();
    let wire = Envelope::decode(&reply.bytes).unwrap();
    assert_eq!(wire.sequence, 11);
    assert_eq!(
        wire.collection(catalog.bindings()).unwrap(),
        collection(&state, catalog.bindings()).unwrap()
    );
    let key = ItemId::new(first + 1).unwrap();
    let mut child = state.items[&key].clone();
    child.sell_price = 3;
    child.state = 8;
    repository
        .apply(
            who,
            &Batch {
                id: 2,
                expected_generation: 1,
                ops: vec![Op::Update(child)],
            },
            Timestamp(2),
        )
        .unwrap();
    let reply = answer_load(&request(12), &repository, who, &catalog, 1, Timestamp(3))
        .unwrap()
        .unwrap();
    let decoded = Envelope::decode(&reply.bytes)
        .unwrap()
        .collection(catalog.bindings())
        .unwrap();
    assert_eq!(reply.generation, 2);
    assert_eq!(decoded.items[&(first + 1)].sell_price, 3);
    assert_eq!(decoded.items[&(first + 1)].state, 8);
    assert_eq!(decoded.items[&first].state, OWNED);
    assert_eq!(decoded.items[&(first + 1)].owner, first);
    assert_eq!(repository.snapshot(who).unwrap().updated_at, Timestamp(2));
}

#[test]
fn unknown_and_malformed_requests_do_not_open_a_profile() {
    let repository = MemoryRepository::default();
    let catalog = catalog();
    let who = account(2);
    assert!(
        answer_load(
            &Request {
                sequence: 9,
                operation: 255
            }
            .encode(),
            &repository,
            who,
            &catalog,
            1,
            Timestamp(1)
        )
        .unwrap()
        .is_none()
    );
    assert!(
        answer_load(
            &request(9)[..4],
            &repository,
            who,
            &catalog,
            1,
            Timestamp(1)
        )
        .is_err()
    );
    assert_eq!(repository.snapshot(who), Err(nfs_storage::Error::Absent));
}

#[test]
fn a_failed_commit_emits_no_reply_and_retry_initializes_once() {
    struct FailOnce {
        inner: MemoryRepository,
        fail: std::sync::atomic::AtomicBool,
    }
    impl InventoryRepository for FailOnce {
        fn receipt(
            &self,
            account: AccountId,
            batch_id: u64,
        ) -> Result<Option<nfs_storage::Applied>, nfs_storage::Error> {
            self.inner.receipt(account, batch_id)
        }
        fn open(&self, account: AccountId) -> Result<Snapshot, nfs_storage::Error> {
            self.inner.open(account)
        }
        fn snapshot(&self, account: AccountId) -> Result<Snapshot, nfs_storage::Error> {
            self.inner.snapshot(account)
        }
        fn apply(
            &self,
            account: AccountId,
            batch: &Batch,
            now: Timestamp,
        ) -> Result<nfs_storage::Applied, nfs_storage::Error> {
            if self.fail.swap(false, Ordering::SeqCst) {
                return Err(nfs_storage::Error::Storage);
            }
            self.inner.apply(account, batch, now)
        }
    }
    let repository = FailOnce {
        inner: MemoryRepository::new(),
        fail: std::sync::atomic::AtomicBool::new(true),
    };
    let catalog = catalog();
    let who = account(7);
    assert!(matches!(
        answer_load(&request(1), &repository, who, &catalog, 1, Timestamp(1)),
        Err(Error::Store(nfs_storage::Error::Storage))
    ));
    let empty = repository.snapshot(who).unwrap();
    assert_eq!(empty.generation, 0);
    assert!(empty.items.is_empty());
    let reply = answer_load(&request(1), &repository, who, &catalog, 1, Timestamp(2))
        .unwrap()
        .unwrap();
    assert_eq!(reply.generation, 1);
    assert_eq!(repository.snapshot(who).unwrap().items.len(), 2);
}

#[test]
fn storage_conversion_checks_class_data_references_and_cycles() {
    let catalog = catalog();
    let original = catalog.instantiate_initial(1).unwrap().collection;
    let stored_records = records(&original, catalog.bindings()).unwrap();
    let mut snapshot = Snapshot {
        items: stored_records.into_iter().map(|r| (r.id, r)).collect(),
        ..Snapshot::default()
    };
    assert_eq!(collection(&snapshot, catalog.bindings()).unwrap(), original);
    let key = ItemId::new(1).unwrap();
    snapshot.items.get_mut(&key).unwrap().derived.push(0);
    assert!(collection(&snapshot, catalog.bindings()).is_err());
    snapshot.items.get_mut(&key).unwrap().derived.clear();
    let child = ItemId::new(2).unwrap();
    snapshot.items.get_mut(&child).unwrap().sub_items.push(key);
    assert_eq!(
        collection(&snapshot, catalog.bindings()),
        Err(Error::Wire(items::Error::Cycle))
    );
    let mut bad = original;
    bad.items.get_mut(&2).unwrap().owner = 999;
    assert!(records(&bad, catalog.bindings()).is_err());
}

#[test]
fn intentionally_empty_later_generation_is_not_reseeded() {
    let repository = MemoryRepository::default();
    let catalog = catalog();
    let who = account(3);
    let loaded = load(&repository, who, &catalog, 1, Timestamp(1)).unwrap();
    repository
        .apply(
            who,
            &Batch {
                id: 2,
                expected_generation: 1,
                ops: loaded.items.keys().copied().map(Op::Remove).collect(),
            },
            Timestamp(2),
        )
        .unwrap();
    let loaded = load(&repository, who, &catalog, 1, Timestamp(3)).unwrap();
    assert_eq!(loaded.generation, 2);
    assert!(loaded.items.is_empty());
}

#[test]
fn racing_initial_loads_and_separate_accounts_have_one_owned_graph_each() {
    let repository = Arc::new(MemoryRepository::default());
    let catalog = Arc::new(catalog());
    let barrier = Arc::new(Barrier::new(4));
    let joins = (0..4)
        .map(|i| {
            let repository = Arc::clone(&repository);
            let catalog = Arc::clone(&catalog);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                load(
                    repository.as_ref(),
                    account(4 + i % 2),
                    &catalog,
                    100,
                    Timestamp(10),
                )
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    for join in joins {
        let state = join.join().unwrap();
        assert_eq!(state.generation, 1);
        assert_eq!(state.items.len(), 2);
    }
    let a = repository.snapshot(account(4)).unwrap();
    let mut record = a.items.values().next().unwrap().clone();
    record.buy_price = 2;
    repository
        .apply(
            account(4),
            &Batch {
                id: 2,
                expected_generation: 1,
                ops: vec![Op::Update(record)],
            },
            Timestamp(11),
        )
        .unwrap();
    assert_eq!(repository.snapshot(account(5)).unwrap().generation, 1);
    assert!(
        repository
            .snapshot(account(5))
            .unwrap()
            .items
            .values()
            .all(|r| r.buy_price == 120)
    );
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "nfs-services-inventory-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        // Only exact files in the directory this test created; no recursion.
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[test]
fn sqlite_restart_returns_modified_typed_state_and_exact_response_bytes() {
    let directory = Temp::new();
    let catalog = catalog();
    let who = account(6);
    let first = (1u64 << 63) + 90;
    let repository = SqliteRepository::open_owned_directory(&directory.0).unwrap();
    let initial = load(&repository, who, &catalog, first, Timestamp(1)).unwrap();
    let key = ItemId::new(first + 1).unwrap();
    let mut child = initial.items[&key].clone();
    child.buy_price = 999;
    repository
        .apply(
            who,
            &Batch {
                id: 2,
                expected_generation: 1,
                ops: vec![Op::Update(child)],
            },
            Timestamp(2),
        )
        .unwrap();
    let before = answer_load(&request(21), &repository, who, &catalog, 1, Timestamp(3))
        .unwrap()
        .unwrap();
    drop(repository);
    let reopened = SqliteRepository::open_owned_directory(&directory.0).unwrap();
    let after = answer_load(&request(21), &reopened, who, &catalog, 1, Timestamp(4))
        .unwrap()
        .unwrap();
    assert_eq!(after.generation, 2);
    assert_eq!(before.bytes, after.bytes);
    let decoded = Envelope::decode(&after.bytes)
        .unwrap()
        .collection(catalog.bindings())
        .unwrap();
    assert_eq!(decoded.items[&(first + 1)].buy_price, 999);
    assert_eq!(decoded.items[&(first + 1)].owner, first);
}
