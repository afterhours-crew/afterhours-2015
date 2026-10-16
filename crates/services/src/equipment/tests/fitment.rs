// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn fixture(
    repo: &dyn InventoryRepository,
    account: AccountId,
) -> (InventoryCatalog, Offers, Snapshot) {
    let before = setup(repo, account);
    let mut definitions = catalog()
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect::<Vec<_>>();
    let mut blocker = definitions[1].1.clone();
    blocker.class = DefinitionClass::RadiatorItemData;
    definitions.push(([4; 16], blocker.clone()));
    definitions.push(([5; 16], blocker));
    let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
    let offers = Offers::new([[3; 16], [5; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [([1; 16], vec![[3; 16], [5; 16]])])
        .unwrap()
        .with_part_policy(&catalog, [], [([3; 16], 0), ([5; 16], 0)])
        .unwrap()
        .with_fitment_restrictions(&catalog, [([3; 16], vec![[4; 16]])])
        .unwrap();
    let id = |n| ItemId::new(n).unwrap();
    let mut car = before.items[&id(10)].clone();
    car.sub_items.push(id(13));
    car.default_items.push(id(13));
    let stock = before.items[&id(11)].clone();
    let result = repo
        .apply(
            account,
            &Batch {
                id: 2,
                expected_generation: before.generation,
                ops: vec![
                    Op::Update(car),
                    Op::Insert(ItemRecord {
                        id: id(13),
                        definition: DefinitionGuid([4; 16]),
                        ..stock.clone()
                    }),
                    Op::Insert(ItemRecord {
                        id: id(14),
                        definition: DefinitionGuid([5; 16]),
                        ..stock
                    }),
                ],
            },
            Timestamp(2),
        )
        .unwrap();
    assert!(!result.replayed);
    (catalog, offers, repo.snapshot(account).unwrap())
}

#[test]
fn fitment_conflicts_use_staged_equipment_and_ignore_owned_spares() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let (catalog, offers, before) = fixture(&repo, account);
    let (_, _, other) = fixture(&repo, who(2));
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    assert_eq!(
        prepare(
            account,
            before.generation,
            &items,
            10,
            12,
            &catalog,
            &offers
        ),
        Err(Error::Unsupported)
    );
    assert_eq!(repo.snapshot(account).unwrap(), before);
    let edit = prepare(
        account,
        before.generation,
        &items,
        10,
        14,
        &catalog,
        &offers,
    )
    .unwrap()
    .with_part(&items, 12, &catalog, &offers)
    .unwrap();
    let preview = edit.preview(&items).unwrap();
    assert_eq!(preview.items[&10].children, [14, 12]);
    assert!(preview.items.contains_key(&13));
    assert!(preview.items[&10].defaults.contains(&13));
    // Canceling this preview leaves both the equipment and generation intact.
    assert_eq!(repo.snapshot(account).unwrap(), before);
    let saved = commit(&repo, account, &catalog, &edit, Timestamp(3)).unwrap();
    assert_eq!(
        commit(&repo, account, &catalog, &edit, Timestamp(4)).unwrap(),
        saved
    );
    assert_eq!(repo.snapshot(who(2)).unwrap(), other);
    assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(70));
    // An edit based on the earlier conflicting state cannot overwrite this save.
    let stale = prepare(
        account,
        before.generation,
        &items,
        10,
        11,
        &catalog,
        &offers,
    )
    .unwrap();
    assert!(commit(&repo, account, &catalog, &stale, Timestamp(5)).is_err());
}

#[test]
fn fitment_checks_nested_installed_parts_and_bounds_malformed_trees() {
    let repo = MemoryRepository::default();
    let (catalog, offers, before) = fixture(&repo, who(1));
    let mut items = inventory::collection(&before, catalog.bindings()).unwrap();
    items.items.get_mut(&10).unwrap().children = vec![14];
    items.items.get_mut(&14).unwrap().children = vec![13];
    assert!(!offers.fits(&items, 10, &[3; 16], &catalog).unwrap());
    items.items.get_mut(&14).unwrap().children = vec![14];
    assert!(offers.fits(&items, 10, &[3; 16], &catalog).is_err());
    items.items.get_mut(&14).unwrap().children = vec![99];
    assert!(offers.fits(&items, 10, &[3; 16], &catalog).is_err());
    // Same-slot replacement removes that old definition from the final fitment.
    let offers = offers
        .with_fitment_restrictions(&catalog, [([3; 16], vec![[2; 16]])])
        .unwrap();
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    assert!(offers.fits(&items, 10, &[3; 16], &catalog).unwrap());
}

