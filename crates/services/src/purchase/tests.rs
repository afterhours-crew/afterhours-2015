// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{Applied, MemoryRepository, SqliteRepository};
use nfs_world_core::items::{Definition, DefinitionFlags, Derived, Layout, OwnershipLevel};
use std::sync::{Arc, Barrier};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn catalog() -> InventoryCatalog {
    let car = Definition {
        class: DefinitionClass::RaceVehicleItemData,
        buy_price: 1000,
        sell_price: 500,
        quantity: 1,
        ownership_level: OwnershipLevel::Owned,
        flags: DefinitionFlags::default(),
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap()),
    };
    let part = Definition {
        class: DefinitionClass::AirFilterItemData,
        buy_price: 19,
        sell_price: 9,
        quantity: 1,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags {
            purchasable: true,
            requires_owner: true,
            ..DefinitionFlags::default()
        },
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::Empty),
    };
    InventoryCatalog::new(
        [([1; 16], car), ([2; 16], part.clone()), ([3; 16], part)],
        vec![[1; 16]],
    )
    .unwrap()
}
fn request() -> PurchaseRequest {
    PurchaseRequest {
        sequence: 3,
        owner: 10,
        selection: PurchaseSelection::Definitions(vec![[2; 16]]),
    }
}
fn initialize(repo: &dyn InventoryRepository, who: AccountId, amount: u32) {
    let snapshot = inventory::load(repo, who, &catalog(), 10, Timestamp(1)).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 20,
            expected_generation: snapshot.generation,
            ops: vec![Balance(amount).store()],
        },
        Timestamp(2),
    )
    .unwrap();
}
fn purchase(
    repo: &dyn InventoryRepository,
    who: AccountId,
    request: &PurchaseRequest,
) -> Result<Purchased, Error> {
    buy(
        repo,
        who,
        &catalog(),
        &Offers::new([[2; 16], [3; 16]]).unwrap(),
        request,
        Timestamp(3),
    )
}

