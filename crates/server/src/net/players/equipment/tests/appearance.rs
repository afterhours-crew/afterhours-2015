// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn appearance_wire(scene: u16, player: u16, paint: Option<u8>, item: u64, size: u8) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(2, 9)
        .put(if paint.is_some() { 1 } else { 10 }, 32);
    if let Some(target) = paint {
        for value in [f32::from(target + 1) / 8.; 3]
            .into_iter()
            .chain([1., 0.5, 0.])
        {
            payload.put(value.to_bits().into(), 32);
        }
        payload.put(target.into(), 8);
    } else {
        payload
            .put(item as u32 as u64, 32)
            .put(item >> 32, 32)
            .put(size.into(), 8);
    }
    payload.put(0, 7);
    let mut body = BitWriter::new();
    body.put(0, 64)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(player.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}

#[tokio::test]
async fn paint_size_and_tire_change_preserve_all_finishes_and_notify_live_vehicle() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    // Paint the stock rim, then swap and paint the new rim in the same batch.
    // Request order determines which item receives the first finish.
    accept(
        &mut listener,
        &appearance_wire(scene, player, Some(1), 0, 0),
    )
    .unwrap();
    accept(&mut listener, &wire(scene, player, Some(car), 0, 9)).unwrap();
    for target in 0..=4 {
        accept(
            &mut listener,
            &appearance_wire(scene, player, Some(target), 0, 0),
        )
        .unwrap();
    }
    accept(&mut listener, &appearance_wire(scene, player, None, 9, 2)).unwrap();
    assert_eq!(repo.snapshot(account).unwrap(), before);
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    let Derived::Rims {
        rim_selection,
        primary_paint,
        secondary_paint,
        primary_material,
        secondary_material,
    } = update.items.items[&9].derived
    else {
        panic!()
    };
    assert_eq!(rim_selection, 2);
    assert_eq!(primary_paint, [0.25f32.to_bits(); 3]);
    assert_eq!(secondary_paint, [0.375f32.to_bits(); 3]);
    assert_eq!(primary_material, [1f32, 0.5, 0.].map(f32::to_bits));
    assert_eq!(secondary_material, primary_material);
    let Derived::Rims {
        primary_paint,
        secondary_paint,
        ..
    } = update.items.items[&3].derived
    else {
        panic!()
    };
    assert_eq!(primary_paint, [0.5f32.to_bits(); 3]);
    assert_eq!(secondary_paint, [0.625f32.to_bits(); 3]);
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(saved.tables, before.tables);
    assert_ne!(
        saved.items[&ItemId::new(2).unwrap()],
        before.items[&ItemId::new(2).unwrap()]
    );
    assert_eq!(listener.pending[0].records[0].id, car);

    // Regression: swapping tires must not reset previously saved rim appearance.
    accept(&mut listener, &wire(scene, player, Some(car), 0, 10)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    let changed = crate::net::items::tests::deliver_notification(&mut jobs);
    let after = repo.snapshot(account).unwrap();
    assert!(changed.items.items[&1].children.contains(&10));
    assert!(!changed.items.items[&1].children.contains(&4));
    for id in [1, 2, 3, 9] {
        assert_eq!(
            after.items[&ItemId::new(id).unwrap()].derived,
            saved.items[&ItemId::new(id).unwrap()].derived
        );
    }
    assert_eq!(after.tables, before.tables);
    assert_eq!(after.generation, saved.generation + 1);
    // Exact repeated paint/size does not create another generation.
    accept(
        &mut listener,
        &appearance_wire(scene, player, Some(2), 0, 0),
    )
    .unwrap();
    accept(&mut listener, &appearance_wire(scene, player, None, 9, 2)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 5, Timestamp(5));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), after);
}

#[tokio::test]
async fn appearance_rejects_foreign_world_spare_and_out_of_range_size_without_writes() {
    for (item, size) in [(9, 0), (2, 1), (4, 0), (2, 255)] {
        let (mut listener, mut jobs, repo, account, scene, player, _) = fixture();
        let before = repo.snapshot(account).unwrap();
        accept(
            &mut listener,
            &appearance_wire(scene, player, None, item, size),
        )
        .unwrap();
        accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
        jobs.start(&mut listener, 3, Timestamp(3));
        assert!(!jobs.completed(&mut listener).await);
        assert_eq!(repo.snapshot(account).unwrap(), before);
        assert!(listener.pending.is_empty());
    }
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(
        accept(
            &mut listener,
            &appearance_wire(scene, player + 1000, Some(1), 0, 0)
        )
        .is_err()
    );
    assert!(
        accept(
            &mut listener,
            &appearance_wire(scene, player, None, 1000, 0)
        )
        .is_err()
    );
    accept(
        &mut listener,
        &appearance_wire(scene, player, Some(1), 0, 0),
    )
    .unwrap();
    accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(!jobs.working());
    listener.world_cars.insert(car, (0, false));
    assert!(
        accept(
            &mut listener,
            &appearance_wire(scene, player, Some(1), 0, 0)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(account).unwrap(), before);
}
