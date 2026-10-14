// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::bank::Balance;
use nfs_storage::{DefinitionGuid, GarageSlots, ItemRecord, MemoryRepository};
use nfs_world_core::items::DefinitionClass;
use nfs_world_core::items::{Definition, DefinitionFlags, Layout, OwnershipLevel};

mod appearance;

fn catalog() -> InventoryCatalog {
    let part = Definition {
        class: DefinitionClass::AirFilterItemData,
        buy_price: 30,
        sell_price: 10,
        quantity: 1,
        ownership_level: OwnershipLevel::Owned,
        flags: DefinitionFlags {
            requires_owner: true,
            ..Default::default()
        },
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::Empty),
    };
    let car = Definition {
        class: DefinitionClass::RaceVehicleItemData,
        flags: DefinitionFlags::default(),
        default_derived: Some(Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap()),
        ..part.clone()
    };
    InventoryCatalog::new(
        [([1; 16], car), ([2; 16], part.clone()), ([3; 16], part)],
        vec![],
    )
    .unwrap()
}
fn who(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}

#[test]
fn front_and_rear_visual_slots_equip_independently_and_restore_recorded_defaults() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let original = setup(&repo, account);
    let mut definitions = catalog()
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect::<Vec<_>>();
    for (_, part) in &mut definitions[1..] {
        part.class = DefinitionClass::RimsItemData;
        part.default_derived =
            Some(Derived::decode(DefinitionClass::RimsItemData.layout(), &[0; 52]).unwrap());
    }
    for guid in [[4; 16], [5; 16]] {
        definitions.push((guid, definitions[1].1.clone()));
    }
    let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
    let id = |v| ItemId::new(v).unwrap();
    let mut car = original.items[&id(10)].clone();
    car.sub_items.push(id(13));
    car.default_items.push(id(13));
    let mut stock = original.items[&id(11)].clone();
    stock.derived = vec![0; 52];
    let mut front = original.items[&id(12)].clone();
    front.derived = vec![0; 52];
    let rear = ItemRecord {
        id: id(13),
        definition: DefinitionGuid([4; 16]),
        ..stock.clone()
    };
    let rear_new = ItemRecord {
        id: id(14),
        definition: DefinitionGuid([5; 16]),
        ..stock.clone()
    };
    repo.apply(
        account,
        &Batch {
            id: 2,
            expected_generation: original.generation,
            ops: vec![
                Op::Update(car),
                Op::Update(stock),
                Op::Update(front),
                Op::Insert(rear),
                Op::Insert(rear_new),
            ],
        },
        Timestamp(2),
    )
    .unwrap();
    let before = repo.snapshot(account).unwrap();
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    let offers = Offers::new([[3; 16], [5; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [([1; 16], vec![[3; 16], [5; 16]])])
        .unwrap();
    let locations = [([2; 16], 0), ([3; 16], 0), ([4; 16], 1), ([5; 16], 1)];
    assert!(
        offers
            .clone()
            .with_part_policy(
                &catalog,
                locations[..3].iter().copied(),
                [([3; 16], 0), ([5; 16], 0)]
            )
            .is_err()
    );
    assert!(
        prepare(
            account,
            before.generation,
            &items,
            10,
            12,
            &catalog,
            &offers
        )
        .is_err()
    );
    let offers = offers
        .with_part_policy(&catalog, locations, [([3; 16], 0), ([5; 16], 0)])
        .unwrap();
    let front = prepare(
        account,
        before.generation,
        &items,
        10,
        12,
        &catalog,
        &offers,
    )
    .unwrap();
    assert_eq!(
        front.preview(&items).unwrap().items[&10].children,
        vec![13, 12]
    );
    let both = front
        .with_part(&items, 14, &catalog, &offers)
        .unwrap()
        .with_part(&items, 12, &catalog, &offers)
        .unwrap();
    let saved = commit(&repo, account, &catalog, &both, Timestamp(3)).unwrap();
    assert_eq!(saved.items[&id(10)].sub_items, vec![id(12), id(14)]);
    assert_eq!(saved.items[&id(10)].default_items, vec![id(11), id(13)]);
    assert_eq!(saved.tables, before.tables);
    let items = inventory::collection(&saved, catalog.bindings()).unwrap();
    let restore = prepare(account, saved.generation, &items, 10, 11, &catalog, &offers)
        .unwrap()
        .with_part(&items, 13, &catalog, &offers)
        .unwrap();
    let restored = commit(&repo, account, &catalog, &restore, Timestamp(4)).unwrap();
    assert_eq!(restored.items[&id(10)].sub_items, vec![id(11), id(13)]);
    assert_eq!(restored.items.len(), 5);
}
fn setup(repo: &dyn InventoryRepository, account: AccountId) -> Snapshot {
    repo.open(account).unwrap();
    let id = |n| ItemId::new(n).unwrap();
    let car = ItemRecord {
        id: id(10),
        definition: DefinitionGuid([1; 16]),
        owner: None,
        state: OWNED,
        buy_price: 100,
        sell_price: 50,
        default_items: vec![id(11)],
        sub_items: vec![id(11)],
        derived: vec![0; 68],
    };
    let stock = ItemRecord {
        id: id(11),
        definition: DefinitionGuid([2; 16]),
        owner: Some(id(10)),
        state: OWNED,
        buy_price: 0,
        sell_price: 0,
        default_items: vec![],
        sub_items: vec![],
        derived: vec![],
    };
    let bought = ItemRecord {
        id: id(12),
        definition: DefinitionGuid([3; 16]),
        ..stock.clone()
    };
    repo.apply(
        account,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops: vec![
                Op::Insert(car),
                Op::Insert(stock),
                Op::Insert(bought),
                Op::SetGarage(GarageSlots([Some(id(10)), None, None, None, None])),
                Balance(70).store(),
            ],
        },
        Timestamp(1),
    )
    .unwrap();
    repo.snapshot(account).unwrap()
}
fn edit(snapshot: &Snapshot, part: u64) -> Edit {
    prepare(
        who(1),
        snapshot.generation,
        &inventory::collection(snapshot, catalog().bindings()).unwrap(),
        10,
        part,
        &catalog(),
        &Offers::new([[3; 16]]).unwrap(),
    )
    .unwrap()
}

