// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

pub(super) fn car_finish_wire(
    scene: u16,
    player: u16,
    plate: Option<&[u8]>,
    tint: f32,
) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(2, 9)
        .put(if plate.is_some() { 2 } else { 11 }, 32);
    if let Some(text) = plate {
        payload.put(text.len() as u64, 10);
        for byte in text {
            payload.put((*byte).into(), 8);
        }
    } else {
        payload.put(tint.to_bits().into(), 32);
    }
    let padding = (8 - payload.len() % 8) % 8;
    payload.put(0, padding);
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
async fn plate_submissions_and_tint_commit_preserve_order_and_later_equipment() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    accept(
        &mut listener,
        &appearance_wire(scene, player, Some(0), 0, 0),
    )
    .unwrap();
    accept(&mut listener, &wire(scene, player, None, 7, 0)).unwrap();
    for body in [
        car_finish_wire(scene, player, Some(b"FIRST"), 0.),
        car_finish_wire(scene, player, None, 0.75),
        car_finish_wire(scene, player, Some(b"LOCAL12"), 0.),
    ] {
        accept(&mut listener, &body).unwrap();
    }
    assert_eq!(repo.snapshot(account).unwrap(), before);
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    for time in [1, 2] {
        jobs.start(&mut listener, time, Timestamp(time));
        assert!(jobs.completed(&mut listener).await);
        crate::net::items::tests::deliver_notification(&mut jobs);
    }
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    let Derived::RaceVehicle {
        window_tint,
        default_license_plate_text,
        ..
    } = update.items.items[&1].derived
    else {
        panic!()
    };
    assert_eq!(window_tint, 0.75f32.to_bits());
    assert_eq!(default_license_plate_text, *b"LOCAL12\0");
    assert_eq!(listener.pending[0].records[0].id, car);
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(
        &saved.items[&ItemId::new(1).unwrap()].derived[..24],
        &before.items[&ItemId::new(1).unwrap()].derived[..24]
    );
    assert_eq!(saved.generation, before.generation + 3);
    assert_eq!(saved.tables, before.tables);
    // Exact retransmission after commit is a durable no-op.
    accept(&mut listener, &car_finish_wire(scene, player, None, 0.75)).unwrap();
    accept(
        &mut listener,
        &car_finish_wire(scene, player, Some(b"LOCAL12"), 0.),
    )
    .unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    crate::net::items::tests::deliver_notification(&mut jobs);
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    crate::net::items::tests::deliver_notification(&mut jobs);
    assert_eq!(repo.snapshot(account).unwrap(), saved);
    accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 5, Timestamp(5));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert_eq!(
        update.items.items[&1].derived.encode().unwrap(),
        saved.items[&ItemId::new(1).unwrap()].derived
    );
}

#[tokio::test]
async fn tint_cancellation_and_foreign_or_world_finish_targets_cannot_save() {
    for plate in [None, Some(b"LOCAL".as_slice())] {
        let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
        let before = repo.snapshot(account).unwrap();
        assert!(
            accept(
                &mut listener,
                &car_finish_wire(scene, player + 1000, plate, 0.5)
            )
            .is_err()
        );
        accept(&mut listener, &car_finish_wire(scene, player, None, 0.5)).unwrap();
        accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap();
        accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
        jobs.start(&mut listener, 3, Timestamp(3));
        assert!(!jobs.working());
        listener.world_cars.insert(car, (0, false));
        assert!(accept(&mut listener, &car_finish_wire(scene, player, plate, 0.5)).is_err());
        assert_eq!(repo.snapshot(account).unwrap(), before);
    }
}

fn livery_wire(scene: u16, player: u16, item: u64, data_id: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(2, 9)
        .put(9, 32)
        .put(item as u32 as u64, 32)
        .put(item >> 32, 32)
        .put(data_id.into(), 32)
        .put(u32::MAX.into(), 32)
        .put(0, 7);
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
async fn livery_edit_paint_equip_and_commit_deliver_the_owned_inventory() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    accept(&mut listener, &livery_wire(scene, player, 11, 3)).unwrap();
    accept(
        &mut listener,
        &appearance_wire(scene, player, Some(0), 0, 0),
    )
    .unwrap();
    accept(&mut listener, &wire(scene, player, Some(car), 0, 11)).unwrap();
    assert_eq!(repo.snapshot(account).unwrap(), before);
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert!(update.items.items[&1].children.contains(&11));
    assert_eq!(update.items.items[&11].owner, 0);
    assert_eq!(
        update.items.items[&11].derived,
        Derived::LiveryCustomization {
            byte_vault_data_id: 3,
            library_preset_id: u32::MAX,
            dynamic_flags: 0
        }
    );
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(saved.tables, before.tables);
    assert_eq!(listener.pending[0].records[0].id, car);
    // An unrelated part change still constructs all components with this livery.
    accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert!(update.items.items[&1].children.contains(&11));
    assert_eq!(
        repo.snapshot(account).unwrap().items[&ItemId::new(11).unwrap()],
        saved.items[&ItemId::new(11).unwrap()]
    );
}

#[tokio::test]
async fn canceled_or_foreign_livery_edits_do_not_mutate_inventory() {
    let (mut listener, mut jobs, repo, account, scene, player, _) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(accept(&mut listener, &livery_wire(scene, player + 1000, 11, 3)).is_err());
    assert!(accept(&mut listener, &livery_wire(scene, player, 1000, 3)).is_err());
    accept(&mut listener, &livery_wire(scene, player, 11, 3)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(!jobs.working());
    assert_eq!(repo.snapshot(account).unwrap(), before);
}

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
