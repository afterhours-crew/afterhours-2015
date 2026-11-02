// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_storage::{
    AccountId, Batch, InventoryRepository, MemoryRepository, Op, SqliteRepository, Timestamp,
};

#[test]
fn private_receipts_reject_stale_reads_after_identical_slot_reuse_and_legacy_rows() {
    let repo = MemoryRepository::default();
    let account = AccountId::from_owned_config([90; 16]).unwrap();
    repo.open(account).unwrap();
    let mut first = None;
    for n in 1..=4 {
        let snapshot = repo.snapshot(account).unwrap();
        let (ops, identity) = append_owned(&snapshot, 3, 7, &[1, 2, 3]).unwrap();
        repo.apply(
            account,
            &Batch {
                id: n,
                expected_generation: snapshot.generation,
                ops,
            },
            Timestamp(n),
        )
        .unwrap();
        first.get_or_insert(identity);
    }
    let saved = repo.snapshot(account).unwrap();
    let old = first.unwrap();
    let current = identities(
        saved.tables.get(&key(TABLE)),
        saved.tables.get(&key(RECEIPTS)),
        3,
    )
    .unwrap();
    assert_eq!(current[1].row, old.row, "all public row values were reused");
    assert_ne!(current[1].generation, old.generation);
    assert!(set_read_owned(&saved, 3, &old, true).unwrap().is_none());
    assert!(
        set_read_owned(
            &saved,
            3,
            &Identity {
                generation: None,
                ..old
            },
            true
        )
        .unwrap()
        .is_none()
    );
    let read = set_read_owned(&saved, 3, &current[1], true)
        .unwrap()
        .unwrap();
    repo.apply(account, &read, Timestamp(8)).unwrap();
    let loaded = repo.snapshot(account).unwrap();
    let rows = rows(loaded.tables.get(&key(TABLE)), 3).unwrap();
    assert!(!rows[0].read);
    assert!(rows[1].read);
    assert_eq!(loaded.tables[&key(RECEIPTS)], saved.tables[&key(RECEIPTS)]);
}

#[test]
fn opaque_attachment_storage_uses_padded_base64_and_rejects_noncanonical_data() {
    for (data, expected) in [
        (b"".as_slice(), ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"fooba", "Zm9vYmE="),
        (b"foobar", "Zm9vYmFy"),
        (&[0, 0x12, 0xab, 0x34, 0xcd], "ABKrNM0="),
    ] {
        assert_eq!(encode(data).unwrap(), expected);
        assert_eq!(decode(expected).unwrap(), data);
    }
    for size in 0..=MAX_DATA {
        let bytes: Vec<_> = (0..size).map(|i| (i * 193 + 17) as u8).collect();
        assert_eq!(decode(&encode(&bytes).unwrap()).unwrap(), bytes);
    }
    assert!(encode(&[0; MAX_DATA + 1]).is_err());
    for malformed in [
        "Zg", "Zg=", "Zg===", "Zg==\n", "Zh==", "Zm9=", "=g==", "Z=g=", "Zg==AAAA", "Zg-_",
    ] {
        assert!(decode(malformed).is_err(), "{malformed}");
    }
    assert!(decode(&"A".repeat(36)).is_err());
}

#[test]
fn repeated_hashes_keep_attachment_identity_and_ring_order_across_multiple_wraps() {
    for capacity in [2u16, 3, 7, 40] {
        let mut table = None;
        for count in 0..capacity * 3 {
            let data = count.to_be_bytes();
            let (next, appended) = append(table.as_ref(), capacity, 0xfe12ab34, &data).unwrap();
            assert_eq!(appended.index, u64::from(count % capacity));
            let retained = rows(Some(&next), capacity).unwrap();
            let start = (count + 1).saturating_sub(capacity - 1);
            assert_eq!(
                retained
                    .iter()
                    .map(|r| u16::from_be_bytes(r.data.clone().try_into().unwrap()))
                    .collect::<Vec<_>>(),
                (start..=count).collect::<Vec<_>>()
            );
            assert!(retained.iter().all(|r| r.hash == 0xfe12ab34 && !r.read));
            assert!(next.rows.len() <= usize::from(capacity));
            assert_eq!(
                next.rows
                    .values()
                    .filter(|r| r.get(&key("Asset")) == Some(&Value::Int(0)))
                    .count(),
                1
            );
            table = Some(next);
        }
    }
}

#[test]
fn reading_uses_the_selected_delivery_and_does_not_retarget_an_evicted_slot() {
    let (table, first) = append(None, 3, 71, &[1]).unwrap();
    let (table, second) = append(Some(&table), 3, 71, &[2]).unwrap();
    let read = set_read(Some(&table), 3, &first, true).unwrap().unwrap();
    let got = rows(Some(&read), 3).unwrap();
    assert!(got[0].read);
    assert!(!got[1].read);
    assert!(set_read(Some(&read), 3, &first, true).unwrap().is_none());
    let unread = set_read(Some(&read), 3, &first, false).unwrap().unwrap();
    assert!(!rows(Some(&unread), 3).unwrap()[0].read);
    let (next, _) = append(Some(&read), 3, 71, &[3]).unwrap();
    assert!(set_read(Some(&next), 3, &first, true).unwrap().is_none());
    let (next, replacement) = append(Some(&next), 3, 71, &[4]).unwrap();
    assert_eq!(replacement.index, first.index);
    assert!(set_read(Some(&next), 3, &first, true).unwrap().is_none());
    assert!(set_read(Some(&next), 3, &second, true).unwrap().is_none());
    assert!(rows(Some(&next), 3).unwrap().iter().all(|r| !r.read));
}

