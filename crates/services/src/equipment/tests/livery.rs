// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn fixture(repo: &dyn InventoryRepository, account: AccountId) -> (InventoryCatalog, Snapshot) {
    let base = setup(repo, account);
    let mut defs: Vec<_> = catalog()
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect();
    let derived = Derived::LiveryCustomization {
        byte_vault_data_id: 0,
        library_preset_id: u32::MAX,
        dynamic_flags: 0b1011,
    };
    defs.push((
        [4; 16],
        Definition {
            class: DefinitionClass::LiveryCustomizationItemData,
            flags: DefinitionFlags::default(),
            default_derived: Some(derived.clone()),
            ..defs[1].1.clone()
        },
    ));
    let catalog = InventoryCatalog::new(defs, vec![]).unwrap();
    let livery = ItemRecord {
        id: ItemId::new(20).unwrap(),
        definition: DefinitionGuid([4; 16]),
        owner: None,
        state: OWNED,
        buy_price: 0,
        sell_price: 0,
        default_items: vec![],
        sub_items: vec![],
        derived: derived.encode().unwrap(),
    };
    repo.apply(
        account,
        &Batch {
            id: 2,
            expected_generation: base.generation,
            ops: vec![Op::Insert(livery)],
        },
        Timestamp(2),
    )
    .unwrap();
    (catalog, repo.snapshot(account).unwrap())
}

#[test]
fn library_livery_and_body_paint_commit_together_and_survive_reopen() {
    let directory = std::env::temp_dir().join(format!(
        "afterhours-livery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let account = who(1);
    let (catalog, edit, stale, before, saved, foreign) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let (catalog, before) = fixture(&repo, account);
        let (_, foreign) = fixture(&repo, who(2));
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        let base = prepare_vehicle(account, before.generation, &items, 10, &catalog).unwrap();
        let staged = base
            .clone()
            .with_livery(&items, &catalog, 20, 3, u32::MAX)
            .unwrap();
        assert!(
            !staged.preview(&items).unwrap().items[&10]
                .children
                .contains(&20)
        );
        assert_eq!(repo.snapshot(account).unwrap(), before);
        let offers = Offers::new([[3; 16]]).unwrap();
        let edit = staged
            .with_paint(
                &items,
                &catalog,
                &offers,
                Paint::new(0, [0.25f32.to_bits(); 3], [0.5f32.to_bits(); 3]).unwrap(),
            )
            .unwrap()
            .with_part(&items, 20, &catalog, &offers)
            .unwrap()
            // Later ordinary upgrades must preserve the root-owned livery.
            .with_part(&items, 12, &catalog, &offers)
            .unwrap()
            .with_handling(&items, &catalog, Default::default(), None)
            .unwrap();
        let stale = base.with_livery(&items, &catalog, 20, 4, u32::MAX).unwrap();
        let saved = commit(&repo, account, &catalog, &edit, Timestamp(3)).unwrap();
        let restored = inventory::collection(&saved, catalog.bindings()).unwrap();
        assert_eq!(restored.items[&10].children, [20, 12]);
        assert_eq!(restored.items[&20].owner, 0);
        assert_eq!(
            restored.items[&20].derived,
            Derived::LiveryCustomization {
                byte_vault_data_id: 3,
                library_preset_id: u32::MAX,
                dynamic_flags: 0b1010,
            }
        );
        assert!(matches!(restored.items[&10].derived,
            Derived::RaceVehicle {default_paint, material, ..}
                if default_paint == [0.25f32.to_bits(); 3] && material == [0.5f32.to_bits(); 3]));
        assert_eq!(saved.generation, before.generation + 1);
        assert_eq!(saved.items.len(), before.items.len());
        assert_eq!(saved.tables, before.tables);
        assert_eq!(saved.garage, before.garage);
        assert_eq!(
            commit(&repo, who(2), &catalog, &edit, Timestamp(4)),
            Err(Error::Ownership)
        );
        (catalog, edit, stale, before, saved, foreign)
    };
    {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), saved);
        assert_eq!(repo.open(who(2)).unwrap(), foreign);
        assert_eq!(
            commit(&repo, account, &catalog, &edit, Timestamp(5)).unwrap(),
            saved
        );
        assert_eq!(
            commit(&repo, account, &catalog, &stale, Timestamp(6)),
            Err(Error::Conflict)
        );
        assert_eq!(
            saved.items[&ItemId::new(11).unwrap()],
            before.items[&ItemId::new(11).unwrap()]
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn livery_rejects_foreign_parts_shared_car_aliases_and_wrong_classes() {
    let repo = MemoryRepository::default();
    let account = who(1);
    let (catalog, before) = fixture(&repo, account);
    let items = inventory::collection(&before, catalog.bindings()).unwrap();
    for mutation in 0..5 {
        let mut items = items.clone();
        match mutation {
            0 => items.items.get_mut(&20).unwrap().owner = 12,
            1 => items.items.get_mut(&20).unwrap().state = 4,
            2 => items.items.get_mut(&20).unwrap().children.push(11),
            3 => items.items.get_mut(&12).unwrap().children.push(20),
            4 => {
                items.items.get_mut(&20).unwrap().definition = [2; 16];
                items.items.get_mut(&20).unwrap().derived = Derived::Empty;
            }
            _ => unreachable!(),
        }
        let edit = prepare_vehicle(account, before.generation, &items, 10, &catalog).unwrap();
        assert!(edit.with_livery(&items, &catalog, 20, 2, 0).is_err());
    }
    assert_eq!(repo.snapshot(account).unwrap(), before);
}
