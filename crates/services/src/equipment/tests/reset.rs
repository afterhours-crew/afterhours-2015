// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn fixture(repo: &dyn InventoryRepository, account: AccountId) -> (InventoryCatalog, Snapshot) {
    use DefinitionClass::*;
    let before = setup(repo, account);
    let mut defs: Vec<_> = catalog()
        .definitions()
        .map(|(g, d)| (*g, d.clone()))
        .collect();
    let mut ops = Vec::new();
    for (id, class, bytes) in [
        (20, HoodItemData, 0),
        (21, HoodItemData, 0),
        (22, RimsItemData, 52),
        (23, RimsItemData, 52),
        (24, LiveryCustomizationItemData, 12),
        (25, LicensePlateBackgroundItemData, 0),
        (26, LicensePlateFrameItemData, 0),
        (27, BodyKitItemData, 0),
        (28, ControlArmTuningItemData, 36),
        (29, TiresItemData, 4),
        (30, TiresItemData, 4),
        (31, CalipersItemData, 0),
        (32, CalipersItemData, 0),
        (33, BrakeDiscsItemData, 8),
        (34, BrakeDiscsItemData, 8),
    ] {
        let default = Derived::decode(class.layout(), &vec![0; bytes])
            .unwrap_or_else(|error| panic!("{class:?}: {error:?}"));
        defs.push((
            [id; 16],
            Definition {
                class,
                default_derived: Some(default.clone()),
                flags: DefinitionFlags {
                    requires_owner: id != 24,
                    ..Default::default()
                },
                ..defs[1].1.clone()
            },
        ));
        let derived = if id == 28 {
            Derived::decode(class.layout(), &[9u32.to_le_bytes(); 9].concat()).unwrap()
        } else {
            default
        };
        ops.push(Op::Insert(ItemRecord {
            id: ItemId::new(id.into()).unwrap(),
            definition: DefinitionGuid([id; 16]),
            owner: if id == 24 {
                None
            } else {
                Some(ItemId::new(10).unwrap())
            },
            derived: derived.encode().unwrap(),
            ..before.items[&ItemId::new(11).unwrap()].clone()
        }));
    }
    let mut car = before.items[&ItemId::new(10).unwrap()].clone();
    car.default_items = [11, 20, 22, 25, 26, 29, 31, 33]
        .map(|id| ItemId::new(id).unwrap())
        .to_vec();
    car.sub_items = [11, 21, 23, 24, 25, 26, 27, 28, 30, 32, 34]
        .map(|id| ItemId::new(id).unwrap())
        .to_vec();
    let mut derived = Derived::decode(Layout::RaceVehicle, &car.derived).unwrap();
    if let Derived::RaceVehicle {
        window_tint,
        default_paint,
        default_license_plate_text,
        ..
    } = &mut derived
    {
        *window_tint = 0.9f32.to_bits();
        *default_paint = [0.25f32.to_bits(); 3];
        *default_license_plate_text = *b"KEEPME\0x";
    }
    car.derived = derived.encode().unwrap();
    if let Some(Derived::RaceVehicle { window_tint, .. }) = &mut defs[0].1.default_derived {
        *window_tint = 0.2f32.to_bits();
    }
    ops.push(Op::Update(car));
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
    (
        InventoryCatalog::new(defs, vec![]).unwrap(),
        repo.snapshot(account).unwrap(),
    )
}

#[test]
fn visual_reset_options_preserve_livery_paint_performance_and_owned_spares() {
    for keep_wheels in [false, true] {
        let repo = MemoryRepository::default();
        let account = who(1);
        let (catalog, before) = fixture(&repo, account);
        let (_, other) = fixture(&repo, who(2));
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        let pending = prepare_vehicle(account, before.generation, &items, 10, &catalog)
            .unwrap()
            .with_stock_visual(&items, &catalog, keep_wheels)
            .unwrap();
        let next = pending.preview(&items).unwrap();
        assert_eq!(
            next.items[&10].children,
            if keep_wheels {
                vec![20, 11, 23, 24, 28, 30, 32, 34]
            } else {
                vec![20, 22, 29, 31, 33, 11, 24, 28]
            }
        );
        let mut car = items.items[&10].clone();
        car.children = next.items[&10].children.clone();
        if let Derived::RaceVehicle { window_tint, .. } = &mut car.derived {
            *window_tint = 0.2f32.to_bits();
        }
        assert_eq!(next.items[&10], car); // All other car fields stay identical.
        assert_eq!(
            next.items[&28].derived,
            catalog
                .definition(&[28; 16])
                .unwrap()
                .default_derived
                .clone()
                .unwrap()
        );
        for (id, original) in &items.items {
            if ![10, 28].contains(id) {
                assert_eq!(&next.items[id], original);
            }
        }
        assert_eq!(repo.snapshot(account).unwrap(), before);
        assert_eq!(
            commit(&repo, who(2), &catalog, &pending, Timestamp(3)),
            Err(Error::Ownership)
        );
        let saved = commit(&repo, account, &catalog, &pending, Timestamp(3)).unwrap();
        assert_eq!(saved.tables, before.tables);
        assert_eq!(repo.snapshot(who(2)).unwrap(), other);
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(4)).unwrap(),
            saved
        );
        let current = inventory::collection(&saved, catalog.bindings()).unwrap();
        let changed = prepare_vehicle(account, saved.generation, &current, 10, &catalog)
            .unwrap()
            .with_window_tint(WindowTint::new(0.4f32.to_bits()).unwrap())
            .unwrap();
        commit(&repo, account, &catalog, &changed, Timestamp(5)).unwrap();
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(6)),
            Err(Error::Conflict)
        );
    }
}

#[test]
fn reset_reopen_and_later_stance_edit_are_durable() {
    let directory = std::env::temp_dir().join(format!(
        "nfs-visual-reset-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let account = who(1);
    let (catalog, pending, saved) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let (catalog, before) = fixture(&repo, account);
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        let pending = prepare_vehicle(account, before.generation, &items, 10, &catalog)
            .unwrap()
            .with_handling(
                &items,
                &catalog,
                nfs_world_core::items::HandlingChanges::new([(10, 8), (14, 7)]).unwrap(),
                None,
            )
            .unwrap()
            .with_stock_visual(&items, &catalog, true)
            .unwrap()
            .with_handling(
                &items,
                &catalog,
                nfs_world_core::items::HandlingChanges::new([(14, 6)]).unwrap(),
                None,
            )
            .unwrap();
        let next = pending.preview(&items).unwrap();
        let Derived::ControlArmTuning {
            caster_setting,
            ride_height_height_setting,
            ..
        } = next.items[&28].derived
        else {
            panic!()
        };
        assert_eq!((caster_setting, ride_height_height_setting), (6, 0));
        let saved = commit(&repo, account, &catalog, &pending, Timestamp(3)).unwrap();
        (catalog, pending, saved)
    };
    {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), saved);
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(4)).unwrap(),
            saved
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
