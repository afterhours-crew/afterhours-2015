// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::messages::{
    self,
    unlocks::{self, Pending, Trigger},
};
use std::collections::BTreeSet;

fn notifications(excluded: BTreeSet<u32>) -> unlocks::Policy {
    unlocks::Policy::new(
        Some([1; 16]),
        [([2; 16], 202), ([3; 16], 303)].into(),
        [
            (101, Trigger::Categories { mask: 2 }),
            (102, Trigger::Categories { mask: 4 }),
            (103, Trigger::Categories { mask: 6 }),
            (
                201,
                Trigger::Part {
                    include: false,
                    ids: excluded,
                },
            ),
        ]
        .into(),
    )
    .unwrap()
}

fn pending(snapshot: &Snapshot) -> Vec<Pending> {
    unlocks::pending(snapshot.tables.get(&key(unlocks::TABLE))).unwrap()
}

#[test]
fn unlock_notifications_commit_with_rewards_and_survive_failed_and_repeated_writes() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let other = fixture.initialize(&repo, account(2));
    let before = fixture.initialize(&repo, account(1));
    let notifications = notifications(BTreeSet::new());
    let mut policy = fixture.policy();
    policy.notifications = Some(&notifications);
    let batch = policy.prepare(&before, credit(10), 701).unwrap();
    let competing = policy.prepare(&before, credit(30), 702).unwrap();
    assert!(pending(&repo.snapshot(account(1)).unwrap()).is_empty());
    let mut invalid = batch.clone();
    invalid
        .ops
        .push(Op::Remove(nfs_storage::ItemId::new(999).unwrap()));
    assert_eq!(
        repo.apply(account(1), &invalid, Timestamp(3)),
        Err(Error::UnknownItem)
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), before);
    repo.apply(account(1), &batch, Timestamp(4)).unwrap();
    let saved = repo.snapshot(account(1)).unwrap();
    assert_eq!(
        pending(&saved),
        [
            Pending {
                hash: 101,
                signal: saved.generation,
                part: None
            },
            Pending {
                hash: 201,
                signal: saved.generation,
                part: Some(202)
            },
        ]
    );
    assert_eq!(fixture.scores(&saved), credit(10));
    assert_eq!(mask(&saved, &fixture.inventory), 0x8000_0003);
    assert_eq!(saved.items.len(), 2);
    let view = fixture.tables.view(&saved).unwrap();
    assert_eq!(
        unlocks::pending(view.owned(unlocks::TABLE)).unwrap(),
        pending(&saved)
    );
    assert!(
        !view.tables().contains_key(&key(unlocks::TABLE)),
        "private intent table is not a client table"
    );
    assert!(
        repo.apply(account(1), &batch, Timestamp(5))
            .unwrap()
            .replayed
    );
    assert_eq!(
        repo.apply(account(1), &competing, Timestamp(6)),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
    assert_eq!(repo.snapshot(account(2)).unwrap(), other);
    let noop = policy.prepare(&saved, credit(1), 703).unwrap();
    repo.apply(account(1), &noop, Timestamp(7)).unwrap();
    assert_eq!(
        pending(&repo.snapshot(account(1)).unwrap()),
        pending(&saved)
    );
}

#[test]
fn multi_threshold_grants_coalesce_and_filtered_updates_sample_the_latest_part() {
    for exclude_last in [false, true] {
        for split in [false, true] {
            let fixture = Fixture::new();
            let repo = MemoryRepository::default();
            let before = fixture.initialize(&repo, account(1));
            let notifications = notifications(if exclude_last {
                [303].into()
            } else {
                BTreeSet::new()
            });
            let mut policy = fixture.policy();
            policy.notifications = Some(&notifications);
            let first = policy
                .prepare(&before, credit(if split { 10 } else { 30 }), 711)
                .unwrap();
            repo.apply(account(1), &first, Timestamp(3)).unwrap();
            let mid = repo.snapshot(account(1)).unwrap();
            if split {
                let second = policy.prepare(&mid, credit(20), 712).unwrap();
                repo.apply(account(1), &second, Timestamp(4)).unwrap();
            }
            let saved = repo.snapshot(account(1)).unwrap();
            let rows = pending(&saved);
            assert_eq!(
                rows.iter().map(|p| p.hash).collect::<Vec<_>>(),
                [101, 102, 103, 201]
            );
            assert_eq!(rows[0].signal, mid.generation);
            assert_eq!(rows[1].signal, saved.generation);
            assert_eq!(
                rows[2].signal, saved.generation,
                "all-mask success fires once at completion"
            );
            assert_eq!(rows[3].part, Some(303));
            assert_eq!(
                rows[3].signal,
                if exclude_last {
                    mid.generation
                } else {
                    saved.generation
                }
            );
        }
    }
}

