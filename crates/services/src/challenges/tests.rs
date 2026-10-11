// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{
    AccountId, Batch, InventoryRepository, MemoryRepository, SqliteRepository, Timestamp,
};
use serde_json::{Value as Json, json};

fn content() -> Json {
    json!({"version":1,"build_sha256":crate::SUPPORTED_BUILD_SHA256,"day_id":7,"monthly_start":-11,
        "awards":[{"challenge_id":9,"items":[{"id":u64::MAX,"item_guid":"local-item","kind":u32::MAX,"unlock_value":3}]}],
        "ranks":[{"id":1,"unlock":2}],"challenges":[{"id":9,"car_id":2,"count_one":3,"count_two":4,"day":"local-day","event_id":5,"kind":6,"weekly":false,"type_override_id":0}]})
}
fn query(persona: i64, debug: i64, force: bool) -> Vec<u8> {
    let body = GetDailyChallengesRequest {
        blaze_id: Some(persona),
        debug_start_day: Some(debug),
        force_debug_start_day: Some(force),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    nfs_fire2::encode(
        nfs_fire2::Frame {
            fields: nfs_fire2::Fields {
                routing_a: COMPONENT,
                routing_b: GET_DAILY_CHALLENGES,
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
fn response(bytes: &[u8]) -> GeneratedChallengesDataResponse<'_> {
    let frame = nfs_fire2::decode(bytes, frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(frame.fields.category, 1);
    assert_eq!(frame.fields.correlation, 79);
    GeneratedChallengesDataResponse::decode(frame.body, body_limits()).unwrap()
}
fn initialize(repo: &dyn InventoryRepository, account: AccountId) -> Snapshot {
    let snapshot = repo.open(account).unwrap();
    ensure_empty(repo, account, snapshot, Timestamp(1)).unwrap()
}
fn populated() -> State {
    State {
        progress: BTreeMap::from([
            (
                (7, 9),
                Progress {
                    complete: true,
                    count_one: 3,
                    count_two: 4,
                },
            ),
            (
                (8, 9),
                Progress {
                    complete: false,
                    count_one: 1,
                    count_two: 0,
                },
            ),
        ]),
        obtained: BTreeSet::from([(7, 9, u64::MAX)]),
        monthly_ranks: BTreeMap::from([(-11, 1)]),
    }
}

#[test]
fn reads_current_account_and_explicit_period_without_grants_or_rotation() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([1; 16]).unwrap();
    let snapshot = initialize(&repo, who);
    let current = Current::from_snapshot(&snapshot).unwrap();
    let catalog = Catalog::from_json(&content()).unwrap();
    let empty = current.reply(&catalog, &query(123, 0, false), 123).unwrap();
    assert_eq!(
        current.reply(&catalog, &query(123, 0, false), 123).unwrap(),
        empty
    );
    assert_eq!(
        response(&empty).progress.unwrap().0[0].current_count_one,
        Some(0)
    );
    assert_eq!(
        response(&empty).awards.unwrap().0[0].1[0].obtained,
        Some(false)
    );
    assert_eq!(repo.snapshot(who).unwrap(), snapshot);
    repo.apply(
        who,
        &Batch {
            id: 10,
            expected_generation: snapshot.generation,
            ops: populated().operations().unwrap(),
        },
        Timestamp(2),
    )
    .unwrap();
    let updated = Current::from_snapshot(&repo.snapshot(who).unwrap()).unwrap();
    assert!(updated.generation() > current.generation());
    let bytes = updated.reply(&catalog, &query(123, 0, false), 123).unwrap();
    assert_eq!(response(&bytes).progress.unwrap().0[0].complete, Some(true));
    assert_eq!(
        response(&bytes).awards.unwrap().0[0].1[0].obtained,
        Some(true)
    );
    assert_eq!(response(&bytes).monthly_rank, Some(1));
    let mut next = content();
    next["day_id"] = json!(8);
    let next = Catalog::from_json(&next).unwrap();
    let bytes = updated.reply(&next, &query(123, 0, false), 123).unwrap();
    assert_eq!(
        response(&bytes).progress.unwrap().0[0].current_count_one,
        Some(1)
    );
    assert_eq!(
        response(&bytes).awards.unwrap().0[0].1[0].obtained,
        Some(false)
    );
    assert_eq!(
        State::load(&repo.snapshot(who).unwrap()).unwrap(),
        populated()
    );
}

#[test]
fn identity_debug_shapes_and_frame_boundaries_fail_without_success() {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([2; 16]).unwrap();
    let current = Current::from_snapshot(&initialize(&repo, who)).unwrap();
    let catalog = Catalog::from_json(&content()).unwrap();
    for wire in [
        query(124, 0, false),
        query(123, 1, false),
        query(123, 0, true),
    ] {
        assert!(current.reply(&catalog, &wire, 123).is_err());
    }
    let wire = query(123, 0, false);
    for end in 0..wire.len() {
        assert!(current.reply(&catalog, &wire[..end], 123).is_err());
    }
    assert!(
        current
            .reply(&catalog, &[wire.clone(), wire.clone()].concat(), 123)
            .is_err()
    );
    for persona in [0, -1] {
        assert!(current.reply(&catalog, &wire, persona).is_err());
    }
    let f = nfs_fire2::decode(&wire, frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    for fields in [
        nfs_fire2::Fields {
            category: 1,
            ..f.fields
        },
        nfs_fire2::Fields {
            slot: 1,
            ..f.fields
        },
        nfs_fire2::Fields {
            reserved: [1, 0],
            ..f.fields
        },
        nfs_fire2::Fields {
            routing_b: 2,
            ..f.fields
        },
    ] {
        let bad = nfs_fire2::encode(nfs_fire2::Frame { fields, ..f }, frame_limits()).unwrap();
        assert!(current.reply(&catalog, &bad, 123).is_err());
    }
}

#[test]
fn bounded_content_and_partial_or_corrupt_state_are_rejected() {
    for name in ["obtained", "progress", "persona", "response"] {
        let mut v = content();
        v[name] = json!(0);
        assert!(Catalog::from_json(&v).is_err());
    }
    let mut v = content();
    v["challenges"][0]["id"] = json!(u64::MAX);
    assert!(Catalog::from_json(&v).is_err());
    let mut v = content();
    v["awards"][0]["items"][0]["obtained"] = json!(true);
    assert!(Catalog::from_json(&v).is_err());
    let mut v = content();
    v["challenges"] = json!(vec![v["challenges"][0].clone(); 65]);
    assert!(Catalog::from_json(&v).is_err());
    let mut v = content();
    v["ranks"] = json!([v["ranks"][0].clone(), v["ranks"][0].clone()]);
    assert!(Catalog::from_json(&v).is_err());
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([3; 16]).unwrap();
    let mut snapshot = initialize(&repo, who);
    snapshot.tables.pop_first();
    assert!(State::load(&snapshot).is_err());
    let mut excessive = State::default();
    for id in 0..513 {
        excessive.progress.insert((7, id), Progress::default());
    }
    assert!(excessive.operations().is_err());
    let mut snapshot = initialize(&repo, who);
    let table = snapshot
        .tables
        .get_mut(&crate::persistent::key("AfterhoursChallengesMeta"))
        .unwrap();
    table.rows.get_mut(&0).unwrap().insert(
        crate::persistent::key("Document"),
        nfs_storage::tables::Value::String("{\"version\":1,\"version\":1}".into()),
    );
    assert!(State::load(&snapshot).is_err());
}

#[test]
fn atomic_updates_retries_isolation_and_sqlite_restart() {
    let path = std::env::temp_dir().join(format!(
        "nfs-challenges-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let who = AccountId::from_owned_config([4; 16]).unwrap();
    let other = AccountId::from_owned_config([5; 16]).unwrap();
    let expected;
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let initial = initialize(&repo, who);
        let untouched = initialize(&repo, other);
        let batch = Batch {
            id: 11,
            expected_generation: initial.generation,
            ops: populated().operations().unwrap(),
        };
        assert!(!repo.apply(who, &batch, Timestamp(2)).unwrap().replayed);
        assert!(repo.apply(who, &batch, Timestamp(3)).unwrap().replayed);
        assert_eq!(
            repo.apply(who, &Batch { id: 12, ..batch }, Timestamp(4)),
            Err(nfs_storage::Error::Conflict)
        );
        assert_eq!(repo.snapshot(other).unwrap(), untouched);
        expected = repo.snapshot(who).unwrap();
    }
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        repo.open(who).unwrap();
        assert_eq!(repo.snapshot(who).unwrap(), expected);
        assert_eq!(
            ensure_empty(&repo, who, expected.clone(), Timestamp(9)).unwrap(),
            expected
        );
        assert_eq!(State::load(&expected).unwrap(), populated());
    }
    std::fs::remove_dir_all(path).unwrap();
}
