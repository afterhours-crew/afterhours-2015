// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::persistent::Catalog;
use nfs_storage::{
    AccountId, Batch, GarageSlots, InventoryRepository, MemoryRepository, Op, SqliteRepository,
    Timestamp,
};
use serde_json::json;

fn catalog() -> Catalog {
    let mut content = crate::persistent::tests::content();
    for (table, names) in [
        (
            "RepValuesTable",
            vec![
                "RepScore",
                "RepLevel",
                "BuildScore",
                "CrewScore",
                "OutlawScore",
                "SpeedScore",
                "StyleScore",
            ],
        ),
        (
            "GameplayStatsTable",
            vec![
                "DistanceDriven",
                "CashEarned",
                "NumberOfEventsCompleted",
                "LargestFineEscaped",
                "LargestFineBusted",
                "TotalTimePlayed",
                "FavoriteVehicleId",
                "TopSpeed",
                "DistanceDrifted",
            ],
        ),
        (
            "SpeedListStatsTable",
            vec![
                "Started", "Finished", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8",
            ],
        ),
        (
            "PrestigeMedalTable",
            vec![
                "BuildMedal",
                "CrewMedal",
                "OutlawMedal",
                "SpeedMedal",
                "StyleMedal",
                "FinalEventMedal",
            ],
        ),
        ("ActivitiesTable", vec!["Collected"]),
        ("CollectiblesTable", vec!["Collected"]),
        ("ProgressionObjective", vec!["Active", "Completed"]),
    ] {
        let row = content["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["name"] == table)
            .unwrap();
        row["columns"] = json!(
            names
                .into_iter()
                .map(|n| json!({"key":key(n),"name":n,"value_type":"int","default":"0"}))
                .collect::<Vec<_>>()
        );
    }
    Catalog::from_json(&content).unwrap()
}
fn initialize(repo: &dyn InventoryRepository, account: AccountId) -> Snapshot {
    repo.open(account).unwrap();
    repo.apply(
        account,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops: vec![Op::SetGarage(GarageSlots::default())],
        },
        Timestamp(1),
    )
    .unwrap();
    let (snapshot, _) = catalog()
        .ensure_loaded(repo, account, repo.snapshot(account).unwrap(), Timestamp(2))
        .unwrap();
    ensure_empty(repo, account, snapshot, Timestamp(3)).unwrap()
}
fn current(snapshot: &Snapshot) -> Current {
    Current::from_snapshot(
        snapshot,
        catalog().view(snapshot).unwrap(),
        &Thresholds::new(vec![0, 10, 30]).unwrap(),
    )
    .unwrap()
}
fn query(persona: i64) -> Vec<u8> {
    let body = StatsAndAwardsRequest {
        blaze_id: Some(persona),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    nfs_fire2::encode(
        nfs_fire2::Frame {
            fields: nfs_fire2::Fields {
                routing_a: COMPONENT,
                routing_b: GET_STATS_AND_AWARDS,
                correlation: 79,
                ..Default::default()
            },
            metadata: &[],
            body: &body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn response(bytes: &[u8]) -> StatsAndAwardsResponse<'_> {
    let f = nfs_fire2::decode(bytes, frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(
        (
            f.fields.routing_a,
            f.fields.routing_b,
            f.fields.category,
            f.fields.correlation
        ),
        (2050, 71, 1, 79)
    );
    assert!(f.metadata.is_empty());
    let r = StatsAndAwardsResponse::decode(f.body, body_limits()).unwrap();
    assert_eq!(r.unknown_field_count(), 0);
    r
}
fn row(values: &[(&str, i32)]) -> Row {
    values
        .iter()
        .map(|&(name, value)| (key(name), Value::Int(value)))
        .collect()
}
fn put(snapshot: &mut Snapshot, table: &str, id: u64, values: &[(&str, i32)]) {
    snapshot
        .tables
        .get_mut(&key(table))
        .unwrap()
        .rows
        .insert(id, row(values));
}
fn populated(mut snapshot: Snapshot) -> Snapshot {
    put(
        &mut snapshot,
        "RepValuesTable",
        0,
        &[
            ("RepScore", 15),
            ("RepLevel", 99),
            ("BuildScore", 1),
            ("CrewScore", 2),
            ("OutlawScore", 3),
            ("SpeedScore", 4),
            ("StyleScore", 5),
        ],
    );
    put(
        &mut snapshot,
        "GameplayStatsTable",
        0,
        &[
            ("DistanceDriven", 1234),
            ("CashEarned", 567),
            ("LargestFineEscaped", 12),
            ("LargestFineBusted", 23),
            ("TotalTimePlayed", 45),
            ("FavoriteVehicleId", -17),
            ("TopSpeed", 123),
            ("DistanceDrifted", 98),
        ],
    );
    put(
        &mut snapshot,
        "SpeedListStatsTable",
        0,
        &[("Started", 4), ("Finished", 3), ("P1", 2), ("P8", 1)],
    );
    put(
        &mut snapshot,
        "PrestigeMedalTable",
        0,
        &[("BuildMedal", 2), ("FinalEventMedal", -1)],
    );
    put(&mut snapshot, "ActivitiesTable", 91, &[("Collected", 1)]);
    put(&mut snapshot, "CollectiblesTable", 21, &[("Collected", 0)]);
    put(
        &mut snapshot,
        "ProgressionObjective",
        u64::MAX,
        &[("Active", 1), ("Completed", 0)],
    );
    let record = Record {
        name: "local-record".into(),
        screenshot: u64::MAX,
        modified: 101,
    };
    let state = State {
        activities: BTreeMap::from([(91, record.clone())]),
        events: vec![(
            44,
            Event {
                attempts: 2,
                position: 1,
                record,
                kind: "local-event".into(),
            },
        )],
        objective_times: BTreeMap::from([(u64::MAX, 102)]),
        rep_modified: 103,
        kickbacks: [1, 2, 3, 4],
        entitlements: BTreeMap::from([(-1, false), (5, true)]),
        ..Default::default()
    };
    for op in state.operations().unwrap() {
        if let Op::SetTable(id, table) = op {
            snapshot.tables.insert(id, table);
        }
    }
    snapshot
}
#[test]
fn local_empty_account_has_empty_collections_and_shared_reputation() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([1; 16]).unwrap();
    let snapshot = initialize(&repo, who);
    let first = current(&snapshot).reply(&query(123), 123).unwrap();
    let r = response(&first);
    assert_eq!(r.blaze_id, Some(123));
    assert!(r.activities.unwrap().0.is_empty());
    assert!(r.collectibles.unwrap().0.is_empty());
    assert!(r.race_events.unwrap().0.is_empty());
    assert!(r.progression_objectives.unwrap().0.is_empty());
    assert!(r.entitlements.unwrap().0.is_empty());
    assert_eq!(r.rep_scores.unwrap().rep_level, Some(1));
    assert_eq!(r.general_stats.unwrap().cash_earned, Some(0));
    assert_eq!(
        r.speed_list_stats.unwrap().positions_breakdown.unwrap().0,
        (1..=8).map(|p| (p, 0)).collect::<Vec<_>>()
    );
    assert_eq!(
        ensure_empty(&repo, who, snapshot.clone(), Timestamp(90)).unwrap(),
        snapshot
    );
    assert_eq!(current(&snapshot).reply(&query(123), 123).unwrap(), first);
}
#[test]
fn projections_include_current_tables_and_preserve_unsigned_identifiers() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([2; 16]).unwrap();
    let snapshot = populated(initialize(&repo, who));
    let bytes = current(&snapshot).reply(&query(234), 234).unwrap();
    let r = response(&bytes);
    let stats = r.general_stats.unwrap();
    assert_eq!(
        (
            stats.biggest_fine_escaped,
            stats.biggest_fine,
            stats.cash_earned,
            stats.distance_driven,
            stats.distance_drifted,
            stats.favorite_car_id,
            stats.time_played,
            stats.top_speed
        ),
        (
            Some(12),
            Some(23),
            Some(567),
            Some(1234),
            Some(98),
            Some((-17i32) as u32),
            Some(45),
            Some(123)
        )
    );
    let rep = r.rep_scores.unwrap();
    assert_eq!(
        (
            rep.rep_level,
            rep.rep_score,
            rep.style_score,
            rep.last_modified
        ),
        (Some(2), Some(15), Some(5), Some(103))
    );
    let activity = &r.activities.as_ref().unwrap().0[0];
    assert_eq!(
        (
            activity.collected,
            activity.persistence_key,
            activity.record_name,
            activity.screenshot_id
        ),
        (
            Some(true),
            Some(91),
            Some(b"local-record".as_slice()),
            Some(u64::MAX)
        )
    );
    assert_eq!(
        r.progression_objectives.unwrap().0[0].persistence_key,
        Some(u64::MAX)
    );
    assert_eq!(r.race_events.unwrap().0[0].attempts, Some(2));
    assert_eq!(r.prestige_medal_stats.unwrap().final_event, Some(-1));
    assert_eq!(r.entitlements.unwrap().0, [(-1, false), (5, true)]);
    assert_eq!(r.kickbacks.unwrap().screenshot_count, Some(4));
    assert_eq!(
        r.speed_list_stats.unwrap().positions_breakdown.unwrap().0[7],
        (8, 1)
    );
}
#[test]
fn queries_enforce_identity_framing_fields_and_correlation() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([3; 16]).unwrap();
    let view = current(&initialize(&repo, who));
    let q = query(123);
    for split in 0..q.len() {
        assert!(view.reply(&q[..split], 123).is_err());
    }
    assert_eq!(
        view.reply(&[q.clone(), q.clone()].concat(), 123),
        Err(Error::Ineligible)
    );
    for id in [0, -1, 124] {
        assert_eq!(view.reply(&query(id), 123), Err(Error::Ineligible));
    }
    let f = nfs_fire2::decode(&q, frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    for case in 0..5 {
        let mut fields = f.fields;
        match case {
            0 => fields.category = 1,
            1 => fields.slot = 1,
            2 => fields.reserved[0] = 1,
            3 => fields.routing_b = 26,
            _ => fields.routing_a = 9,
        }
        let invalid = nfs_fire2::encode(nfs_fire2::Frame { fields, ..f }, frame_limits()).unwrap();
        assert_eq!(view.reply(&invalid, 123), Err(Error::Ineligible));
    }
    let mut body = f.body.to_vec();
    body.extend_from_slice(&[0xaa, 0xaa, 0xaa, 0, 0]);
    let unknown = nfs_fire2::encode(nfs_fire2::Frame { body: &body, ..f }, frame_limits()).unwrap();
    assert!(view.reply(&unknown, 123).is_err());
}
#[test]
fn invalid_or_incomplete_state_never_falls_back_to_a_snapshot() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([4; 16]).unwrap();
    let base = initialize(&repo, who);
    let mut changed = base.clone();
    changed.generation += 1;
    assert!(
        Current::from_snapshot(
            &changed,
            catalog().view(&base).unwrap(),
            &Thresholds::new(vec![0]).unwrap()
        )
        .is_err()
    );
    for (table, column, value) in [
        ("RepValuesTable", "RepScore", -1),
        ("GameplayStatsTable", "CashEarned", -1),
        ("ActivitiesTable", "Collected", 2),
    ] {
        let mut s = base.clone();
        put(&mut s, table, 0, &[(column, value)]);
        assert_eq!(current(&s).reply(&query(123), 123), Err(Error::State));
    }
    let mut s = base.clone();
    s.tables.remove(&key("AfterhoursAwardEntitlements"));
    assert!(State::load(&s).is_err());
    assert!(ensure_empty(&repo, who, s, Timestamp(4)).is_err());
    let mut state = State::default();
    state.activities.insert(99, Record::default());
    let mut s = base.clone();
    for op in state.operations().unwrap() {
        if let Op::SetTable(id, t) = op {
            s.tables.insert(id, t);
        }
    }
    assert_eq!(current(&s).reply(&query(123), 123), Err(Error::State));
    state.activities.get_mut(&99).unwrap().name = "x".repeat(1024);
    assert!(state.operations().is_err());
    state.activities.clear();
    state.objective_times.extend((0..513).map(|n| (n, 0)));
    assert!(state.operations().is_err());
    let mut bad = base;
    bad.tables
        .get_mut(&key("AfterhoursAwardsMeta"))
        .unwrap()
        .rows
        .insert(1, Row::new());
    assert!(State::load(&bad).is_err());
}
#[test]
fn updates_are_atomic_retry_safe_isolated_and_survive_sqlite_restart() {
    let path = std::env::temp_dir().join(format!(
        "nfs-awards-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let who = AccountId::from_owned_config([5; 16]).unwrap();
    let other = AccountId::from_owned_config([6; 16]).unwrap();
    let expected;
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let initial = initialize(&repo, who);
        let other_initial = initialize(&repo, other);
        let target = populated(initial.clone());
        let ops = target
            .tables
            .into_iter()
            .map(|(id, t)| Op::SetTable(id, t))
            .collect();
        let batch = Batch {
            id: 10,
            expected_generation: initial.generation,
            ops,
        };
        let result = repo.apply(who, &batch, Timestamp(8)).unwrap();
        assert!(!result.replayed);
        assert!(repo.apply(who, &batch, Timestamp(9)).unwrap().replayed);
        let stale = Batch { id: 11, ..batch };
        assert_eq!(
            repo.apply(who, &stale, Timestamp(10)),
            Err(nfs_storage::Error::Conflict)
        );
        expected = current(&repo.snapshot(who).unwrap())
            .reply(&query(123), 123)
            .unwrap();
        assert_eq!(repo.snapshot(other).unwrap(), other_initial);
    }
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        repo.open(who).unwrap();
        let saved = repo.snapshot(who).unwrap();
        assert_eq!(current(&saved).reply(&query(123), 123).unwrap(), expected);
        assert_eq!(
            ensure_empty(&repo, who, saved.clone(), Timestamp(99)).unwrap(),
            saved
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn event_record_order_survives_storage_without_sorting_by_identifier() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([7; 16]).unwrap();
    let mut snapshot = initialize(&repo, who);
    let mut state = State {
        events: vec![
            (91, Event::default()),
            (3, Event::default()),
            (44, Event::default()),
        ],
        ..Default::default()
    };
    for op in state.operations().unwrap() {
        if let Op::SetTable(id, table) = op {
            snapshot.tables.insert(id, table);
        }
    }
    assert_eq!(State::load(&snapshot).unwrap(), state);
    let bytes = current(&snapshot).reply(&query(123), 123).unwrap();
    assert_eq!(
        response(&bytes)
            .race_events
            .unwrap()
            .0
            .iter()
            .map(|e| e.event_id.unwrap())
            .collect::<Vec<_>>(),
        [91, 3, 44]
    );
    state.events.push((3, Event::default()));
    assert!(state.operations().is_err());
    let rows = &mut snapshot
        .tables
        .get_mut(&key("AfterhoursRaceRecords"))
        .unwrap()
        .rows;
    let row = rows.remove(&1).unwrap();
    rows.insert(5, row);
    assert!(State::load(&snapshot).is_err());
}
