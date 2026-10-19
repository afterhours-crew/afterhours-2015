// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::replication::vehicle::{self, Body, Customization, Initial};

fn world_chassis(listener: &PlayerListener, items: &nfs_world::items::Collection) -> Customization {
    let content = &listener.vehicles.as_ref().unwrap().vehicles;
    let request = crate::vehicle_content::Request {
        items,
        vehicle: 1,
        locator: [1., 2., 3.],
        basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        origin: [0.; 3],
        speed: None,
        connection: Some(3),
        owner: vehicle::Resource {
            words: [10, 0],
            name: b"Local".to_vec(),
        },
        customization_attached: true,
        placement: crate::vehicle_content::Placement::PlayerWorld,
        authority_group: 10,
        authority_components: &[100],
    };
    // A new registry represents a fresh world lifetime, with no garage preview.
    let body = content
        .construct(
            request,
            &mut vehicle::authority::Registry::default(),
            |_| Some(7),
        )
        .unwrap();
    let profile = &content
        .definition(&items.items[&1].definition)
        .unwrap()
        .profile;
    let encoded = body.encode(profile).unwrap();
    let decoded = Body::decode(encoded.span(), profile, true).unwrap();
    assert_eq!(decoded.bits, encoded.len());
    decoded
        .body
        .creation
        .unwrap()
        .fields
        .into_iter()
        .find_map(|field| {
            if let Initial::Chassis(chassis) = field {
                Some(chassis.custom)
            } else {
                None
            }
        })
        .unwrap()
}

fn reset_wire(scene: u16, player: u16, car: u16) -> BitWriter {
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(3, 8)
        .put(scene.into(), 13)
        .put(player.into(), 13)
        .put(car.into(), 13)
        .put(6, 9)
        .put(2, 9)
        .put(13, 32)
        .put(0, 7);
    body
}

#[tokio::test]
async fn free_roam_spawn_uses_equipped_performance_and_tuning_independently_of_dyno_cache() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    let stock = world_chassis(&listener, &listener.inventory.as_ref().unwrap().items);
    assert_eq!(stock.value, 0);
    assert_eq!(stock.tuning[26], 5);
    assert_eq!(stock.tuning[31], 0);

    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(
        accept(
            &mut listener,
            &handling_wire(scene, player, car, &[(26, 7), (30, 10)])
        )
        .unwrap()
    );
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    crate::net::items::tests::deliver_notification(&mut jobs);
    let saved = repo.snapshot(account).unwrap();
    // Reload from the repository instead of reusing the transient edit/preview.
    let items = crate::net::items::tests::reload_inventory(&jobs, &saved);
    let upgraded = world_chassis(&listener, &items);
    assert_eq!(upgraded.value, 63); // installed part contributes 0.25 * 255
    assert_eq!(upgraded.tuning[26], 7);
    assert_eq!(upgraded.tuning[31], 10);
    assert_eq!(saved.tables, before.tables);

    let mut display_only = items.clone();
    nfs_world::items::DynoStats::new([300, 1500, 1500, 200], [2., 3., 8.])
        .unwrap()
        .apply(&mut display_only.items.get_mut(&1).unwrap().derived)
        .unwrap();
    assert_eq!(world_chassis(&listener, &display_only), upgraded);

    // Reset restores performance defaults, preserves a visual replacement and
    // tuning held by the same default item, and keeps purchased spares owned.
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 9)).unwrap());
    assert!(accept(&mut listener, &reset_wire(scene, player, car)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    crate::net::items::tests::deliver_notification(&mut jobs);
    let reset = repo.snapshot(account).unwrap();
    let items = crate::net::items::tests::reload_inventory(&jobs, &reset);
    assert_eq!(&items.items[&1].children[..2], &[6, 8]);
    assert!(items.items[&1].children.contains(&9));
    assert!(!items.items[&1].children.contains(&7));
    assert!(items.items.contains_key(&7));
    let settings = world_chassis(&listener, &items);
    assert_eq!(settings.value, 0);
    assert_eq!(settings.tuning, upgraded.tuning);
    assert_eq!(reset.tables, before.tables);

    assert!(accept(&mut listener, &reset_wire(scene, player, car)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 5, Timestamp(5));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), reset);
}

#[tokio::test]
async fn reset_requires_owned_garage_car_and_begin_cancels_it() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(accept(&mut listener, &reset_wire(scene, player + 1000, car)).is_err());
    assert!(accept(&mut listener, &reset_wire(scene, player, car + 1000)).is_err());
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(accept(&mut listener, &reset_wire(scene, player, car)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(!jobs.working());
    assert_eq!(repo.snapshot(account).unwrap(), before);
    listener.world_cars.insert(car, (0, false));
    assert!(accept(&mut listener, &reset_wire(scene, player, car)).is_err());
}