#[test]
fn malformed_cursors_holes_flags_and_payloads_fail_without_mutation() {
    let (good, _) = append(None, 5, 71, &[1, 2, 3]).unwrap();
    for capacity in [0, 1, 513] {
        assert!(rows(None, capacity).is_err());
    }
    for mut bad in [
        good.clone(),
        good.clone(),
        good.clone(),
        good.clone(),
        good.clone(),
        good.clone(),
    ]
    .into_iter()
    .enumerate()
    {
        match bad.0 {
            0 => {
                bad.1.rows.remove(&1);
            }
            1 => {
                bad.1.rows.insert(2, [(key("Asset"), Value::Int(0))].into());
            }
            2 => {
                let row = bad.1.rows.remove(&0).unwrap();
                bad.1.rows.insert(3, row);
            }
            3 => {
                bad.1
                    .rows
                    .get_mut(&0)
                    .unwrap()
                    .insert(key("Read"), Value::Int(2));
            }
            4 => {
                bad.1
                    .rows
                    .get_mut(&0)
                    .unwrap()
                    .insert(key("Data"), Value::String("invalid".into()));
            }
            5 => {
                bad.1.rows.get_mut(&0).unwrap().remove(&key("Asset"));
            }
            _ => unreachable!(),
        }
        let before = bad.1.clone();
        assert!(rows(Some(&bad.1), 5).is_err());
        assert!(append(Some(&bad.1), 5, 72, &[9]).is_err());
        assert_eq!(bad.1, before);
    }
    assert!(append(Some(&good), 5, 0, &[9]).is_err());
    assert!(append(Some(&good), 5, 72, &[]).is_err());
    assert!(append(Some(&good), 5, 72, &[0; 25]).is_err());
    assert_eq!(rows(Some(&good), 5).unwrap().len(), 1);
}

fn account(id: u8) -> AccountId {
    AccountId::from_owned_config([id; 16]).unwrap()
}
fn commit(repo: &dyn InventoryRepository, who: AccountId, id: u64, data: &[u8]) -> Batch {
    let before = repo.snapshot(who).unwrap();
    let (table, _) = append(before.tables.get(&key(TABLE)), 3, 71, data).unwrap();
    let batch = Batch {
        id,
        expected_generation: before.generation,
        ops: vec![Op::SetTable(key(TABLE), table)],
    };
    repo.apply(who, &batch, Timestamp(id)).unwrap();
    batch
}
#[test]
fn transactional_retries_generation_conflicts_and_accounts_do_not_duplicate_delivery() {
    let repo = MemoryRepository::default();
    let a = account(31);
    let b = account(32);
    repo.open(a).unwrap();
    repo.open(b).unwrap();
    let first = commit(&repo, a, 10, &[1]);
    assert!(repo.apply(a, &first, Timestamp(11)).unwrap().replayed);
    assert_eq!(
        rows(repo.snapshot(a).unwrap().tables.get(&key(TABLE)), 3)
            .unwrap()
            .len(),
        1
    );
    assert!(
        rows(repo.snapshot(b).unwrap().tables.get(&key(TABLE)), 3)
            .unwrap()
            .is_empty()
    );
    let mut conflict = first.clone();
    conflict.id = 11;
    let before = repo.snapshot(a).unwrap();
    assert_eq!(
        repo.apply(a, &conflict, Timestamp(12)),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(a).unwrap(), before);
    // The same retry key belongs to another account's independent history.
    repo.apply(b, &first, Timestamp(12)).unwrap();
    assert_eq!(
        rows(repo.snapshot(b).unwrap().tables.get(&key(TABLE)), 3)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn actual_database_reopen_retains_attachment_read_flag_cursor_and_order() {
    let path = std::env::temp_dir().join(format!(
        "nfs-dynamic-message-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let who = account(33);
    let expected = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        repo.open(who).unwrap();
        for i in 1..=5 {
            commit(&repo, who, i, &[i as u8]);
        }
        let snapshot = repo.snapshot(who).unwrap();
        let table = snapshot.tables.get(&key(TABLE));
        let selected = rows(table, 3).unwrap()[0].clone();
        let next = set_read(table, 3, &selected, true).unwrap().unwrap();
        repo.apply(
            who,
            &Batch {
                id: 6,
                expected_generation: snapshot.generation,
                ops: vec![Op::SetTable(key(TABLE), next)],
            },
            Timestamp(6),
        )
        .unwrap();
        repo.snapshot(who).unwrap()
    };
    {
        let reopened = SqliteRepository::open_owned_directory(&path).unwrap();
        assert_eq!(reopened.snapshot(who).unwrap(), expected);
        let snapshot = reopened.snapshot(who).unwrap();
        let restored = rows(snapshot.tables.get(&key(TABLE)), 3).unwrap();
        assert_eq!(
            restored
                .iter()
                .map(|r| (&r.data, r.read))
                .collect::<Vec<_>>(),
            vec![(&vec![4], true), (&vec![5], false)]
        );
        commit(&reopened, who, 7, &[6]);
        let snapshot = reopened.snapshot(who).unwrap();
        assert_eq!(
            rows(snapshot.tables.get(&key(TABLE)), 3)
                .unwrap()
                .iter()
                .map(|r| r.data.clone())
                .collect::<Vec<_>>(),
            vec![vec![5], vec![6]]
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}