#[test]
fn owning_a_part_does_not_bypass_vehicle_scope_and_stock_restore_remains_available() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let initial = setup(&repo, account);
    let catalog = catalog();
    let items = inventory::collection(&initial, catalog.bindings()).unwrap();
    let offers = Offers::new([[3; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [([1; 16], vec![[2; 16]])])
        .unwrap();
    assert_eq!(
        prepare(
            account,
            initial.generation,
            &items,
            10,
            12,
            &catalog,
            &offers
        ),
        Err(Error::Unsupported)
    );
    assert!(
        prepare(
            account,
            initial.generation,
            &items,
            10,
            11,
            &catalog,
            &offers
        )
        .is_ok()
    );
    assert_eq!(repo.snapshot(account).unwrap(), initial);
}

#[test]
fn staging_cancel_commit_retry_and_stock_restore_preserve_owned_spares_and_money() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let initial = setup(&repo, account);
    let pending = edit(&initial, 12);
    assert_eq!(repo.snapshot(account).unwrap(), initial); // cancel by dropping stage
    let saved = commit(&repo, account, &catalog(), &pending, Timestamp(2)).unwrap();
    let car = &saved.items[&ItemId::new(10).unwrap()];
    assert_eq!(car.sub_items, [ItemId::new(12).unwrap()]);
    assert_eq!(car.default_items, [ItemId::new(11).unwrap()]);
    assert_eq!(saved.items.len(), 3);
    assert_eq!(saved.tables, initial.tables);
    assert_eq!(
        commit(&repo, account, &catalog(), &pending, Timestamp(3)).unwrap(),
        saved
    );
    let restore = edit(&saved, 11);
    let restored = commit(&repo, account, &catalog(), &restore, Timestamp(4)).unwrap();
    assert_eq!(restored.items, initial.items);
    assert!(matches!(
        commit(&repo, account, &catalog(), &pending, Timestamp(5)),
        Err(Error::Conflict)
    ));
    assert_eq!(repo.snapshot(account).unwrap(), restored);
}

#[test]
fn foreign_owner_unknown_part_stale_generation_and_other_account_do_not_mutate() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let initial = setup(&repo, account);
    let other = setup(&repo, who(2)); // even identical ids/content cannot share a plan
    let pending = edit(&initial, 12);
    assert_eq!(
        commit(&repo, who(2), &catalog(), &pending, Timestamp(2)),
        Err(Error::Ownership)
    );
    assert_eq!(repo.snapshot(who(2)).unwrap(), other);
    let mut items = inventory::collection(&initial, catalog().bindings()).unwrap();
    let offers = Offers::new([[3; 16]]).unwrap();
    assert_eq!(
        prepare(account, 1, &items, 10, 99, &catalog(), &offers),
        Err(Error::Ownership)
    );
    items.items.get_mut(&12).unwrap().owner = 11;
    assert_eq!(
        prepare(account, 1, &items, 10, 12, &catalog(), &offers),
        Err(Error::Ownership)
    );
    repo.apply(
        account,
        &Batch {
            id: 3,
            expected_generation: 1,
            ops: vec![Balance(69).store()],
        },
        Timestamp(2),
    )
    .unwrap();
    let newer = repo.snapshot(account).unwrap();
    assert_eq!(
        commit(&repo, account, &catalog(), &pending, Timestamp(3)),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(account).unwrap(), newer);
}

