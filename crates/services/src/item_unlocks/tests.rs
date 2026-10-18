// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::bank::Balance;
use nfs_storage::{MemoryRepository, SqliteRepository, tables::Table};
use nfs_world_core::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, Layout, OwnershipLevel, PurchaseRequest,
    PurchaseSelection,
};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn catalog() -> InventoryCatalog {
    let part = Definition {
        class: DefinitionClass::ForcedInductionItemData,
        buy_price: 19,
        sell_price: 9,
        quantity: 1,
        ownership_level: OwnershipLevel::Claimable,
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
        ownership_level: OwnershipLevel::Owned,
        default_derived: Some(Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap()),
        ..part.clone()
    };
    InventoryCatalog::new(
        [([1; 16], car), ([2; 16], part.clone()), ([3; 16], part)],
        vec![[1; 16]],
    )
    .unwrap()
}
fn policy() -> Policy {
    Policy::new(&catalog(), [([2; 16], 25), ([3; 16], 50)]).unwrap()
}
fn offers() -> purchase::Offers {
    purchase::Offers::new([[2; 16], [3; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog(), [([1; 16], vec![[2; 16], [3; 16]])])
        .unwrap()
        .with_part_policy(&catalog(), [], [([2; 16], 25), ([3; 16], 50)])
        .unwrap()
        .with_item_unlocks(policy())
        .unwrap()
}
fn score(repo: &dyn InventoryRepository, who: AccountId, value: i32) -> Snapshot {
    let before = inventory::load(repo, who, &catalog(), 10, Timestamp(1)).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 1000 + before.generation,
            expected_generation: before.generation,
            ops: vec![
                Op::SetTable(
                    key("RepValuesTable"),
                    Table {
                        rows: BTreeMap::from([(
                            0,
                            BTreeMap::from([
                                (key("RepScore"), Value::Int(value)),
                                (key("RepLevel"), Value::Int(999)),
                            ]),
                        )]),
                    },
                ),
                Balance(100).store(),
            ],
        },
        Timestamp(2),
    )
    .unwrap();
    repo.snapshot(who).unwrap()
}
fn reconcile(repo: &dyn InventoryRepository, who: AccountId, snapshot: Snapshot) -> Snapshot {
    policy()
        .ensure(repo, who, &catalog(), snapshot, Timestamp(3))
        .unwrap()
}
fn buy(repo: &dyn InventoryRepository, guid: [u8; 16]) -> purchase::Purchased {
    purchase::buy(
        repo,
        account(1),
        &catalog(),
        &offers(),
        &PurchaseRequest {
            sequence: 3,
            owner: 10,
            selection: PurchaseSelection::Definitions(vec![guid]),
        },
        Timestamp(4),
    )
    .unwrap()
}

