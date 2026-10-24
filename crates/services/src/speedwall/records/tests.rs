// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{Batch, InventoryRepository, MemoryRepository, SqliteRepository, Timestamp};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn result(time_ms: u32, rep: i32, vehicle: &str) -> Sprint {
    Sprint {
        time_ms,
        rep,
        vehicle: vehicle.into(),
    }
}
fn commit(repo: &dyn InventoryRepository, who: AccountId, id: u64, event: u32, result: &Sprint) {
    let snapshot = repo.snapshot(who).unwrap();
    let batch = Batch {
        id,
        expected_generation: snapshot.generation,
        ops: vec![result.prepare(&snapshot, event).unwrap()],
    };
    repo.apply(who, &batch, Timestamp(id)).unwrap();
    // The exact retry must not increment attempts or change either best.
    assert!(
        repo.apply(who, &batch, Timestamp(id + 100))
            .unwrap()
            .replayed
    );
}
fn verify(repo: &dyn InventoryRepository) {
    let a = account(1);
    let b = account(2);
    repo.open(a).unwrap();
    repo.open(b).unwrap();
    commit(repo, a, 1, 10, &result(100_000, 12_000, "CAR_A"));
    commit(repo, a, 2, 10, &result(90_000, 10_000, "CAR_B"));
    commit(repo, a, 3, 10, &result(120_000, 13_000, "CAR_C"));
    commit(repo, a, 4, 11, &result(60_000, 5_000, "CAR_D"));
    let current = Current::from_snapshot(a, &repo.snapshot(a).unwrap()).unwrap();
    assert_eq!(current.rows.len(), 4);
    let row = &current.rows[0];
    assert_eq!(row.speed_wall_id, 10);
    assert_eq!(row.integers, vec![("Attempts".into(), 3)]);
    assert_eq!(
        row.floats,
        vec![
            ("REP".into(), 13_000f32.to_bits()),
            ("Time".into(), 90_000f32.to_bits())
        ]
    );
    assert_eq!(
        row.strings,
        Some(vec![
            ("SecondaryVehicle".into(), "CAR_B".into()),
            ("Vehicle".into(), "CAR_C".into())
        ])
    );
    assert!(
        Current::from_snapshot(b, &repo.snapshot(b).unwrap())
            .unwrap()
            .rows
            .is_empty()
    );
}
#[test]
fn independent_bests_and_retry_safe_attempts_are_account_and_event_owned() {
    verify(&MemoryRepository::default());
}
#[test]
fn sqlite_records_survive_repository_reopen() {
    let folder = std::env::temp_dir().join(format!(
        "nfs-speedwall-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&folder).unwrap();
    let repo = SqliteRepository::open_owned_directory(&folder).unwrap();
    verify(&repo);
    drop(repo);
    let repo = SqliteRepository::open_owned_directory(&folder).unwrap();
    let current = Current::from_snapshot(account(1), &repo.snapshot(account(1)).unwrap()).unwrap();
    assert_eq!(current.rows[0].integers[0].1, 3);
    drop(repo);
    std::fs::remove_dir_all(folder).unwrap();
}
#[test]
fn imported_bests_remain_and_repeated_refresh_does_not_recount() {
    let repo = MemoryRepository::default();
    let who = account(3);
    repo.open(who).unwrap();
    commit(&repo, who, 1, 10, &result(90_000, 10_000, "NEW"));
    let current = Current::from_snapshot(who, &repo.snapshot(who).unwrap()).unwrap();
    let mut base = current.rows[0].clone();
    base.integers[0].1 = 7;
    base.floats[0].1 = 20_000f32.to_bits();
    base.floats[1].1 = 100_000f32.to_bits();
    base.strings.as_mut().unwrap()[1].1 = "OLD".into();
    let state = super::super::State::new(who, vec![base]).unwrap();
    let first = state.with_current(&current).unwrap();
    let second = state.with_current(&current).unwrap();
    assert_eq!(first.rows(), second.rows());
    assert_eq!(first.rows()[0].integers[0].1, 8);
    assert_eq!(first.rows()[0].floats[0].1, 20_000f32.to_bits());
    assert_eq!(first.rows()[0].floats[1].1, 90_000f32.to_bits());
    assert_eq!(first.rows()[0].strings.as_ref().unwrap()[1].1, "OLD");
    let other = super::super::State::new(account(4), vec![]).unwrap();
    assert!(other.with_current(&current).is_err());
}
#[test]
fn corrupt_records_invalid_values_and_table_bound_fail_without_mutation() {
    let mut snapshot = Snapshot::default();
    assert!(result(0, 1, "CAR").prepare(&snapshot, 1).is_err());
    assert!(result(1, -1, "CAR").prepare(&snapshot, 1).is_err());
    assert!(result(1, 1, "BAD\0CAR").prepare(&snapshot, 1).is_err());
    snapshot.tables.insert(
        key(TABLE),
        Table {
            rows: BTreeMap::from([(1, BTreeMap::new())]),
        },
    );
    assert!(result(1, 1, "CAR").prepare(&snapshot, 1).is_err());
    assert!(Current::from_snapshot(account(1), &snapshot).is_err());
    snapshot.tables.clear();
    for event in 0..MAX_ROWS / 2 {
        let Op::SetTable(key, value) = result(1, 1, "CAR")
            .prepare(&snapshot, event as u32)
            .unwrap()
        else {
            panic!("table")
        };
        snapshot.tables.insert(key, value);
    }
    let before = snapshot.clone();
    assert!(result(1, 1, "CAR").prepare(&snapshot, 999).is_err());
    assert_eq!(snapshot, before);
}
