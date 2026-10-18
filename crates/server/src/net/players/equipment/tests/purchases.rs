// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[test]
fn purchased_instance_refreshes_live_snapshot_and_bank_without_equipping() {
    let (mut listener, jobs, repo, account, _, _, _) = fixture();
    let snapshot = repo.snapshot(account).unwrap();
    let mut grant = snapshot.items[&ItemId::new(7).unwrap()].clone();
    grant.owner = None;
    grant.state = nfs_world::items::PURCHASABLE;
    repo.apply(
        account,
        &Batch {
            id: 90,
            expected_generation: snapshot.generation,
            ops: vec![Op::Update(grant.clone())],
        },
        Timestamp(3),
    )
    .unwrap();
    let snapshot = repo.snapshot(account).unwrap();
    let old = listener.inventory.as_ref().unwrap().clone();
    let before = Inventory {
        slots: old.slots,
        items: Arc::new(crate::net::items::tests::reload_inventory(&jobs, &snapshot)),
    };
    listener
        .population
        .as_mut()
        .unwrap()
        .refresh_inventory(
            &listener.players,
            HOST_SELECTOR as u8,
            listener.persona,
            &old,
            &before,
        )
        .unwrap();
    listener.inventory = Some(before.clone());
    listener.persistent = Some(Arc::new(tables().view(&snapshot).unwrap()));
    let roles = listener.roles.as_mut().unwrap();
    roles.bank = Some(crate::scene_roles::Bank {
        scene: roles.garage,
        serializer: 1,
    });

    let mut replacement = grant.clone();
    replacement.id = ItemId::new(99).unwrap();
    grant.state = OWNED;
    grant.owner = ItemId::new(1);
    repo.apply(
        account,
        &Batch {
            id: 91,
            expected_generation: snapshot.generation,
            ops: vec![
                Op::Update(grant),
                Op::Insert(replacement),
                nfs_services::bank::Balance(40).store(),
            ],
        },
        Timestamp(4),
    )
    .unwrap();
    let committed = repo.snapshot(account).unwrap();
    let after = Arc::new(crate::net::items::tests::reload_inventory(
        &jobs, &committed,
    ));
    let persistent = Some(Arc::new(tables().view(&committed).unwrap()));
    let objects = listener.players.objects().snapshot();
    let garage = before.slots.values();
    // Unrelated existing-item edits are rejected without refreshing or queuing.
    let mut bad = (*after).clone();
    bad.items.get_mut(&6).unwrap().sell_price += 1;
    assert_eq!(
        listener.purchased_inventory(garage, persistent.clone(), Arc::new(bad)),
        Err(replication::Error::Shape)
    );
    assert_eq!(listener.inventory.as_ref().unwrap().items, before.items);
    assert!(listener.pending_rpcs.is_empty());
    let notices = listener
        .purchase_notifications(&after, crate::net::items::tests::item_bindings(&jobs))
        .unwrap();
    assert_eq!(notices.len(), 1);
    let notice = nfs_world::items::ChangeNotification::decode(
        &notices[0],
        crate::net::items::tests::item_bindings(&jobs),
    )
    .unwrap();
    assert_eq!(notice.added.roots, [99]);
    assert!(notice.removed.items.is_empty());
    listener
        .purchased_inventory(garage, persistent.clone(), after.clone())
        .unwrap();
    assert_eq!(listener.inventory.as_ref().unwrap().items, after);
    assert_eq!(listener.players.objects().snapshot(), objects);
    assert_eq!(after.items[&1].children, before.items.items[&1].children);
    assert_eq!(after.items[&7].owner, 1);
    assert_eq!(after.items[&7].state, OWNED);
    assert!(
        matches!(listener.pending_rpcs.back(), Some(nfs_world::participants::HostRpc::Bank(balance)) if balance.amount == 40)
    );
    // A committed retry refreshes the same snapshot and never changes the car.
    listener
        .purchased_inventory(garage, persistent, after.clone())
        .unwrap();
    assert_eq!(listener.players.objects().snapshot(), objects);
    assert_eq!(repo.snapshot(account).unwrap(), committed);
    assert!(
        listener
            .purchase_notifications(&after, crate::net::items::tests::item_bindings(&jobs))
            .unwrap()
            .is_empty()
    );
}