#[test]
fn threshold_grants_are_additive_separate_from_purchase_and_use_shared_allocator() {
    let repo = MemoryRepository::default();
    let other = score(&repo, account(2), 100);
    let before = score(&repo, account(1), 24);
    assert_eq!(reconcile(&repo, account(1), before.clone()), before);
    let eligible = score(&repo, account(1), 25);
    let request = PurchaseRequest {
        sequence: 3,
        owner: 10,
        selection: PurchaseSelection::Definitions(vec![[2; 16]]),
    };
    assert!(
        purchase::buy(
            &repo,
            account(1),
            &catalog(),
            &offers(),
            &request,
            Timestamp(3)
        )
        .is_err()
    );
    let after = reconcile(&repo, account(1), eligible.clone());
    assert_eq!(after.items.len(), 2);
    let grant = after
        .items
        .values()
        .find(|i| i.state == items::PURCHASABLE)
        .unwrap();
    assert_eq!(
        (grant.owner, grant.buy_price, grant.sell_price),
        (None, 19, 9)
    );
    assert_eq!(
        after.items[&ItemId::new(10).unwrap()],
        eligible.items[&ItemId::new(10).unwrap()]
    );
    assert_eq!(Balance::from_snapshot(&after).unwrap(), Balance(100));
    assert_eq!(reconcile(&repo, account(1), eligible), after); // stale replay converges
    assert_eq!(repo.snapshot(account(2)).unwrap(), other);
    let selected_grant = PurchaseRequest {
        selection: PurchaseSelection::Items(vec![grant.id.get()]),
        ..request.clone()
    };
    let bought = purchase::buy(
        &repo,
        account(1),
        &catalog(),
        &offers(),
        &selected_grant,
        Timestamp(4),
    )
    .unwrap();
    assert_eq!(bought.balance, Balance(81));
    assert_eq!(bought.items.roots, [grant.id.get()]);
    assert_eq!(bought.items.items[&grant.id.get()].owner, 10);
    assert_eq!(bought.items.items[&grant.id.get()].state, items::OWNED);
    assert_eq!(bought.snapshot.items.len(), after.items.len() + 1);
    assert!(Policy::granted(&bought.snapshot, &[2; 16]));
    assert_eq!(
        reconcile(&repo, account(1), bought.snapshot.clone()),
        bought.snapshot
    );
    let wire = items::PurchaseReply {
        sequence: 3,
        purchased: Some(bought.items.clone()),
    }
    .encode(catalog().bindings())
    .unwrap();
    assert_eq!(
        items::PurchaseReply::decode(&wire, catalog().bindings())
            .unwrap()
            .purchased
            .unwrap()
            .roots,
        [grant.id.get()]
    );
    assert!(
        purchase::buy(
            &repo,
            account(1),
            &catalog(),
            &offers(),
            &selected_grant,
            Timestamp(5)
        )
        .unwrap()
        .replayed
    );
    assert!(
        purchase::buy(
            &repo,
            account(2),
            &catalog(),
            &offers(),
            &selected_grant,
            Timestamp(4)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(account(2)).unwrap(), other);
    let forged = PurchaseRequest {
        selection: PurchaseSelection::Items(vec![10]),
        ..request.clone()
    };
    assert!(
        purchase::buy(
            &repo,
            account(1),
            &catalog(),
            &offers(),
            &forged,
            Timestamp(4)
        )
        .is_err()
    );
    assert!(Policy::granted(&bought.snapshot, &[2; 16]));
    let eligible = score(&repo, account(1), 50);
    let after = reconcile(&repo, account(1), eligible);
    assert_eq!(after.items.len(), 4);
    // A refreshed grant must never receive a successful reply naming an old
    // owned ID: that violates the selected-ID contract and crashes the client.
    let new_grant = after
        .items
        .values()
        .find(|i| i.state == items::PURCHASABLE && i.definition.0 == [2; 16])
        .unwrap();
    assert!(
        purchase::buy(
            &repo,
            account(1),
            &catalog(),
            &offers(),
            &PurchaseRequest {
                selection: PurchaseSelection::Items(vec![new_grant.id.get()]),
                ..request
            },
            Timestamp(6)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), after);
    assert_eq!(buy(&repo, [3; 16]).items.roots, [14]);
    let reduced = score(&repo, account(1), 0);
    assert_eq!(reconcile(&repo, account(1), reduced.clone()), reduced);
    assert!(buy(&repo, [2; 16]).replayed);
    assert_eq!(repo.snapshot(account(1)).unwrap(), reduced);
}

#[test]
fn sqlite_restart_and_inventory_reply_preserve_exact_grants() {
    let path = std::env::temp_dir().join(format!(
        "afterhours-item-unlocks-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let saved = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let before = score(&repo, account(1), 50);
        reconcile(&repo, account(1), before)
    };
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    let request = items::Request {
        sequence: 9,
        operation: items::LOAD_INVENTORY,
    }
    .encode();
    let reply = inventory::answer_load_with_unlocks(
        &request,
        &repo,
        account(1),
        &catalog(),
        10,
        Timestamp(4),
        None,
        None,
        Some(&policy()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(reply.generation, saved.generation);
    assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
    let wire = items::Envelope::decode(&reply.bytes)
        .unwrap()
        .collection(catalog().bindings())
        .unwrap();
    assert_eq!(
        wire,
        inventory::collection(&saved, catalog().bindings()).unwrap()
    );
    let grant = saved
        .items
        .values()
        .find(|i| i.definition.0 == [2; 16])
        .unwrap();
    let purchase = PurchaseRequest {
        sequence: 3,
        owner: 10,
        selection: PurchaseSelection::Items(vec![grant.id.get()]),
    };
    let bought = purchase::buy(
        &repo,
        account(1),
        &catalog(),
        &offers(),
        &purchase,
        Timestamp(5),
    )
    .unwrap();
    assert_eq!(bought.balance, Balance(81));
    assert_eq!(bought.items.roots, [grant.id.get()]);
    drop(repo);
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    assert!(
        purchase::buy(
            &repo,
            account(1),
            &catalog(),
            &offers(),
            &purchase,
            Timestamp(6)
        )
        .unwrap()
        .replayed
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), bought.snapshot);
    drop(repo);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn competing_instance_purchases_keep_one_identity_and_charge() {
    let repo = MemoryRepository::default();
    let before = score(&repo, account(1), 50);
    let unlocked = reconcile(&repo, account(1), before);
    let grant = unlocked
        .items
        .values()
        .find(|i| i.definition.0 == [2; 16])
        .unwrap();
    let request = PurchaseRequest {
        sequence: 7,
        owner: 10,
        selection: PurchaseSelection::Items(vec![grant.id.get()]),
    };
    std::thread::scope(|scope| {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let (repo, request, barrier) = (&repo, &request, barrier.clone());
            handles.push(scope.spawn(move || {
                barrier.wait();
                purchase::buy(
                    repo,
                    account(1),
                    &catalog(),
                    &offers(),
                    request,
                    Timestamp(5),
                )
                .unwrap()
            }));
        }
        for handle in handles {
            let result = handle.join().unwrap();
            assert_eq!(result.items.roots, [grant.id.get()]);
            assert_eq!(result.balance, Balance(81));
        }
    });
    let saved = repo.snapshot(account(1)).unwrap();
    assert_eq!(saved.generation, unlocked.generation + 1);
    assert_eq!(saved.items.len(), unlocked.items.len() + 1);
    assert_eq!(
        saved
            .items
            .values()
            .filter(|i| i.definition.0 == [2; 16] && i.state == items::PURCHASABLE)
            .count(),
        1
    );
}

#[test]
fn concurrent_grants_converge_and_invalid_policies_fail_closed() {
    let repo = MemoryRepository::default();
    let before = score(&repo, account(1), 50);
    std::thread::scope(|scope| {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let before = before.clone();
                let repo = &repo;
                scope.spawn(move || {
                    barrier.wait();
                    reconcile(repo, account(1), before)
                })
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().items.len(), 3);
        }
    });
    assert_eq!(
        repo.snapshot(account(1)).unwrap().generation,
        before.generation + 1
    );
    for rows in [
        vec![([2; 16], 0)],
        vec![([2; 16], -1)],
        vec![([2; 16], 25), ([2; 16], 25)],
        vec![([1; 16], 25)],
        vec![([9; 16], 25)],
    ] {
        assert!(Policy::new(&catalog(), rows).is_err());
    }
    let bad = Policy::new(&catalog(), [([2; 16], 24)]).unwrap();
    assert!(offers().with_item_unlocks(bad).is_err());
}
