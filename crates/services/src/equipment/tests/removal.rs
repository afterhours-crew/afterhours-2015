// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[test]
fn removal_uses_slot_retains_owned_spares_and_survives_restart() {
    let directory = std::env::temp_dir().join(format!(
        "nfs-remove-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let account = who(1);
    let catalog = catalog();
    let offers = Offers::new([[3; 16]]).unwrap();
    let (pending, saved) = {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        let before = setup(&repo, account);
        let other = setup(&repo, who(2));
        let items = inventory::collection(&before, catalog.bindings()).unwrap();
        let pending = prepare_vehicle(account, before.generation, &items, 10, &catalog)
            .unwrap()
            // A selected spare represents its class slot, not its current id.
            .without_part(&items, 12, &catalog, &offers)
            .unwrap();
        assert!(
            pending.preview(&items).unwrap().items[&10]
                .children
                .is_empty()
        );
        assert_eq!(repo.snapshot(account).unwrap(), before);
        let ordered = pending
            .clone()
            .with_part(&items, 12, &catalog, &offers)
            .unwrap();
        assert_eq!(ordered.preview(&items).unwrap().items[&10].children, [12]);
        let ordered = ordered.without_part(&items, 11, &catalog, &offers).unwrap();
        assert!(
            ordered.preview(&items).unwrap().items[&10]
                .children
                .is_empty()
        );
        assert_eq!(
            commit(&repo, who(2), &catalog, &pending, Timestamp(3)),
            Err(Error::Ownership)
        );
        let saved = commit(&repo, account, &catalog, &pending, Timestamp(3)).unwrap();
        for id in [11, 12] {
            assert_eq!(
                saved.items[&ItemId::new(id).unwrap()],
                before.items[&ItemId::new(id).unwrap()]
            );
        }
        assert_eq!(saved.tables, before.tables);
        assert_eq!(repo.snapshot(who(2)).unwrap(), other);
        let current = inventory::collection(&saved, catalog.bindings()).unwrap();
        let newer = prepare_vehicle(account, saved.generation, &current, 10, &catalog)
            .unwrap()
            .with_part(&current, 12, &catalog, &offers)
            .unwrap();
        let future = commit(&repo, account, &catalog, &newer, Timestamp(4)).unwrap();
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(5)),
            Err(Error::Conflict)
        );
        // Preserve the future state for a true reopen; no stale reset is allowed.
        (newer, future)
    };
    {
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), saved);
        assert_eq!(
            commit(&repo, account, &catalog, &pending, Timestamp(6)).unwrap(),
            saved
        );
        let current = inventory::collection(&saved, catalog.bindings()).unwrap();
        let removed = prepare_vehicle(account, saved.generation, &current, 10, &catalog)
            .unwrap()
            .without_part(&current, 12, &catalog, &offers)
            .unwrap();
        let stored = commit(&repo, account, &catalog, &removed, Timestamp(7)).unwrap();
        drop(repo);
        let repo = nfs_storage::SqliteRepository::open_owned_directory(&directory).unwrap();
        assert_eq!(repo.open(account).unwrap(), stored);
        assert_eq!(
            commit(&repo, account, &catalog, &removed, Timestamp(8)).unwrap(),
            stored
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
