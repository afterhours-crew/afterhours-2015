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
            .with_license_plate(LicensePlate::new(b"FIRST"))
            .unwrap()
            .with_window_tint(WindowTint::new(0.75f32.to_bits()).unwrap())
            .unwrap()
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
        let edit = edit
            .with_license_plate(LicensePlate::new(b"FINAL12"))
            .unwrap();
        // Staging/cancellation cannot write the durable account.
        assert_eq!(repo.snapshot(account).unwrap(), before);
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
        let Derived::RaceVehicle {
            window_tint,
            default_license_plate_text,
            ..
        } = items.items[&10].derived
        else {
            panic!()
        };
        assert_eq!(window_tint, 0.75f32.to_bits());
        assert_eq!(default_license_plate_text, *b"FINAL12\0");
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

#[test]
fn plate_copy_preserves_tail_and_all_other_car_bytes() {
    let catalog = catalog();
    let repo = nfs_storage::MemoryRepository::default();
    let before = setup(&repo, who(1));
    let mut items = inventory::collection(&before, catalog.bindings()).unwrap();
    let Derived::RaceVehicle {
        default_license_plate_text,
        ..
    } = &mut items.items.get_mut(&10).unwrap().derived
    else {
        panic!()
    };
    *default_license_plate_text = *b"OLD1234\0";
    let base = prepare_vehicle(who(1), before.generation, &items, 10, &catalog).unwrap();
    for (input, expected) in [
        (b"".as_slice(), *b"\0LD1234\0"),
        (b"NEW", *b"NEW\x00234\0"),
        (b"AB\0IGNORED", *b"AB\x001234\0"),
        (b"ABCDEFGH", *b"ABCDEFG\0"),
    ] {
        let preview = base
            .clone()
            .with_license_plate(LicensePlate::new(input))
            .unwrap()
            .preview(&items)
            .unwrap();
        let mut expected_car = items.items[&10].clone();
        let Derived::RaceVehicle {
            default_license_plate_text,
            ..
        } = &mut expected_car.derived
        else {
            panic!()
        };
        *default_license_plate_text = expected;
        assert_eq!(preview.items[&10], expected_car);
        assert_eq!(preview.items[&11], items.items[&11]);
    }
}

#[test]
fn stock_paint_uses_this_definition_and_preserves_every_other_saved_field() {
    let repo = MemoryRepository::default();
    let before = setup(&repo, who(1));
    let mut definitions: Vec<_> = catalog()
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect();
    let Some(Derived::RaceVehicle {
        default_paint,
        material,
        ..
    }) = &mut definitions[0].1.default_derived
    else {
        panic!()
    };
    *default_paint = [0.25f32, 0.5, 0.75].map(f32::to_bits);
    *material = [1f32, 0.125, 0.].map(f32::to_bits);
    let catalog = InventoryCatalog::new(definitions.clone(), vec![]).unwrap();
    let mut items = inventory::collection(&before, catalog.bindings()).unwrap();
    let Derived::RaceVehicle {
        default_paint,
        material,
        window_tint,
        default_license_plate_text,
        vehicle_item_flags,
        ..
    } = &mut items.items.get_mut(&10).unwrap().derived
    else {
        panic!()
    };
    *default_paint = [1f32.to_bits(); 3];
    *material = [0; 3];
    *window_tint = 0.75f32.to_bits();
    *default_license_plate_text = *b"KEEP123\0";
    *vehicle_item_flags = 3;
    let base = prepare_vehicle(who(1), before.generation, &items, 10, &catalog).unwrap();
    let once = base.clone().with_stock_paint(&catalog).unwrap();
    let twice = once.clone().with_stock_paint(&catalog).unwrap();
    assert_eq!(once, twice);
    let preview = once.preview(&items).unwrap();
    let bytes = preview.items[&10].derived.encode().unwrap();
    let defaults = definitions[0]
        .1
        .default_derived
        .as_ref()
        .unwrap()
        .encode()
        .unwrap();
    assert_eq!(&bytes[..24], &defaults[..24]);
    assert_eq!(
        &bytes[24..],
        &items.items[&10].derived.encode().unwrap()[24..]
    );
    assert_eq!(preview.items[&11], items.items[&11]);
    definitions[0].1.default_derived = None;
    assert_eq!(
        base.with_stock_paint(&InventoryCatalog::new(definitions, vec![]).unwrap()),
        Err(Error::Unsupported)
    );
    assert_eq!(repo.snapshot(who(1)).unwrap(), before);
}