#[test]
fn equipment_survives_sqlite_reopen_and_storage_failure_has_no_effect() {
    use nfs_storage::{Applied, SqliteRepository};
    let directory = std::env::temp_dir().join(format!(
        "afterhours-equipment-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let account = who(1);
    let (pending, saved) = {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        let before = setup(&repo, account);
        let pending = edit(&before, 12).with_dyno(dyno()).unwrap();
        let saved = commit(&repo, account, &catalog(), &pending, Timestamp(2)).unwrap();
        (pending, saved)
    };
    {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), saved);
        assert_eq!(
            commit(&repo, account, &catalog(), &pending, Timestamp(3)).unwrap(),
            saved
        );
    }
    std::fs::remove_dir_all(directory).unwrap();

    struct Refuses(MemoryRepository);
    impl InventoryRepository for Refuses {
        fn open(&self, account: AccountId) -> Result<Snapshot, nfs_storage::Error> {
            self.0.open(account)
        }
        fn snapshot(&self, account: AccountId) -> Result<Snapshot, nfs_storage::Error> {
            self.0.snapshot(account)
        }
        fn apply(
            &self,
            _: AccountId,
            _: &Batch,
            _: Timestamp,
        ) -> Result<Applied, nfs_storage::Error> {
            Err(nfs_storage::Error::Storage)
        }
    }
    let repo = Refuses(MemoryRepository::default());
    let before = setup(&repo.0, account);
    assert_eq!(
        commit(&repo, account, &catalog(), &edit(&before, 12), Timestamp(2)),
        Err(Error::Store(nfs_storage::Error::Storage))
    );
    assert_eq!(repo.snapshot(account).unwrap(), before);
}

fn dyno() -> nfs_world_core::items::DynoStats {
    nfs_world_core::items::DynoStats::new([180, 400, 500, 120], [4.5, 10., 12.5]).unwrap()
}

fn handling_fixture(
    repo: &dyn InventoryRepository,
    account: AccountId,
) -> (InventoryCatalog, Snapshot) {
    let before = setup(repo, account);
    let base = catalog();
    let mut definitions: Vec<_> = base
        .definitions()
        .map(|(guid, d)| (*guid, d.clone()))
        .collect();
    for (guid, class, words) in [
        (4, DefinitionClass::PersistantTuningItemData, 5),
        (5, DefinitionClass::BrakesItemData, 2),
        (6, DefinitionClass::BrakeDiscsItemData, 2),
    ] {
        definitions.push((
            [guid; 16],
            Definition {
                class,
                default_derived: Some(
                    Derived::decode(class.layout(), &vec![0; words * 4]).unwrap(),
                ),
                ..base.definition(&[2; 16]).unwrap().clone()
            },
        ));
    }
    let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
    let mut car = before.items[&ItemId::new(10).unwrap()].clone();
    car.sub_items
        .extend([14, 15, 16].map(|id| ItemId::new(id).unwrap()));
    car.default_items.push(ItemId::new(17).unwrap());
    let mut ops = vec![Op::Update(car)];
    for (id, guid, words) in [(14, 4, 5), (15, 5, 2), (16, 6, 2), (17, 4, 5)] {
        ops.push(Op::Insert(ItemRecord {
            id: ItemId::new(id).unwrap(),
            definition: DefinitionGuid([guid; 16]),
            derived: (0..words).flat_map(|_| 5u32.to_le_bytes()).collect(),
            ..before.items[&ItemId::new(11).unwrap()].clone()
        }));
    }
    repo.apply(
        account,
        &Batch {
            id: 2,
            expected_generation: before.generation,
            ops,
        },
        Timestamp(2),
    )
    .unwrap();
    (catalog, repo.snapshot(account).unwrap())
}

