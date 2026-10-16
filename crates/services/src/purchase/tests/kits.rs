// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn kit_catalog() -> InventoryCatalog {
    let base = catalog();
    let car = base.definition(&[1; 16]).unwrap().clone();
    let mut hood = base.definition(&[2; 16]).unwrap().clone();
    hood.class = DefinitionClass::HoodItemData;
    let mut splitter = hood.clone();
    splitter.class = DefinitionClass::SplitterItemData;
    let kit = Definition {
        class: DefinitionClass::BodyKitItemData,
        buy_price: 40,
        sub_items: vec![[2; 16], [3; 16]],
        ..hood.clone()
    };
    InventoryCatalog::new(
        [
            ([1; 16], car),
            ([2; 16], hood),
            ([3; 16], splitter),
            ([4; 16], kit),
        ],
        vec![[1; 16]],
    )
    .unwrap()
}
fn kit_offers(c: &InventoryCatalog) -> Offers {
    Offers::new([[2; 16], [3; 16], [4; 16]])
        .unwrap()
        .with_vehicle_scopes(c, [([1; 16], vec![[2; 16], [3; 16], [4; 16]])])
        .unwrap()
        .with_part_policy(c, [], [([2; 16], 0), ([3; 16], 0), ([4; 16], 0)])
        .unwrap()
        .with_fitment_restrictions(c, [])
        .unwrap()
}
fn funded(repo: &dyn InventoryRepository, who: AccountId, amount: u32) -> Snapshot {
    let before = inventory::load(repo, who, &kit_catalog(), 10, Timestamp(1)).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 20,
            expected_generation: before.generation,
            ops: vec![Balance(amount).store()],
        },
        Timestamp(2),
    )
    .unwrap();
    repo.snapshot(who).unwrap()
}
fn kit_request() -> PurchaseRequest {
    PurchaseRequest {
        selection: PurchaseSelection::Definitions(vec![[4; 16]]),
        ..request()
    }
}

#[test]
fn kit_purchase_is_atomic_charges_bundle_only_and_reopens_without_duplicate_children() {
    let directory = std::env::temp_dir().join(format!(
        "nfs-kit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let c = kit_catalog();
    let offers = kit_offers(&c).with_body_kits(&c, [[4; 16]]).unwrap();
    let saved = {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        let before = funded(&repo, account(1), 100);
        let other = funded(&repo, account(2), 39);
        assert_eq!(
            buy(&repo, account(2), &c, &offers, &kit_request(), Timestamp(3)).unwrap_err(),
            Error::Rejected(Reason::Funds)
        );
        assert_eq!(repo.snapshot(account(2)).unwrap(), other);
        let result = buy(&repo, account(1), &c, &offers, &kit_request(), Timestamp(3)).unwrap();
        assert_eq!(result.balance, Balance(60));
        assert_eq!(result.snapshot.generation, before.generation + 1);
        assert_eq!(result.items.roots, [11, 12, 13]);
        assert_eq!(result.items.items.len(), 3);
        let kit = &result.items.items[&11];
        assert_eq!(kit.owner, 10);
        assert_eq!(kit.defaults, [12, 13]);
        assert_eq!(kit.children, kit.defaults);
        for id in [12, 13] {
            assert_eq!(result.items.items[&id].owner, 11);
            assert_eq!(result.items.items[&id].buy_price, 19);
        }
        assert_eq!(
            result.snapshot.items[&ItemId::new(10).unwrap()],
            before.items[&ItemId::new(10).unwrap()]
        );
        let encoded = result.items.encode(c.bindings()).unwrap();
        assert_eq!(
            Collection::decode(&encoded, c.bindings()).unwrap(),
            result.items
        );
        result.snapshot
    };
    {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account(1)).unwrap(), saved);
        for selection in [
            PurchaseSelection::Definitions(vec![[4; 16]]),
            PurchaseSelection::Items(vec![11]),
        ] {
            let again = buy(
                &repo,
                account(1),
                &c,
                &offers,
                &PurchaseRequest {
                    sequence: 55,
                    selection,
                    ..request()
                },
                Timestamp(4),
            )
            .unwrap();
            assert!(again.replayed);
            assert_eq!(again.items.roots, [11, 12, 13]);
            assert_eq!(again.snapshot, saved);
            assert_eq!(again.items.items.len(), 3);
        }
        let leaf = buy(&repo, account(1), &c, &offers, &request(), Timestamp(5)).unwrap();
        assert_eq!(leaf.items.roots, [14]);
        assert_eq!(leaf.balance, Balance(41));
        assert_eq!(leaf.snapshot.items.len(), 5);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn kit_offers_require_explicit_opt_in_and_reject_unsupported_or_duplicate_children() {
    let c = kit_catalog();
    let repo = MemoryRepository::default();
    let before = funded(&repo, account(1), 100);
    let offers = kit_offers(&c);
    assert_eq!(
        buy(&repo, account(1), &c, &offers, &kit_request(), Timestamp(3)).unwrap_err(),
        Error::Rejected(Reason::Offer)
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), before);
    for mutation in 0..5 {
        let mut definitions: BTreeMap<_, _> =
            c.definitions().map(|(g, d)| (*g, d.clone())).collect();
        match mutation {
            0 => definitions.get_mut(&[4; 16]).unwrap().sub_items = vec![[2; 16], [2; 16]],
            1 => definitions
                .get_mut(&[4; 16])
                .unwrap()
                .additional_items
                .push([2; 16]),
            2 => definitions.get_mut(&[2; 16]).unwrap().flags.requires_owner = false,
            3 => definitions.get_mut(&[2; 16]).unwrap().class = DefinitionClass::AirFilterItemData,
            _ => definitions.get_mut(&[4; 16]).unwrap().sub_items.clear(),
        }
        let bad = InventoryCatalog::new(definitions, vec![[1; 16]]).unwrap();
        assert!(
            kit_offers(&bad).with_body_kits(&bad, [[4; 16]]).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn concurrent_kit_requests_allocate_one_owned_subtree_and_one_charge() {
    let repo = Arc::new(MemoryRepository::default());
    let before = funded(repo.as_ref(), account(1), 100);
    let barrier = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let repo = repo.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let c = kit_catalog();
                let offers = kit_offers(&c).with_body_kits(&c, [[4; 16]]).unwrap();
                barrier.wait();
                buy(
                    repo.as_ref(),
                    account(1),
                    &c,
                    &offers,
                    &kit_request(),
                    Timestamp(3),
                )
                .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| !r.replayed).count(), 1);
    let after = repo.snapshot(account(1)).unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(after.items.len(), 4);
    assert_eq!(Balance::from_snapshot(&after).unwrap(), Balance(60));
    for result in results {
        assert_eq!(result.snapshot, after);
    }
}
