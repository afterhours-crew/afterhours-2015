// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::{PurchaseRequest, PurchaseSelection};

fn fixture(
    repo: &dyn InventoryRepository,
    account: AccountId,
) -> (InventoryCatalog, Offers, Snapshot) {
    let base = catalog();
    let mut leaf = base.definition(&[2; 16]).unwrap().clone();
    leaf.flags.purchasable = true;
    leaf.buy_price = 7;
    let mut definitions = BTreeMap::new();
    for (id, class, optional) in [
        (2, DefinitionClass::HoodItemData, false),
        (3, DefinitionClass::SplitterItemData, false),
        (4, DefinitionClass::CanardItemData, true),
        (5, DefinitionClass::AirFilterItemData, false),
        (6, DefinitionClass::HoodItemData, false),
        (7, DefinitionClass::CanardItemData, true),
        (8, DefinitionClass::SplitterItemData, false),
        (9, DefinitionClass::HoodItemData, false),
    ] {
        let mut d = leaf.clone();
        d.class = class;
        d.flags.optional = optional;
        definitions.insert([id; 16], d);
    }
    let mut car = base.definition(&[1; 16]).unwrap().clone();
    car.sub_items = vec![[2; 16], [3; 16], [4; 16], [5; 16]];
    definitions.insert([1; 16], car);
    for (id, children, price) in [(10, vec![[6; 16], [7; 16]], 40), (11, vec![[8; 16]], 30)] {
        definitions.insert(
            [id; 16],
            Definition {
                class: DefinitionClass::BodyKitItemData,
                sub_items: children,
                buy_price: price,
                ..leaf.clone()
            },
        );
    }
    let c = InventoryCatalog::new(definitions, vec![[1; 16]]).unwrap();
    let offers = Offers::new([[9; 16], [10; 16], [11; 16]])
        .unwrap()
        .with_vehicle_scopes(&c, [([1; 16], vec![[9; 16], [10; 16], [11; 16]])])
        .unwrap()
        .with_part_policy(
            &c,
            [([4; 16], 0), ([7; 16], 0)],
            [([9; 16], 0), ([10; 16], 0), ([11; 16], 0)],
        )
        .unwrap()
        .with_fitment_restrictions(&c, [])
        .unwrap()
        .with_body_kits(&c, [[10; 16], [11; 16]])
        .unwrap();
    let initial = inventory::load(repo, account, &c, 10, Timestamp(1)).unwrap();
    repo.apply(
        account,
        &Batch {
            id: 20,
            expected_generation: initial.generation,
            ops: vec![Balance(1000).store()],
        },
        Timestamp(2),
    )
    .unwrap();
    for definition in [10, 11, 9] {
        crate::purchase::buy(
            repo,
            account,
            &c,
            &offers,
            &PurchaseRequest {
                sequence: 2,
                owner: 10,
                selection: PurchaseSelection::Definitions(vec![[definition; 16]]),
            },
            Timestamp(3),
        )
        .unwrap();
    }
    (c, offers, repo.snapshot(account).unwrap())
}

