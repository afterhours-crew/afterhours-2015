// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{bank::Balance, inventory};
use nfs_storage::{
    AccountId, Batch, GarageSlots, InventoryRepository, ItemId, MemoryRepository, Op, Snapshot,
    SqliteRepository, Timestamp,
};
use nfs_world_core::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, InventoryCatalog, Layout,
    OwnershipLevel, PurchaseRequest, PurchaseSelection, SellRequest,
};
use std::sync::{Arc, Barrier};
mod warehouse;
fn who(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn id(n: u64) -> ItemId {
    ItemId::new(n).unwrap()
}
fn catalog() -> InventoryCatalog {
    let car = Definition {
        class: DefinitionClass::RaceVehicleItemData,
        buy_price: 100,
        sell_price: 65,
        quantity: u32::MAX,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags {
            purchasable: true,
            uses_scope: true,
            ..Default::default()
        },
        sub_items: vec![[2; 16]],
        additional_items: vec![[3; 16]],
        default_derived: Some(Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap()),
    };
    let part = Definition {
        class: DefinitionClass::AirFilterItemData,
        buy_price: 20,
        sell_price: 10,
        quantity: 1,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags {
            requires_owner: true,
            purchasable: true,
            ..Default::default()
        },
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::Empty),
    };
    let wrap = Definition {
        class: DefinitionClass::LiveryCustomizationItemData,
        flags: DefinitionFlags::default(),
        default_derived: Some(Derived::LiveryCustomization {
            byte_vault_data_id: 0,
            library_preset_id: 0,
            dynamic_flags: 0,
        }),
        ..part.clone()
    };
    InventoryCatalog::new(
        [
            ([1; 16], car),
            ([2; 16], part.clone()),
            ([3; 16], wrap),
            ([4; 16], part),
        ],
        vec![[1; 16]],
    )
    .unwrap()
}
fn init(repo: &dyn InventoryRepository, account: AccountId) -> u64 {
    repo.open(account).unwrap();
    let c = catalog();
    let initial = c.instantiate_initial(10).unwrap();
    let mut ops = inventory::records(&initial.collection, c.bindings())
        .unwrap()
        .into_iter()
        .map(Op::Insert)
        .collect::<Vec<_>>();
    ops.extend([
        Op::SetGarage(GarageSlots([Some(id(10)), None, None, None, None])),
        Balance(1000).store(),
    ]);
    repo.apply(
        account,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops,
        },
        Timestamp(1),
    )
    .unwrap();
    open_session(repo, account, Timestamp(2)).unwrap()
}
fn purchase(
    repo: &dyn InventoryRepository,
    account: AccountId,
    session: u64,
    sequence: u32,
) -> Result<Changed, Error> {
    let c = catalog();
    buy(
        repo,
        account,
        &c,
        &Offers::new(&c, [([1; 16], 0)]).unwrap(),
        CommandId { session, sequence },
        &PurchaseRequest {
            sequence,
            owner: 0,
            selection: PurchaseSelection::Definitions(vec![[1; 16]]),
        },
        Timestamp(3),
    )
}
fn car(result: &Changed) -> u64 {
    result
        .items
        .items
        .values()
        .find(|i| i.owner == 0 && i.definition == [1; 16])
        .unwrap()
        .id
}
fn sale(
    repo: &dyn InventoryRepository,
    account: AccountId,
    session: u64,
    sequence: u32,
    item: u64,
) -> Result<Changed, Error> {
    sell(
        repo,
        account,
        &catalog(),
        CommandId { session, sequence },
        &SellRequest {
            sequence,
            items: vec![item],
        },
        Timestamp(4),
    )
}
#[test]
fn duplicate_definition_is_a_new_car_but_retry_is_not_a_second_debit() {
    let repo = MemoryRepository::default();
    let session = init(&repo, who(1));
    init(&repo, who(2));
    let other = repo.snapshot(who(2)).unwrap();
    let first = purchase(&repo, who(1), session, 2).unwrap();
    let a = car(&first);
    let retry = purchase(&repo, who(1), session, 2).unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.items, first.items);
    assert_eq!(retry.snapshot, first.snapshot);
    let second = purchase(&repo, who(1), session, 3).unwrap();
    let b = car(&second);
    assert_ne!(a, b);
    assert_eq!(Balance::from_snapshot(&second.snapshot), Ok(Balance(800)));
    assert_eq!(
        second.snapshot.garage.unwrap().0,
        [Some(id(10)), Some(id(a)), Some(id(b)), None, None]
    );
    let cars = inventory::collection(&second.snapshot, catalog().bindings()).unwrap();
    assert_ne!(cars.items[&a].children, cars.items[&b].children);
    for (car, child) in [
        (a, cars.items[&a].children[0]),
        (b, cars.items[&b].children[0]),
    ] {
        assert_eq!(cars.items[&child].owner, car);
    }
    assert_eq!(
        second
            .snapshot
            .items
            .values()
            .filter(|r| r.definition.0 == [3; 16])
            .count(),
        1
    );
    assert_eq!(
        purchase(&repo, who(1), session, 2).unwrap_err(),
        Error::Rejected(Reason::Expired)
    );
    assert_eq!(repo.snapshot(who(2)).unwrap(), other);
}
#[test]
fn five_slots_funds_and_quantity_fail_without_partial_changes() {
    let repo = MemoryRepository::default();
    let session = init(&repo, who(1));
    for sequence in 2..6 {
        purchase(&repo, who(1), session, sequence).unwrap();
    }
    let full = repo.snapshot(who(1)).unwrap();
    assert_eq!(
        purchase(&repo, who(1), session, 6).unwrap_err(),
        Error::Rejected(Reason::Full)
    );
    assert_eq!(repo.snapshot(who(1)).unwrap(), full);
    let poor = MemoryRepository::default();
    let session = init(&poor, who(1));
    let snapshot = poor.snapshot(who(1)).unwrap();
    poor.apply(
        who(1),
        &Batch {
            id: 33,
            expected_generation: snapshot.generation,
            ops: vec![Balance(99).store()],
        },
        Timestamp(5),
    )
    .unwrap();
    let before = poor.snapshot(who(1)).unwrap();
    assert_eq!(
        purchase(&poor, who(1), session, 2).unwrap_err(),
        Error::Rejected(Reason::Funds)
    );
    assert_eq!(poor.snapshot(who(1)).unwrap(), before);
    let source = catalog();
    let definitions = source.definitions().map(|(g, d)| {
        let mut d = d.clone();
        if *g == [1; 16] {
            d.quantity = 1;
        }
        (*g, d)
    });
    let c = InventoryCatalog::new(definitions, vec![]).unwrap();
    let limited = MemoryRepository::default();
    init(&limited, who(1));
    let before = limited.snapshot(who(1)).unwrap();
    assert_eq!(
        buy(
            &limited,
            who(1),
            &c,
            &Offers::new(&c, [([1; 16], 0)]).unwrap(),
            CommandId {
                session: 1,
                sequence: 9
            },
            &PurchaseRequest {
                sequence: 9,
                owner: 0,
                selection: PurchaseSelection::Definitions(vec![[1; 16]])
            },
            Timestamp(9)
        )
        .unwrap_err(),
        Error::Rejected(Reason::Quantity)
    );
    assert_eq!(limited.snapshot(who(1)).unwrap(), before);
}
#[test]
fn selection_swaps_items_and_is_idempotent_without_editing_either_car() {
    let repo = MemoryRepository::default();
    let session = init(&repo, who(1));
    let bought = purchase(&repo, who(1), session, 2).unwrap();
    let new = car(&bought);
    let placed = place(&repo, who(1), &catalog(), new, 3, Timestamp(4)).unwrap();
    assert_eq!(
        placed.garage.unwrap().0,
        [Some(id(10)), None, None, None, Some(id(new))]
    );
    let selected = select(&repo, who(1), &catalog(), new, 10, Timestamp(5)).unwrap();
    assert_eq!(
        selected.garage.unwrap().0,
        [Some(id(new)), None, None, None, Some(id(10))]
    );
    assert_eq!(
        select(&repo, who(1), &catalog(), new, 10, Timestamp(6)).unwrap(),
        selected
    );
    assert_eq!(selected.items, bought.snapshot.items);
    assert_eq!(
        sale(&repo, who(1), session, 3, new).unwrap_err(),
        Error::Rejected(Reason::Primary)
    );
    assert_eq!(
        select(&repo, who(1), &catalog(), 11, new, Timestamp(6)).unwrap_err(),
        Error::Rejected(Reason::Owner)
    );
    assert_eq!(repo.snapshot(who(1)).unwrap(), selected);
}
#[test]
fn sale_removes_installed_and_spare_descendants_but_keeps_account_wraps() {
    let repo = MemoryRepository::default();
    let session = init(&repo, who(1));
    let bought = purchase(&repo, who(1), session, 2).unwrap();
    let new = car(&bought);
    let c = catalog();
    let part_request = PurchaseRequest {
        sequence: 4,
        owner: new,
        selection: PurchaseSelection::Definitions(vec![[4; 16]]),
    };
    let parts = crate::purchase::buy(
        &repo,
        who(1),
        &c,
        &crate::purchase::Offers::new([[4; 16]]).unwrap(),
        &part_request,
        Timestamp(5),
    )
    .unwrap();
    let spare = parts.items.roots[0];
    assert!(parts.snapshot.items.contains_key(&id(spare)));
    let sold = sale(&repo, who(1), session, 3, new).unwrap();
    assert!(sold.items.items.contains_key(&spare));
    assert!(sold.items.items.contains_key(&new));
    assert_eq!(sold.items.items.len(), 3);
    assert_eq!(Balance::from_snapshot(&sold.snapshot), Ok(Balance(945)));
    assert_eq!(
        sold.snapshot.garage.unwrap().0,
        [Some(id(10)), None, None, None, None]
    );
    assert!(
        sold.snapshot
            .items
            .values()
            .any(|r| r.definition.0 == [3; 16] && r.owner.is_none())
    );
    let retry = sale(&repo, who(1), session, 3, new).unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.items, sold.items);
    assert_eq!(retry.snapshot, sold.snapshot);
    assert_eq!(
        sale(&repo, who(1), session, 4, new).unwrap_err(),
        Error::Rejected(Reason::Owner)
    );
    let rebought = purchase(&repo, who(1), session, 5).unwrap();
    assert!(car(&rebought) > spare);
}
#[test]
fn concurrent_identical_purchase_has_one_car_and_one_charge() {
    let repo = Arc::new(MemoryRepository::default());
    let session = init(repo.as_ref(), who(1));
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let repo = repo.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                purchase(repo.as_ref(), who(1), session, 2).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let mut results = threads.into_iter().map(|t| t.join().unwrap());
    let a = results.next().unwrap();
    let b = results.next().unwrap();
    assert_eq!(a.items, b.items);
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(
        Balance::from_snapshot(&repo.snapshot(who(1)).unwrap()),
        Ok(Balance(900))
    );
}
#[test]
fn sessions_and_resale_receipts_survive_repository_restart() {
    let directory = std::env::temp_dir().join(format!(
        "afterhours-cars-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let (session, new, selected): (_, _, Snapshot) = {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        let session = init(&repo, who(1));
        let bought = purchase(&repo, who(1), session, 2).unwrap();
        let new = car(&bought);
        let selected = select(&repo, who(1), &catalog(), new, 10, Timestamp(5)).unwrap();
        (session, new, selected)
    };
    {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(who(1)).unwrap(), selected);
        assert!(purchase(&repo, who(1), session, 2).unwrap().replayed);
        let next = open_session(&repo, who(1), Timestamp(6)).unwrap();
        assert!(next > session);
        let sold = sale(&repo, who(1), next, 2, 10).unwrap();
        assert!(!sold.snapshot.items.contains_key(&id(10)));
        assert_eq!(sold.snapshot.garage.unwrap().0[0], Some(id(new)));
    }
    {
        let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
        repo.open(who(1)).unwrap();
        assert!(sale(&repo, who(1), session + 1, 2, 10).unwrap().replayed);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn superseded_or_evicted_commands_cannot_charge_again() {
    let repo = MemoryRepository::default();
    let session = init(&repo, who(1));
    let first = purchase(&repo, who(1), session, 2).unwrap();
    purchase(&repo, who(1), session, 3).unwrap();
    let before = repo.snapshot(who(1)).unwrap();
    assert_eq!(
        purchase(&repo, who(1), session, 2).unwrap_err(),
        Error::Rejected(Reason::Expired)
    );
    assert_eq!(
        sale(&repo, who(1), session, 3, car(&first)).unwrap_err(),
        Error::Rejected(Reason::Shape)
    );
    assert_eq!(repo.snapshot(who(1)).unwrap(), before);
    for _ in 0..65 {
        open_session(&repo, who(1), Timestamp(9)).unwrap();
    }
    let before = repo.snapshot(who(1)).unwrap();
    assert_eq!(
        purchase(&repo, who(1), session, 3).unwrap_err(),
        Error::Rejected(Reason::Expired)
    );
    assert_eq!(repo.snapshot(who(1)).unwrap(), before);
    assert_eq!(Balance::from_snapshot(&before), Ok(Balance(800)));
}
