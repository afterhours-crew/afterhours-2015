// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn store(repo: &dyn InventoryRepository, item: u64, slot: u8) -> Result<Snapshot, Error> {
    move_to_warehouse(repo, who(1), &catalog(), item, slot, Timestamp(30))
}
fn take(
    repo: &dyn InventoryRepository,
    item: u64,
    slot: u8,
    old: Option<u64>,
) -> Result<Snapshot, Error> {
    take_from_warehouse(repo, who(1), &catalog(), item, slot, old, Timestamp(31))
}

#[test]
fn transfer_retrieval_and_swap_preserve_all_owned_records_and_cash() {
    let repo = MemoryRepository::default();
    let epoch = init(&repo, who(1));
    init(&repo, who(2));
    let other = repo.snapshot(who(2)).unwrap();
    let a = car(&purchase(&repo, who(1), epoch, 2).unwrap());
    let b = car(&purchase(&repo, who(1), epoch, 3).unwrap());
    let before = repo.snapshot(who(1)).unwrap();
    let stored = store(&repo, a, 0).unwrap();
    assert_eq!(
        stored.garage.unwrap().0,
        [Some(id(10)), None, Some(id(b)), None, None]
    );
    assert_eq!(store(&repo, a, 0).unwrap(), stored);
    let retrieved = take(&repo, a, 3, None).unwrap();
    assert_eq!(take(&repo, a, 3, None).unwrap(), retrieved);
    store(&repo, a, 3).unwrap();
    let swapped = take(&repo, a, 1, Some(b)).unwrap();
    assert_eq!(
        swapped.garage.unwrap().0,
        [Some(id(10)), None, Some(id(a)), None, None]
    );
    assert_eq!(take(&repo, a, 1, Some(b)).unwrap(), swapped);
    let after = take(&repo, b, 0, None).unwrap();
    assert_eq!(after.items, before.items);
    assert_eq!(after.tables, before.tables);
    assert_eq!(repo.snapshot(who(2)).unwrap(), other);
}

#[test]
fn stale_slot_reuse_primary_foreign_parts_and_active_targets_are_rejected_safely() {
    let repo = MemoryRepository::default();
    let epoch = init(&repo, who(1));
    init(&repo, who(2));
    let a = car(&purchase(&repo, who(1), epoch, 2).unwrap());
    let stored = store(&repo, a, 0).unwrap();
    let b = car(&purchase(&repo, who(1), epoch, 3).unwrap());
    let before = repo.snapshot(who(1)).unwrap();
    // Retry the resolved move after its original slot has been reused.
    assert_eq!(store(&repo, a, 0).unwrap(), before);
    assert_eq!(
        take(&repo, a, 0, None),
        Err(Error::Storage(nfs_storage::Error::Conflict))
    );
    assert_eq!(take(&repo, b, 1, None), Err(Error::Rejected(Reason::Owner)));
    assert_eq!(store(&repo, 10, 0), Err(Error::Rejected(Reason::Primary)));
    assert_eq!(
        take(&repo, 10, 1, None),
        Err(Error::Rejected(Reason::Primary))
    );
    assert_eq!(
        store(&repo, b, 1),
        Err(Error::Storage(nfs_storage::Error::Conflict))
    );
    let child = stored.items[&id(a)].sub_items[0].get();
    for item in [child, u64::MAX] {
        assert_eq!(store(&repo, item, 0), Err(Error::Rejected(Reason::Owner)));
        assert_eq!(
            take(&repo, item, 0, Some(b)),
            Err(Error::Rejected(Reason::Owner))
        );
    }
    assert_eq!(
        move_to_warehouse(&repo, who(2), &catalog(), a, 0, Timestamp(32)),
        Err(Error::Rejected(Reason::Owner))
    );
    assert_eq!(
        take_from_warehouse(&repo, who(2), &catalog(), a, 0, None, Timestamp(32)),
        Err(Error::Rejected(Reason::Owner))
    );
    for slot in [4, u8::MAX] {
        assert_eq!(store(&repo, b, slot), Err(Error::Rejected(Reason::Shape)));
        assert_eq!(
            take(&repo, a, slot, None),
            Err(Error::Rejected(Reason::Shape))
        );
    }
    assert_eq!(repo.snapshot(who(1)).unwrap(), before);
}

