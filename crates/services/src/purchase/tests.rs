// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{Applied, MemoryRepository, SqliteRepository};
use nfs_world_core::items::{Definition, DefinitionFlags, Layout, OwnershipLevel};
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