#[test]
fn handling_changes_installed_parts_and_car_atomically_without_touching_spares_or_cosmetics() {
    use nfs_world_core::items::HandlingChanges;
    let repo = MemoryRepository::default();
    let account = who(1);
    let (catalog, before) = handling_fixture(&repo, account);
    let (_, other) = handling_fixture(&repo, who(2));
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    let changes = HandlingChanges::new([(1, 7), (26, 6), (30, 10)]).unwrap();
    let pending = prepare(
        account,
        before.generation,
        &items,
        10,
        12,
        &catalog,
        &Offers::new([[3; 16]]).unwrap(),
    )
    .unwrap()
    .with_handling(&items, &catalog, changes, Some([10, 0]))
    .unwrap()
    .with_dyno(dyno())
    .unwrap();
    assert_eq!(repo.snapshot(account).unwrap(), before);
    let saved = commit(&repo, account, &catalog, &pending, Timestamp(3)).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(saved.tables, before.tables);
    assert_eq!(
        &saved.items[&ItemId::new(14).unwrap()].derived[12..16],
        &6u32.to_le_bytes()
    );
    assert_eq!(
        &saved.items[&ItemId::new(15).unwrap()].derived[..4],
        &7u32.to_le_bytes()
    );
    assert_eq!(
        &saved.items[&ItemId::new(10).unwrap()].derived[64..68],
        &4u32.to_le_bytes()
    );
    for id in [11, 12, 16, 17] {
        assert_eq!(
            saved.items[&ItemId::new(id).unwrap()],
            before.items[&ItemId::new(id).unwrap()]
        );
    }
    assert_eq!(repo.snapshot(who(2)).unwrap(), other);
    assert_eq!(
        commit(&repo, account, &catalog, &pending, Timestamp(4)).unwrap(),
        saved
    );
    assert_eq!(
        commit(&repo, who(2), &catalog, &pending, Timestamp(4)),
        Err(Error::Ownership)
    );
    let late = prepare_vehicle(account, before.generation, &items, 10, &catalog)
        .unwrap()
        .with_handling(
            &items,
            &catalog,
            HandlingChanges::new([(26, 4)]).unwrap(),
            None,
        )
        .unwrap();
    assert_eq!(
        commit(&repo, account, &catalog, &late, Timestamp(5)),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(account).unwrap(), saved);
}

#[test]
fn handling_rejects_wrong_owner_missing_parts_and_invalid_transmission_without_partial_writes() {
    use nfs_world_core::items::HandlingChanges;
    let repo = MemoryRepository::default();
    let account = who(1);
    let (catalog, before) = handling_fixture(&repo, account);
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    for (changes, transmission) in [
        (vec![(26, 6), (0, 7)], None), // no installed spoiler
        (vec![(30, 10)], None),
        (vec![(30, 5)], Some([10, 0])),
    ] {
        assert!(
            prepare_vehicle(account, before.generation, &items, 10, &catalog)
                .unwrap()
                .with_handling(
                    &items,
                    &catalog,
                    HandlingChanges::new(changes).unwrap(),
                    transmission
                )
                .is_err()
        );
        assert_eq!(repo.snapshot(account).unwrap(), before);
    }
    let mut wrong = items.clone();
    wrong.items.get_mut(&14).unwrap().owner = 12;
    let result =
        prepare_vehicle(account, before.generation, &wrong, 10, &catalog).and_then(|edit| {
            edit.with_handling(
                &wrong,
                &catalog,
                HandlingChanges::new([(26, 6)]).unwrap(),
                None,
            )
        });
    assert!(result.is_err());
}

#[test]
fn sequential_equip_keeps_other_categories_and_tunes_only_the_final_installed_part() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let (catalog, before) = handling_fixture(&repo, account);
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    let offers = Offers::new([[3; 16]]).unwrap();
    // Select an upgraded air filter, change the persistent tuning part, then
    // restore the filter. This must retain the second category's selection.
    let pending = prepare(
        account,
        before.generation,
        &items,
        10,
        12,
        &catalog,
        &offers,
    )
    .unwrap()
    .with_part(&items, 17, &catalog, &offers)
    .unwrap()
    .with_part(&items, 11, &catalog, &offers)
    .unwrap()
    .with_handling(
        &items,
        &catalog,
        nfs_world_core::items::HandlingChanges::new([(26, 9)]).unwrap(),
        None,
    )
    .unwrap();
    let after = commit(&repo, account, &catalog, &pending, Timestamp(3)).unwrap();
    let car = &after.items[&ItemId::new(10).unwrap()];
    assert!(car.sub_items.contains(&ItemId::new(11).unwrap()));
    assert!(car.sub_items.contains(&ItemId::new(17).unwrap()));
    assert!(!car.sub_items.contains(&ItemId::new(12).unwrap()));
    assert!(!car.sub_items.contains(&ItemId::new(14).unwrap()));
    assert_eq!(
        &after.items[&ItemId::new(17).unwrap()].derived[12..16],
        &9u32.to_le_bytes()
    );
    for id in [11, 12, 14, 15, 16] {
        assert_eq!(
            after.items[&ItemId::new(id).unwrap()],
            before.items[&ItemId::new(id).unwrap()]
        );
    }
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(
        commit(&repo, account, &catalog, &pending, Timestamp(4)).unwrap(),
        after
    );
}