#[test]
fn warehouse_survives_sqlite_restart_and_retains_customized_car_and_parts() {
    let path = std::env::temp_dir().join(format!(
        "afterhours-warehouse-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (car, before) = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let epoch = init(&repo, who(1));
        let car = car(&purchase(&repo, who(1), epoch, 2).unwrap());
        let snapshot = repo.snapshot(who(1)).unwrap();
        let mut record = snapshot.items[&id(car)].clone();
        // The typed car suffix accepts these distinct customization bytes.
        record.derived[0] = 27;
        repo.apply(
            who(1),
            &Batch {
                id: 76,
                expected_generation: snapshot.generation,
                ops: vec![Op::Update(record)],
            },
            Timestamp(25),
        )
        .unwrap();
        let stored = store(&repo, car, 0).unwrap();
        (car, stored)
    };
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        assert_eq!(repo.open(who(1)).unwrap(), before);
        let after = take(&repo, car, 2, None).unwrap();
        assert_eq!(after.items, before.items);
        assert_eq!(after.garage.unwrap().0[3], Some(id(car)));
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn warehouse_limit_allows_swaps_at_capacity_and_freed_slot_purchases() {
    let repo = MemoryRepository::default();
    let epoch = init(&repo, who(1));
    let base = catalog();
    let mut definitions = base
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect::<Vec<_>>();
    let parts = (5..68).map(|n| [n; 16]).collect::<Vec<_>>();
    definitions
        .iter_mut()
        .find(|(g, _)| *g == [1; 16])
        .unwrap()
        .1
        .sub_items = parts.clone();
    for guid in parts {
        definitions.push((guid, base.definition(&[2; 16]).unwrap().clone()));
    }
    let c = InventoryCatalog::new(definitions, vec![[1; 16]]).unwrap();
    let offers = Offers::new(&c, [([1; 16], 0)]).unwrap();
    let buy_car = |sequence| {
        car(&buy(
            &repo,
            who(1),
            &c,
            &offers,
            CommandId {
                session: epoch,
                sequence,
            },
            &PurchaseRequest {
                sequence,
                owner: 0,
                selection: PurchaseSelection::Definitions(vec![[1; 16]]),
            },
            Timestamp(25),
        )
        .unwrap())
    };
    let store = |item, slot| move_to_warehouse(&repo, who(1), &c, item, slot, Timestamp(30));
    let take =
        |item, slot, old| take_from_warehouse(&repo, who(1), &c, item, slot, old, Timestamp(31));
    let snapshot = repo.snapshot(who(1)).unwrap();
    repo.apply(
        who(1),
        &Batch {
            id: 123,
            expected_generation: snapshot.generation,
            ops: vec![Balance(100_000).store()],
        },
        Timestamp(20),
    )
    .unwrap();
    let mut stored = Vec::new();
    for n in 0..WAREHOUSE_CAPACITY {
        let item = buy_car(n as u32 + 2);
        store(item, 0).unwrap();
        stored.push(item);
    }
    let parked = buy_car(102);
    let before = repo.snapshot(who(1)).unwrap();
    assert_eq!(store(parked, 0), Err(Error::Rejected(Reason::Full)));
    assert_eq!(repo.snapshot(who(1)).unwrap(), before);
    let swapped = take(stored[0], 0, Some(parked)).unwrap();
    assert_eq!(swapped.items, before.items);
    assert_eq!(store(stored[0], 0), Err(Error::Rejected(Reason::Full)));
    take(parked, 1, None).unwrap();
    let after = store(stored[0], 0).unwrap();
    assert!(after.items.len() > 4096);
    assert_eq!(nfs_storage::MAX_ITEMS, nfs_world_core::items::MAX_ITEMS);
    let wire = inventory::collection(&after, c.bindings()).unwrap();
    let bytes = wire.encode(c.bindings()).unwrap();
    assert_eq!(
        nfs_world_core::items::Collection::decode(&bytes, c.bindings()).unwrap(),
        wire
    );
}

#[test]
fn concurrent_retrieval_of_one_car_never_duplicates_it_in_two_slots() {
    let repo = Arc::new(MemoryRepository::default());
    let epoch = init(repo.as_ref(), who(1));
    let item = car(&purchase(repo.as_ref(), who(1), epoch, 2).unwrap());
    store(repo.as_ref(), item, 0).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2)
        .map(|slot| {
            let repo = repo.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                take(repo.as_ref(), item, slot, None)
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let snapshot = repo.snapshot(who(1)).unwrap();
    assert_eq!(
        snapshot
            .garage
            .unwrap()
            .0
            .iter()
            .filter(|v| **v == Some(id(item)))
            .count(),
        1
    );
}
