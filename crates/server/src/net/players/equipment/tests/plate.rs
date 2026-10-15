// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{appearance::car_finish_wire, *};

#[tokio::test]
async fn accepted_plate_saves_without_commit_and_leaves_preview_cancellable() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    let Derived::RaceVehicle {
        window_tint: original_tint,
        ..
    } = listener.inventory.as_ref().unwrap().items.items[&1].derived
    else {
        panic!()
    };
    accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap();
    accept(&mut listener, &car_finish_wire(scene, player, None, 0.75)).unwrap();
    accept(
        &mut listener,
        &car_finish_wire(scene, player, Some(b"LOCAL"), 0.),
    )
    .unwrap();
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    let Derived::RaceVehicle {
        default_license_plate_text,
        window_tint,
        ..
    } = update.items.items[&1].derived
    else {
        panic!()
    };
    assert_eq!(default_license_plate_text, *b"LOCAL\0\0\0");
    assert_eq!(window_tint, original_tint);
    assert_eq!(listener.pending[0].records[0].id, car);
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(saved.tables, before.tables);
    // Opening another editor cancels the tint preview, not the accepted text.
    accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(!jobs.working());
    assert_eq!(repo.snapshot(account).unwrap(), saved);
    // Repeating the accepted text has no durable side effects.
    accept(
        &mut listener,
        &car_finish_wire(scene, player, Some(b"LOCAL"), 0.),
    )
    .unwrap();
    jobs.start(&mut listener, 5, Timestamp(5));
    assert!(jobs.completed(&mut listener).await);
    crate::net::items::tests::deliver_notification(&mut jobs);
    assert_eq!(repo.snapshot(account).unwrap(), saved);
}

#[tokio::test]
async fn plate_submission_behind_queued_equip_preserves_the_committed_part() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap();
    accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap();
    accept(
        &mut listener,
        &car_finish_wire(scene, player, Some(b"LOCAL"), 0.),
    )
    .unwrap();
    // Even before delivery, a later Begin cannot discard the accepted text.
    accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap();
    for time in [3, 4] {
        jobs.start(&mut listener, time, Timestamp(time));
        assert!(jobs.completed(&mut listener).await);
        let update = crate::net::items::tests::deliver_notification(&mut jobs);
        assert!(update.items.items[&1].children.contains(&7));
    }
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 2);
    assert_eq!(saved.tables, before.tables);
    let items = &listener.inventory.as_ref().unwrap().items;
    let Derived::RaceVehicle {
        default_license_plate_text,
        ..
    } = items.items[&1].derived
    else {
        panic!()
    };
    assert_eq!(default_license_plate_text, *b"LOCAL\0\0\0");
}

#[tokio::test]
async fn stale_plate_submission_cannot_overwrite_an_external_account_change() {
    let (mut listener, mut jobs, repo, account, scene, player, _) = fixture();
    let before = repo.snapshot(account).unwrap();
    accept(
        &mut listener,
        &car_finish_wire(scene, player, Some(b"LOCAL"), 0.),
    )
    .unwrap();
    repo.apply(
        account,
        &Batch {
            id: 90,
            expected_generation: before.generation,
            ops: vec![nfs_services::bank::Balance(71).store()],
        },
        Timestamp(3),
    )
    .unwrap();
    let other = repo.snapshot(account).unwrap();
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(!jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), other);
    assert!(listener.pending.is_empty());
    assert!(!jobs.working());
}

#[test]
fn accepted_plate_queue_is_bounded_and_foreign_input_does_not_consume_it() {
    let (mut listener, _, repo, account, scene, player, _) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(
        accept(
            &mut listener,
            &car_finish_wire(scene, player + 1000, Some(b"LOCAL"), 0.)
        )
        .is_err()
    );
    assert!(listener.equipment.committed.is_empty());
    for _ in 0..16 {
        accept(
            &mut listener,
            &car_finish_wire(scene, player, Some(b"LOCAL"), 0.),
        )
        .unwrap();
    }
    assert_eq!(
        accept(
            &mut listener,
            &car_finish_wire(scene, player, Some(b"OTHER"), 0.)
        ),
        Err(replication::Error::Bound)
    );
    assert_eq!(listener.equipment.committed.len(), 16);
    assert!(listener.equipment.edits.is_empty());
    assert_eq!(repo.snapshot(account).unwrap(), before);
}