#[test]
fn owned_copy_suppresses_first_unlock_event_but_still_changes_pending_attachment() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let before = fixture.initialize(&repo, account(1));
    let notifications = notifications(BTreeSet::new());
    let mut policy = fixture.policy();
    policy.notifications = Some(&notifications);
    let first = policy.prepare(&before, credit(10), 721).unwrap();
    repo.apply(account(1), &first, Timestamp(3)).unwrap();
    let mid = repo.snapshot(account(1)).unwrap();
    let mut owned = mid
        .items
        .values()
        .find(|p| p.definition.0 == [2; 16])
        .unwrap()
        .clone();
    let (id, allocation) = crate::purchase::reserve_ids(&mid, 1).unwrap();
    owned.id = nfs_storage::ItemId::new(id).unwrap();
    owned.definition = nfs_storage::DefinitionGuid([3; 16]);
    owned.state = items::OWNED;
    repo.apply(
        account(1),
        &Batch {
            id: 722,
            expected_generation: mid.generation,
            ops: vec![allocation, Op::Insert(owned)],
        },
        Timestamp(4),
    )
    .unwrap();
    let before_second = repo.snapshot(account(1)).unwrap();
    let second = policy.prepare(&before_second, credit(20), 723).unwrap();
    repo.apply(account(1), &second, Timestamp(5)).unwrap();
    let saved = repo.snapshot(account(1)).unwrap();
    let part = pending(&saved).into_iter().find(|p| p.hash == 201).unwrap();
    assert_eq!(
        part,
        Pending {
            hash: 201,
            signal: mid.generation,
            part: Some(303)
        }
    );
    assert_eq!(
        saved
            .items
            .values()
            .filter(|i| i.definition.0 == [3; 16])
            .count(),
        2
    );
}

#[test]
fn sqlite_restart_retains_pending_and_delivery_is_atomic_stale_safe_and_retry_safe() {
    let path = std::env::temp_dir().join(format!(
        "nfs-unlock-messages-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let fixture = Fixture::new();
    let notifications = notifications([303].into());
    let mut policy = fixture.policy();
    policy.notifications = Some(&notifications);
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    let before = fixture.initialize(&repo, account(1));
    let credit_batch = policy.prepare(&before, credit(10), 731).unwrap();
    repo.apply(account(1), &credit_batch, Timestamp(3)).unwrap();
    let saved = repo.snapshot(account(1)).unwrap();
    let old_part = pending(&saved)
        .into_iter()
        .find(|p| p.part.is_some())
        .unwrap();
    drop(repo);
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
    assert!(
        repo.apply(account(1), &credit_batch, Timestamp(4))
            .unwrap()
            .replayed
    );
    let another = policy.prepare(&saved, credit(20), 732).unwrap();
    repo.apply(account(1), &another, Timestamp(5)).unwrap();
    let ready = repo.snapshot(account(1)).unwrap();
    assert!(
        unlocks::deliver(&ready, old_part, messages::UNDERGROUND, 4, &[1])
            .unwrap()
            .is_none()
    );
    for row in pending(&ready) {
        let current = repo.snapshot(account(1)).unwrap();
        // Opaque synthetic attachment: the service does not implement the wire codec.
        let bytes: &[u8] = if row.part.is_some() { &[1, 2, 3] } else { &[] };
        let batch = unlocks::deliver(&current, row, messages::UNDERGROUND, 4, bytes)
            .unwrap()
            .unwrap();
        let mut invalid = batch.clone();
        invalid
            .ops
            .push(Op::Remove(nfs_storage::ItemId::new(999).unwrap()));
        assert_eq!(
            repo.apply(account(1), &invalid, Timestamp(6)),
            Err(Error::UnknownItem)
        );
        assert_eq!(repo.snapshot(account(1)).unwrap(), current);
        repo.apply(account(1), &batch, Timestamp(7)).unwrap();
        let delivered = repo.snapshot(account(1)).unwrap();
        assert!(
            repo.apply(account(1), &batch, Timestamp(8))
                .unwrap()
                .replayed
        );
        assert!(
            unlocks::deliver(&delivered, row, messages::UNDERGROUND, 4, bytes)
                .unwrap()
                .is_none()
        );
        assert_eq!(delivered.items, ready.items);
        assert_eq!(fixture.scores(&delivered), fixture.scores(&ready));
    }
    let final_state = repo.snapshot(account(1)).unwrap();
    assert!(pending(&final_state).is_empty());
    assert_eq!(
        messages::rows(&final_state.tables, messages::UNDERGROUND)
            .unwrap()
            .iter()
            .map(|r| r.hash)
            .collect::<Vec<_>>(),
        [101, 102, 103]
    );
    let dynamic =
        messages::dynamic::rows(final_state.tables.get(&key(messages::dynamic::TABLE)), 4).unwrap();
    assert_eq!(dynamic.len(), 1);
    assert_eq!(dynamic[0].hash, 201);
    assert_eq!(dynamic[0].data, [1, 2, 3]);
    drop(repo);
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    assert_eq!(repo.snapshot(account(1)).unwrap(), final_state);
    drop(repo);
    std::fs::remove_dir_all(path).unwrap();
}
