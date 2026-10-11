// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_storage::{
    AccountId, Batch, GarageSlots, InventoryRepository, Op, SqliteRepository, Timestamp,
    tables::{Table, Value},
};
use serde_json::json;

fn content() -> Json {
    json!({"format":"nfs-stat-definitions","version":1,"build_sha256":SUPPORTED_BUILD_SHA256,"groups":[{
        "name":"Persistence_RepValuesTable","table":"RepValuesTable","category":"Test","description":"Constructed definitions","metadata":"","entity_type":[30722,1],
        "stats":(["RepScore","RepLevel"].map(|name|json!({"category":"Test","default":"99","derived":false,"format":"%d","kind":"","long_desc":name,"metadata":"","name":name,"short_desc":name,"stat_type":0})))
    }]})
}
fn query(persona: i64, view: u32) -> Vec<u8> {
    let body = GetStatsByGroupRequest {
        entity_ids: Some(EntityIds(vec![persona])),
        group_name: Some(b"Persistence_RepValuesTable"),
        period_ctr: Some(1),
        period_offset: Some(0),
        period_id: Some(0),
        period_type: Some(0),
        time: Some(0),
        view_id: Some(view),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    encode(
        Fields {
            routing_a: 7,
            routing_b: 16,
            correlation: 73,
            ..Default::default()
        },
        &body,
    )
    .unwrap()
}
fn current(score: i32, level: i32) -> Current {
    Current {
        stores: BTreeMap::from([
            ((key("RepValuesTable"), key("RepScore")), score),
            ((key("RepValuesTable"), key("RepLevel")), level),
        ]),
        generation: 1,
    }
}
fn values(reply: &[Vec<u8>], persona: i64, view: u32) -> Vec<String> {
    assert_eq!(reply.len(), 2);
    let ack = nfs_fire2::decode(&reply[0], frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(
        (
            ack.fields.routing_a,
            ack.fields.routing_b,
            ack.fields.category,
            ack.fields.correlation
        ),
        (7, 16, 1, 73)
    );
    assert!(ack.body.is_empty());
    let notification = nfs_fire2::decode(&reply[1], frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(
        (
            notification.fields.routing_a,
            notification.fields.routing_b,
            notification.fields.category,
            notification.fields.correlation
        ),
        (7, 50, 2, 0)
    );
    let result = KeyScopedStatValues::decode(notification.body, body_limits()).unwrap();
    assert_eq!((result.view_id, result.last), (Some(view), Some(true)));
    let rows = result.stat_values.unwrap().entity_stats.unwrap().0;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entity_id, Some(persona));
    rows[0]
        .stat_values
        .as_ref()
        .unwrap()
        .0
        .iter()
        .map(|v| String::from_utf8(v.to_vec()).unwrap())
        .collect()
}
#[test]
fn current_values_identity_view_and_correlated_batch_are_not_captured() {
    let catalog = Catalog::from_json(&content()).unwrap();
    for (persona, score, level, view) in [(123, 41, 3, 19), (456, -2, 0, 87)] {
        let request = query(persona, view);
        let result = catalog
            .reply(&request, persona, &current(score, level))
            .unwrap()
            .unwrap();
        assert_eq!(
            values(&result, persona, view),
            [score.to_string(), level.to_string()]
        );
        assert_eq!(
            catalog
                .reply(&request, persona, &current(score, level))
                .unwrap()
                .unwrap(),
            result
        );
        assert!(
            catalog
                .reply(&request, persona + 1, &current(score, level))
                .is_err()
        );
    }
}
#[test]
fn definitions_remain_static_while_values_change_and_missing_state_fails() {
    let catalog = Catalog::from_json(&content()).unwrap();
    let body = GetStatGroupRequest {
        name: Some(b"Persistence_RepValuesTable"),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    let request = encode(
        Fields {
            routing_a: 7,
            routing_b: 4,
            correlation: 29,
            ..Default::default()
        },
        &body,
    )
    .unwrap();
    let replies = catalog
        .reply(&request, 12, &current(10, 2))
        .unwrap()
        .unwrap();
    let frame = nfs_fire2::decode(&replies[0], frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(frame.fields.correlation, 29);
    let group = StatGroupResponse::decode(frame.body, body_limits()).unwrap();
    assert_eq!(
        group.stat_descs.unwrap().0[0].default_value,
        Some(b"99".as_slice())
    );
    assert_eq!(
        catalog
            .reply(&request, 12, &current(22, 8))
            .unwrap()
            .unwrap(),
        replies
    );
    let mut missing = current(10, 2);
    missing.stores.clear();
    assert_eq!(
        catalog.reply(&query(12, 1), 12, &missing),
        Err(Error::State)
    );
}
#[test]
fn malformed_queries_unknown_fields_periods_and_frames_never_get_success() {
    let catalog = Catalog::from_json(&content()).unwrap();
    let request = query(12, 1);
    for n in 0..request.len() {
        assert!(catalog.reply(&request[..n], 12, &current(1, 1)).is_err());
    }
    assert!(
        catalog
            .reply(
                &[request.clone(), request.clone()].concat(),
                12,
                &current(1, 1)
            )
            .is_err()
    );
    let frame = nfs_fire2::decode(&request, frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    for case in 0..5 {
        let mut q = GetStatsByGroupRequest::decode(frame.body, body_limits()).unwrap();
        match case {
            0 => q.period_offset = Some(1),
            1 => q.period_type = Some(1),
            2 => q.view_id = Some(0),
            3 => q.entity_ids = Some(EntityIds(vec![12, 13])),
            _ => q.time = Some(99),
        }
        let wire = encode(frame.fields, &q.encode(body_limits()).unwrap()).unwrap();
        assert!(catalog.reply(&wire, 12, &current(1, 1)).is_err());
    }
    let mut body = frame.body.to_vec();
    let mut extra = nfs_heat2::Encoder::new(body_limits());
    extra.integer([0xe3, 0x8e, 0x38], 1).unwrap();
    body.extend(extra.finish().unwrap());
    assert!(
        catalog
            .reply(&encode(frame.fields, &body).unwrap(), 12, &current(1, 1))
            .is_err()
    );
}
#[test]
fn catalog_rejects_player_values_duplicates_and_unsupported_schemas() {
    for case in 0..8 {
        let mut value = content();
        match case {
            0 => value["groups"][0]["captured_values"] = json!([123]),
            1 => value["groups"][0]["table"] = json!("AnotherTable"),
            2 => value["groups"][0]["stats"][1] = value["groups"][0]["stats"][0].clone(),
            3 => value["groups"][0]["stats"][0]["derived"] = json!(true),
            4 => value["groups"][0]["stats"][0]["stat_type"] = json!(1),
            5 => value["groups"][0]["entity_type"] = json!([4, 1]),
            6 => value["groups"][0]["metadata"] = json!("x".repeat(1025)),
            _ => {
                let group = value["groups"][0].clone();
                value["groups"] = json!(vec![group; MAX_GROUPS + 1]);
            }
        }
        assert!(Catalog::from_json(&value).is_err(), "case {case}");
    }
}
#[test]
fn committed_updates_survive_reopen_and_other_accounts_remain_independent() {
    let mut metadata = crate::persistent::tests::content();
    let row = metadata["tables"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == "RepValuesTable")
        .unwrap();
    row["columns"] = json!(
        [
            "RepScore",
            "RepLevel",
            "StyleScore",
            "SpeedScore",
            "BuildScore",
            "CrewScore",
            "OutlawScore"
        ]
        .map(|name| json!({"name":name,"key":key(name),"value_type":"int","default":"0"}))
    );
    let tables = crate::persistent::Catalog::from_json(&metadata).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "nfs-stats-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let repo = SqliteRepository::open_owned_directory(&dir).unwrap();
    let a = AccountId::from_owned_config([1; 16]).unwrap();
    let b = AccountId::from_owned_config([2; 16]).unwrap();
    for who in [a, b] {
        repo.open(who).unwrap();
        repo.apply(
            who,
            &Batch {
                id: 1,
                expected_generation: 0,
                ops: vec![Op::SetGarage(GarageSlots::default())],
            },
            Timestamp(1),
        )
        .unwrap();
        tables
            .ensure_loaded(&repo, who, repo.snapshot(who).unwrap(), Timestamp(2))
            .unwrap();
    }
    let thresholds = reputation::Thresholds::new(vec![0, 10, 25]).unwrap();
    let view = |repo: &SqliteRepository, who| {
        Current::from_loaded(
            &tables.view(&repo.snapshot(who).unwrap()).unwrap(),
            &thresholds,
        )
        .unwrap()
    };
    let catalog = Catalog::from_json(&content()).unwrap();
    assert_eq!(
        values(
            &catalog
                .reply(&query(123, 1), 123, &view(&repo, a))
                .unwrap()
                .unwrap(),
            123,
            1
        ),
        ["0", "1"]
    );
    let changed = Table {
        rows: BTreeMap::from([(
            0,
            BTreeMap::from([
                (key("RepScore"), Value::Int(12)),
                (key("RepLevel"), Value::Int(999)),
            ]),
        )]),
    };
    let batch = Batch {
        id: 3,
        expected_generation: 2,
        ops: vec![Op::SetTable(key("RepValuesTable"), changed)],
    };
    repo.apply(a, &batch, Timestamp(3)).unwrap();
    repo.apply(a, &batch, Timestamp(3)).unwrap();
    drop(repo);
    let reopened = SqliteRepository::open_owned_directory(&dir).unwrap();
    assert_eq!(
        values(
            &catalog
                .reply(&query(123, 8), 123, &view(&reopened, a))
                .unwrap()
                .unwrap(),
            123,
            8
        ),
        ["12", "2"]
    );
    assert_eq!(
        values(
            &catalog
                .reply(&query(456, 9), 456, &view(&reopened, b))
                .unwrap()
                .unwrap(),
            456,
            9
        ),
        ["0", "1"]
    );
    assert_eq!(view(&reopened, a).generation, 3);
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}