#[test]
fn kit_equipment_replacement_and_stock_reset_preserve_spares_and_cash_after_restart() {
    let directory = std::env::temp_dir().join(format!(
        "nfs-kit-equip-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let (c, saved) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let (c, offers, before) = fixture(&repo, who(1));
        let (_, _, other) = fixture(&repo, who(2));
        let items = inventory::collection(&before, c.bindings()).unwrap();
        let edit = prepare(who(1), before.generation, &items, 10, 15, &c, &offers).unwrap();
        assert_eq!(
            edit.preview(&items).unwrap().items[&10].children,
            [12, 14, 15]
        );
        assert_eq!(repo.snapshot(who(1)).unwrap(), before); // canceled preview
        assert_eq!(
            commit(&repo, who(2), &c, &edit, Timestamp(4)),
            Err(Error::Ownership)
        );
        let first = commit(&repo, who(1), &c, &edit, Timestamp(4)).unwrap();
        assert_eq!(
            commit(&repo, who(1), &c, &edit, Timestamp(5)).unwrap(),
            first
        );
        let items = inventory::collection(&first, c.bindings()).unwrap();
        let replace = prepare(who(1), first.generation, &items, 10, 18, &c, &offers).unwrap();
        // The first kit's optional canard is not restored; missing mandatory
        // hood returns from car defaults when the new kit covers only splitter.
        assert_eq!(
            replace.preview(&items).unwrap().items[&10].children,
            [14, 18, 11]
        );
        let individual = prepare(who(1), first.generation, &items, 10, 20, &c, &offers).unwrap();
        assert_eq!(
            individual.preview(&items).unwrap().items[&10].children,
            [12, 14, 15, 20]
        );
        let individual = commit(&repo, who(1), &c, &individual, Timestamp(6)).unwrap();
        assert_eq!(
            commit(&repo, who(1), &c, &replace, Timestamp(7)),
            Err(Error::Conflict)
        );
        let items = inventory::collection(&individual, c.bindings()).unwrap();
        let replace = prepare(who(1), individual.generation, &items, 10, 18, &c, &offers).unwrap();
        assert_eq!(
            replace.preview(&items).unwrap().items[&10].children,
            [14, 20, 18]
        );
        let replaced = commit(&repo, who(1), &c, &replace, Timestamp(8)).unwrap();
        let items = inventory::collection(&replaced, c.bindings()).unwrap();
        let reset = prepare_vehicle(who(1), replaced.generation, &items, 10, &c)
            .unwrap()
            .with_stock_visual(&items, &c, false)
            .unwrap();
        assert_eq!(
            reset.preview(&items).unwrap().items[&10].children,
            [11, 12, 13, 14]
        );
        let saved = commit(&repo, who(1), &c, &reset, Timestamp(9)).unwrap();
        assert_eq!(saved.tables, before.tables);
        assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(923));
        assert_eq!(saved.items.len(), before.items.len());
        for (id, part) in &before.items {
            if id.get() != 10 {
                assert_eq!(&saved.items[id], part);
            }
        }
        assert_eq!(repo.snapshot(who(2)).unwrap(), other);
        (c, saved)
    };
    {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(who(1)).unwrap(), saved);
        inventory::collection(&saved, c.bindings()).unwrap();
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn kit_equipment_checks_child_ownership_graph_and_remaining_fitment() {
    let repo = MemoryRepository::default();
    let (c, offers, before) = fixture(&repo, who(1));
    let items = inventory::collection(&before, c.bindings()).unwrap();
    for selected in [16, 17, 999] {
        assert!(prepare(who(1), before.generation, &items, 10, selected, &c, &offers).is_err());
    }
    for mutation in 0..5 {
        let mut bad = items.clone();
        match mutation {
            0 => bad.items.get_mut(&16).unwrap().owner = 10,
            1 => bad
                .items
                .get_mut(&15)
                .unwrap()
                .children
                .pop()
                .map(|_| ())
                .unwrap(),
            2 => bad.items.get_mut(&15).unwrap().owner = 18,
            3 => bad.items.get_mut(&16).unwrap().definition = [8; 16],
            _ => bad.items.get_mut(&15).unwrap().state = 1,
        }
        assert!(
            prepare(who(1), before.generation, &bad, 10, 15, &c, &offers).is_err(),
            "mutation {mutation}"
        );
    }
    let blocked = offers
        .clone()
        .with_fitment_restrictions(&c, [([10; 16], vec![[5; 16]])])
        .unwrap();
    assert_eq!(
        prepare(who(1), before.generation, &items, 10, 15, &c, &blocked),
        Err(Error::Unsupported)
    );
    let replaced = offers
        .with_fitment_restrictions(&c, [([10; 16], vec![[2; 16]])])
        .unwrap();
    assert!(prepare(who(1), before.generation, &items, 10, 15, &c, &replaced).is_ok());
    assert_eq!(repo.snapshot(who(1)).unwrap(), before);
}