#[test]
fn vehicle_scope_is_checked_before_money_receipts_or_inventory_change() {
    let repo = MemoryRepository::default();
    let who = account(1);
    initialize(&repo, who, 100);
    let before = repo.snapshot(who).unwrap();
    let catalog = catalog();
    let offers = Offers::new([[2; 16], [3; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [([1; 16], vec![[3; 16]])])
        .unwrap();
    assert!(!offers.allows(&[1; 16], &[2; 16]));
    assert!(!offers.allows(&[9; 16], &[3; 16]));
    assert_eq!(
        buy(&repo, who, &catalog, &offers, &request(), Timestamp(3)).unwrap_err(),
        Error::Rejected(Reason::Offer)
    );
    assert_eq!(repo.snapshot(who).unwrap(), before);
    let mut permitted = request();
    permitted.selection = PurchaseSelection::Definitions(vec![[3; 16]]);
    let first = buy(&repo, who, &catalog, &offers, &permitted, Timestamp(4)).unwrap();
    assert_eq!(first.balance, Balance(81));
    let repeated = buy(&repo, who, &catalog, &offers, &permitted, Timestamp(5)).unwrap();
    assert!(repeated.replayed);
    assert_eq!(first.snapshot, repeated.snapshot);
    for entries in [
        vec![([2; 16], vec![[3; 16]])], // a part cannot be the vehicle scope owner
        vec![([1; 16], vec![[9; 16]])], // unresolved definition
        vec![([1; 16], vec![[3; 16], [3; 16]])],
        vec![([1; 16], vec![]), ([1; 16], vec![])],
    ] {
        assert!(
            Offers::new([[2; 16]])
                .unwrap()
                .with_vehicle_scopes(&catalog, entries)
                .is_err()
        );
    }
    let none = Offers::new([[2; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [])
        .unwrap();
    assert!(!none.allows(&[1; 16], &[2; 16]));
}

#[test]
fn different_performance_classes_use_their_own_defaults_prices_and_receipts() {
    let repo = MemoryRepository::default();
    let who = account(1);
    initialize(&repo, who, 100);
    let before = repo.snapshot(who).unwrap();
    let base = catalog();
    let mut definitions: Vec<_> = base.definitions().map(|(g, d)| (*g, d.clone())).collect();
    let brake = Derived::BrakeDiscs {
        brake_strength_setting: 4,
        brake_bias_setting: 6,
    };
    let nos = Derived::NosTuning { nos_setting: 7 };
    for (guid, class, derived, price) in [
        (4, DefinitionClass::BrakesItemData, brake.clone(), 30),
        (5, DefinitionClass::NosTuningItemData, nos.clone(), 25),
    ] {
        definitions.push((
            [guid; 16],
            Definition {
                class,
                default_derived: Some(derived),
                buy_price: price,
                ..base.definition(&[2; 16]).unwrap().clone()
            },
        ));
    }
    let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
    let offers = Offers::new([[4; 16], [5; 16]]).unwrap();
    for (guid, derived, balance) in [(4, brake, 70), (5, nos, 45)] {
        let request = PurchaseRequest {
            selection: PurchaseSelection::Definitions(vec![[guid; 16]]),
            ..request()
        };
        let result = buy(&repo, who, &catalog, &offers, &request, Timestamp(3)).unwrap();
        let item = &result.items.items[&result.items.roots[0]];
        assert_eq!(item.derived, derived);
        assert_eq!(item.owner, 10);
        assert_eq!(result.balance, Balance(balance));
        assert_eq!(
            result.snapshot.items[&ItemId::new(10).unwrap()],
            before.items[&ItemId::new(10).unwrap()]
        );
        let retry = buy(&repo, who, &catalog, &offers, &request, Timestamp(4)).unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.items, result.items);
        assert_eq!(retry.snapshot, result.snapshot);
    }
    assert_eq!(
        repo.snapshot(who).unwrap().generation,
        before.generation + 2
    );
}

#[test]
fn purchase_and_receipt_survive_sqlite_restart_and_do_not_equip_implicitly() {
    let directory = std::env::temp_dir().join(format!(
        "afterhours-purchase-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let who = account(1);
    {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        initialize(&repo, who, 50);
        initialize(&repo, account(2), 0);
        let before = repo.snapshot(who).unwrap();
        let bought = purchase(&repo, who, &request()).unwrap();
        assert!(!bought.replayed);
        assert_eq!(bought.balance, Balance(31));
        assert_eq!(bought.items.roots, [11]);
        assert_eq!(bought.items.items[&11].owner, 10);
        assert_eq!(
            bought.snapshot.items[&ItemId::new(10).unwrap()],
            before.items[&ItemId::new(10).unwrap()]
        );
        assert_eq!(repo.snapshot(who).unwrap(), bought.snapshot);
        assert_eq!(
            purchase(&repo, account(2), &request()).unwrap_err(),
            Error::Rejected(Reason::Funds)
        );
        assert_eq!(repo.snapshot(account(2)).unwrap().items.len(), 1);
    }
    {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        let before = repo.open(who).unwrap();
        let retry = purchase(
            &repo,
            who,
            &PurchaseRequest {
                sequence: 40,
                ..request()
            },
        )
        .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.items.roots, [11]);
        assert_eq!(retry.balance, Balance(31));
        assert_eq!(repo.snapshot(who).unwrap(), before);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejected_input_and_failed_commit_leave_cash_inventory_and_receipts_unchanged() {
    let repo = MemoryRepository::default();
    let who = account(3);
    initialize(&repo, who, 18);
    let before = repo.snapshot(who).unwrap();
    for (request, reason) in [
        (request(), Reason::Funds),
        (
            PurchaseRequest {
                owner: 99,
                ..request()
            },
            Reason::Owner,
        ),
        (
            PurchaseRequest {
                sequence: 1,
                ..request()
            },
            Reason::Shape,
        ),
        (
            PurchaseRequest {
                selection: PurchaseSelection::Items(vec![10]),
                ..request()
            },
            Reason::Shape,
        ),
        (
            PurchaseRequest {
                selection: PurchaseSelection::Definitions(vec![[2; 16], [2; 16]]),
                ..request()
            },
            Reason::Shape,
        ),
        (
            PurchaseRequest {
                selection: PurchaseSelection::Definitions(vec![[9; 16]]),
                ..request()
            },
            Reason::Offer,
        ),
    ] {
        assert_eq!(
            purchase(&repo, who, &request).unwrap_err(),
            Error::Rejected(reason)
        );
        assert_eq!(repo.snapshot(who).unwrap(), before);
    }
    struct Refuses(MemoryRepository);
    impl InventoryRepository for Refuses {
        fn open(&self, a: AccountId) -> Result<Snapshot, nfs_storage::Error> {
            self.0.open(a)
        }
        fn snapshot(&self, a: AccountId) -> Result<Snapshot, nfs_storage::Error> {
            self.0.snapshot(a)
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
    initialize(&repo.0, who, 50);
    let before = repo.snapshot(who).unwrap();
    assert_eq!(
        purchase(&repo, who, &request()).unwrap_err(),
        Error::Storage(nfs_storage::Error::Storage)
    );
    assert_eq!(repo.snapshot(who).unwrap(), before);
}

#[test]
fn concurrent_purchases_recompute_after_stale_generation_and_charge_each_offer_once() {
    let repo = Arc::new(MemoryRepository::default());
    let who = account(4);
    initialize(repo.as_ref(), who, 50);
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let repo = repo.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                purchase(
                    repo.as_ref(),
                    who,
                    &PurchaseRequest {
                        selection: PurchaseSelection::Definitions(vec![[2 + i % 2; 16]]),
                        ..request()
                    },
                )
                .unwrap()
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let after = repo.snapshot(who).unwrap();
    assert_eq!(Balance::from_snapshot(&after), Ok(Balance(12)));
    assert_eq!(after.items.len(), 3);
    assert_eq!(after.generation, 4);
    assert_eq!(ledger(&after).unwrap().1, 13);
}

#[test]
fn a_missing_receipted_item_fails_closed_instead_of_rebuying() {
    let repo = MemoryRepository::default();
    let who = account(5);
    initialize(&repo, who, 50);
    let bought = purchase(&repo, who, &request()).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 21,
            expected_generation: bought.snapshot.generation,
            ops: vec![Op::Remove(ItemId::new(11).unwrap())],
        },
        Timestamp(4),
    )
    .unwrap();
    let before = repo.snapshot(who).unwrap();
    assert_eq!(
        purchase(&repo, who, &request()).unwrap_err(),
        Error::Storage(nfs_storage::Error::Config)
    );
    assert_eq!(repo.snapshot(who).unwrap(), before);
}
