// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[test]
fn paint_and_size_survive_reopen_preserve_spares_and_reject_stale_or_foreign_commits() {
    let directory = std::env::temp_dir().join(format!(
        "afterhours-appearance-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let mut definitions: Vec<_> = catalog()
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect();
    for (_, d) in &mut definitions[1..] {
        d.class = DefinitionClass::RimsItemData;
        d.default_derived = Some(Derived::decode(Layout::Rims, &[0; 52]).unwrap());
    }
    let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
    let offers = Offers::new([[3; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [([1; 16], vec![[3; 16]])])
        .unwrap()
        .with_part_policy(&catalog, [([2; 16], 0), ([3; 16], 0)], [([3; 16], 0)])
        .unwrap();
    let account = who(1);
    let (edit, stale, saved, foreign) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let before = setup(&repo, account);
        let foreign = setup(&repo, who(2));
        let ops = [11, 12]
            .map(|id| {
                let mut item = before.items[&ItemId::new(id).unwrap()].clone();
                item.derived = vec![0; 52];
                Op::Update(item)
            })
            .to_vec();
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
        let before = repo.snapshot(account).unwrap();
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        let base = prepare_vehicle(account, before.generation, &items, 10, &catalog).unwrap();
        assert_eq!(
            base.clone().with_rim_size(&items, &catalog, 12, 3),
            Err(Error::Ownership)
        );
        let stale = base
            .clone()
            .with_paint(
                &items,
                &catalog,
                &offers,
                Paint::new(1, [0; 3], [0; 3]).unwrap(),
            )
            .unwrap();
        let edit = base
            .with_paint(
                &items,
                &catalog,
                &offers,
                Paint::new(1, [0.25f32.to_bits(); 3], [0.75f32.to_bits(); 3]).unwrap(),
            )
            .unwrap()
            .with_rim_size(&items, &catalog, 11, 3)
            .unwrap()
            .with_paint(
                &items,
                &catalog,
                &offers,
                Paint::new(2, [0.5f32.to_bits(); 3], [1f32.to_bits(); 3]).unwrap(),
            )
            .unwrap();
        let saved = commit(&repo, account, &catalog, &edit, Timestamp(3)).unwrap();
        assert_eq!(saved.generation, before.generation + 1);
        assert_eq!(
            saved.items[&ItemId::new(12).unwrap()],
            before.items[&ItemId::new(12).unwrap()]
        );
        assert_eq!(saved.tables, before.tables);
        assert_eq!(
            commit(&repo, who(2), &catalog, &edit, Timestamp(4)),
            Err(Error::Ownership)
        );
        (edit, stale, saved, foreign)
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
        let items = inventory::collection(&saved, catalog.bindings()).unwrap();
        let Derived::Rims {
            rim_selection,
            primary_paint,
            primary_material,
            secondary_paint,
            secondary_material,
        } = items.items[&11].derived
        else {
            panic!()
        };
        assert_eq!(rim_selection, 3);
        assert_eq!(primary_paint, [0.25f32.to_bits(); 3]);
        assert_eq!(primary_material, [0.75f32.to_bits(); 3]);
        assert_eq!(secondary_paint, [0.5f32.to_bits(); 3]);
        assert_eq!(secondary_material, [1f32.to_bits(); 3]);
        assert_eq!(repo.snapshot(account).unwrap(), saved);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