#[test]
fn canard_and_caliper_conflicts_do_not_ignore_the_opposite_axle() {
    for class in [
        DefinitionClass::CanardItemData,
        DefinitionClass::CalipersItemData,
    ] {
        let repo = MemoryRepository::default();
        let account = who(1);
        let (original, _, before) = fixture(&repo, account);
        let definitions = original.definitions().map(|(guid, definition)| {
            let mut definition = definition.clone();
            if *guid != [1; 16] {
                definition.class = class;
            }
            (*guid, definition)
        });
        let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
        let offers = Offers::new([[3; 16], [5; 16]])
            .unwrap()
            .with_vehicle_scopes(&catalog, [([1; 16], vec![[3; 16], [5; 16]])])
            .unwrap()
            .with_part_policy(
                &catalog,
                [([2; 16], 0), ([3; 16], 0), ([4; 16], 1), ([5; 16], 1)],
                [([3; 16], 0), ([5; 16], 0)],
            )
            .unwrap()
            .with_fitment_restrictions(&catalog, [([3; 16], vec![[4; 16]])])
            .unwrap();
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        assert_eq!(
            prepare(
                account,
                before.generation,
                &items,
                10,
                12,
                &catalog,
                &offers
            ),
            Err(Error::Unsupported)
        );
        assert_eq!(repo.snapshot(account).unwrap(), before);
        let edit = prepare(
            account,
            before.generation,
            &items,
            10,
            14,
            &catalog,
            &offers,
        )
        .unwrap();
        assert_eq!(edit.preview(&items).unwrap().items[&10].children, [11, 14]);
        let edit = edit.with_part(&items, 12, &catalog, &offers).unwrap();
        assert_eq!(edit.preview(&items).unwrap().items[&10].children, [14, 12]);
        let saved = commit(&repo, account, &catalog, &edit, Timestamp(3)).unwrap();
        assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(70));
    }
}

#[test]
fn restricted_purchase_keeps_spare_until_it_fits_and_survives_reopen_and_retry() {
    use nfs_world_core::items::{PurchaseRequest, PurchaseSelection};
    let directory = std::env::temp_dir().join(format!(
        "nfs-fitment-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let account = who(1);
    let request = PurchaseRequest {
        sequence: 3,
        owner: 10,
        selection: PurchaseSelection::Definitions(vec![[6; 16]]),
    };
    let (catalog, offers, saved, pending, purchased_id) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let (original, _, before) = fixture(&repo, account);
        let mut definitions = original
            .definitions()
            .map(|(g, d)| (*g, d.clone()))
            .collect::<Vec<_>>();
        let mut new_part = original.definition(&[3; 16]).unwrap().clone();
        new_part.flags.purchasable = true;
        definitions.push(([6; 16], new_part));
        let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
        let offers = Offers::new([[3; 16], [5; 16], [6; 16]])
            .unwrap()
            .with_vehicle_scopes(&catalog, [([1; 16], vec![[3; 16], [5; 16], [6; 16]])])
            .unwrap()
            .with_part_policy(&catalog, [], [([3; 16], 0), ([5; 16], 0), ([6; 16], 0)])
            .unwrap()
            .with_fitment_restrictions(&catalog, [([6; 16], vec![[4; 16]])])
            .unwrap();
        let purchase =
            crate::purchase::buy(&repo, account, &catalog, &offers, &request, Timestamp(3))
                .unwrap();
        let purchased_id = purchase.items.roots[0];
        assert_eq!(purchase.balance, Balance(40));
        assert_eq!(
            purchase.snapshot.items[&ItemId::new(10).unwrap()],
            before.items[&ItemId::new(10).unwrap()]
        );
        let items = inventory::collection(&purchase.snapshot, catalog.bindings()).unwrap();
        assert_eq!(
            prepare(
                account,
                purchase.snapshot.generation,
                &items,
                10,
                purchased_id,
                &catalog,
                &offers
            ),
            Err(Error::Unsupported)
        );
        assert_eq!(repo.snapshot(account).unwrap(), purchase.snapshot);
        let retry = crate::purchase::buy(&repo, account, &catalog, &offers, &request, Timestamp(4))
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.snapshot, purchase.snapshot);
        let clear = prepare(
            account,
            purchase.snapshot.generation,
            &items,
            10,
            14,
            &catalog,
            &offers,
        )
        .unwrap();
        let cleared = commit(&repo, account, &catalog, &clear, Timestamp(5)).unwrap();
        let items = inventory::collection(&cleared, catalog.bindings()).unwrap();
        let pending = prepare(
            account,
            cleared.generation,
            &items,
            10,
            purchased_id,
            &catalog,
            &offers,
        )
        .unwrap();
        let saved = commit(&repo, account, &catalog, &pending, Timestamp(6)).unwrap();
        assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(40));
        (catalog, offers, saved, pending, purchased_id)
    };
    {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), saved);
        assert_eq!(
            saved.items[&ItemId::new(10).unwrap()].sub_items,
            [ItemId::new(14).unwrap(), ItemId::new(purchased_id).unwrap()]
        );
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(7)).unwrap(),
            saved
        );
        let replay =
            crate::purchase::buy(&repo, account, &catalog, &offers, &request, Timestamp(8))
                .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.snapshot, saved);
        // Completed receipts still replay if later equipment conflicts.
        let items = inventory::collection(&saved, catalog.bindings()).unwrap();
        let conflict =
            prepare(account, saved.generation, &items, 10, 13, &catalog, &offers).unwrap();
        let latest = commit(&repo, account, &catalog, &conflict, Timestamp(9)).unwrap();
        let replay =
            crate::purchase::buy(&repo, account, &catalog, &offers, &request, Timestamp(10))
                .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.snapshot, latest);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