#[test]
fn multiple_handling_parts_survive_sqlite_reopen() {
    let directory = std::env::temp_dir().join(format!(
        "afterhours-handling-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let account = who(1);
    let (catalog, pending, saved) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let (catalog, before) = handling_fixture(&repo, account);
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        let edit = prepare_vehicle(account, before.generation, &items, 10, &catalog)
            .unwrap()
            .with_handling(
                &items,
                &catalog,
                nfs_world_core::items::HandlingChanges::new([(1, 0), (26, 10), (27, 0)]).unwrap(),
                None,
            )
            .unwrap();
        let saved = commit(&repo, account, &catalog, &edit, Timestamp(3)).unwrap();
        (catalog, edit, saved)
    };
    {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), saved);
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(4)).unwrap(),
            saved
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn part_and_dyno_commit_together_and_late_stale_results_cannot_replace_a_new_setup() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let before = setup(&repo, account);
    let original = inventory::collection(&before, catalog().bindings()).unwrap();
    let pending = edit(&before, 12).with_dyno(dyno()).unwrap();
    let saved = commit(&repo, account, &catalog(), &pending, Timestamp(2)).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(saved.tables, before.tables);
    let mut expected = original.items[&10].clone();
    expected.children = vec![12];
    dyno().apply(&mut expected.derived).unwrap();
    let items = inventory::collection(&saved, catalog().bindings()).unwrap();
    assert_eq!(items.items[&10], expected);
    for id in [11, 12] {
        assert_eq!(items.items[&id], original.items[&id]);
    }
    assert_eq!(
        commit(&repo, account, &catalog(), &pending, Timestamp(3)).unwrap(),
        saved
    );
    let late = prepare_dyno(
        account,
        before.generation,
        &original,
        10,
        dyno(),
        &catalog(),
    )
    .unwrap();
    assert_eq!(
        commit(&repo, account, &catalog(), &late, Timestamp(4)),
        Err(Error::Conflict)
    );
    let new_stats =
        nfs_world_core::items::DynoStats::new([181, 401, 501, 121], [4., 9., 12.]).unwrap();
    let refreshed =
        prepare_dyno(account, saved.generation, &items, 10, new_stats, &catalog()).unwrap();
    assert_eq!(
        commit(&repo, who(2), &catalog(), &refreshed, Timestamp(5)),
        Err(Error::Ownership)
    );
    let refreshed = commit(&repo, account, &catalog(), &refreshed, Timestamp(5)).unwrap();
    let updated = inventory::collection(&refreshed, catalog().bindings()).unwrap();
    new_stats.apply(&mut expected.derived).unwrap();
    assert_eq!(updated.items[&10], expected);
    assert_eq!(refreshed.tables, before.tables);
}

#[test]
fn simultaneous_different_equipment_edits_cannot_overwrite_the_winner() {
    use std::sync::{Arc, Barrier};
    let repo = Arc::new(MemoryRepository::default());
    let account = who(1);
    let initial = setup(repo.as_ref(), account);
    let mut another = initial.items[&ItemId::new(12).unwrap()].clone();
    another.id = ItemId::new(13).unwrap();
    repo.apply(
        account,
        &Batch {
            id: 2,
            expected_generation: 1,
            ops: vec![Op::Insert(another)],
        },
        Timestamp(2),
    )
    .unwrap();
    let before = repo.snapshot(account).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [12, 13]
        .into_iter()
        .map(|part| {
            let repo = repo.clone();
            let barrier = barrier.clone();
            let pending = edit(&before, part);
            std::thread::spawn(move || {
                barrier.wait();
                commit(repo.as_ref(), account, &catalog(), &pending, Timestamp(3))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| matches!(
                e,
                Error::Conflict | Error::Store(nfs_storage::Error::Conflict)
            ))
    );
    let after = repo.snapshot(account).unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(after.tables, before.tables);
    assert_eq!(after.items.len(), before.items.len());
}
