// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn visual_reset(scene: u16, player: u16, car: u16, keep: bool) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(2, 9)
        .put(6, 32)
        .put(u64::from(keep), 8)
        .put(0, 7);
    let mut body = BitWriter::new();
    body.put(0, 64)
        .put(3, 8)
        .put(scene.into(), 13)
        .put(player.into(), 13)
        .put(car.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}

#[test]
fn visual_reset_clears_prior_stance_only_and_preserves_later_tuning_order() {
    let (mut listener, _, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(
        accept(
            &mut listener,
            &handling_wire(scene, player, car, &[(10, 7), (14, 8), (26, 6)])
        )
        .unwrap()
    );
    assert!(accept(&mut listener, &visual_reset(scene, player, car, true)).unwrap());
    assert!(
        accept(
            &mut listener,
            &handling_wire(scene, player, car, &[(14, 4)])
        )
        .unwrap()
    );
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    let request = listener.take_equipment().unwrap();
    assert_eq!(
        request.handling.iter().collect::<Vec<_>>(),
        [(14, 4), (26, 6)]
    );
    assert_eq!(
        request.part_edits,
        [nfs_services::item_builder::Operation::StockVisual {
            vehicle: car,
            keep_wheels: true
        }]
    );
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(accept(&mut listener, &visual_reset(scene, player, car + 1, true)).is_err());
}

#[tokio::test]
async fn removal_commits_and_publishes_car_without_selling_the_part() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 12, 7)).unwrap());
    // Cancel before committing: another begin clears the pending removal.
    assert!(accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    assert!(listener.take_equipment().is_none());
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 12, 7)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert!(!update.items.items[&1].children.contains(&6));
    assert!(!update.items.items[&1].children.contains(&7));
    let saved = repo.snapshot(account).unwrap();
    for id in [6, 7] {
        assert_eq!(
            saved.items[&ItemId::new(id).unwrap()],
            before.items[&ItemId::new(id).unwrap()]
        );
    }
    assert_eq!(saved.tables, before.tables);
    assert_eq!(listener.pending[0].records[0].id, car);
    assert!(listener.pending_rpcs.is_empty());
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 12, 7)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), saved);
}
