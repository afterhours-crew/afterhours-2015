// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::autolog::*;
use nfs_services::ContentError;
use nfs_services::speedwall::{Error, Row, State, body_limits, frame_limits};
use nfs_storage::AccountId;
use serde_json::json;

fn account(byte: u8) -> AccountId {
    AccountId::from_owned_config([byte; 16]).unwrap()
}
fn query(persona: i64, wall: u32, stat: i32, corr: u32) -> Vec<u8> {
    let body = SpeedWallBriefInfoRequest {
        blaze_id: Some(persona),
        speed_wall_id: Some(wall),
        speed_wall_stat_type: Some(stat),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2050,
                routing_b: 79,
                correlation: corr,
                ..Default::default()
            },
            metadata: &[],
            body: &body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn frame(b: &[u8]) -> Frame<'_> {
    let d = nfs_fire2::decode(b, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, b.len());
    d.frame
}
fn document(account: AccountId) -> serde_json::Value {
    json!({"format":"nfs-speedwall-state","version":1,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"account":account.hex(),
        "rows":[
            {"speed_wall_id":5,"stat_type":0,"rank":3,"rating_bits":0x4248_0000u32,"total_count":40,
             "integers":[["Events",7],["Wins",-1]],"floats":[["TopSpeed",0x4320_0000u32]],"strings":[["Car","M3"]]},
            {"speed_wall_id":5,"stat_type":1,"rank":1,"rating_bits":0,"total_count":2,
             "integers":[],"floats":[],"strings":null}]})
}

#[test]
fn own_row_names_the_current_persona_and_display_name() {
    let a = account(1);
    let state = State::from_json(&document(a), a).unwrap();
    assert_eq!(state.rows().len(), 2);
    let reply = state
        .reply(&query(42, 5, 0, 9), a, 42, b"Local Driver")
        .unwrap();
    let f = frame(&reply);
    assert_eq!(
        (f.fields.routing_b, f.fields.category, f.fields.correlation),
        (79, 1, 9)
    );
    let m = SpeedWallBriefInfoResponse::decode(f.body, body_limits()).unwrap();
    assert_eq!(m.unknown_field_count(), 0);
    assert_eq!((m.rank, m.total_count), (Some(3), Some(40)));
    assert_eq!(m.rating.map(|r| r.0), Some(0x4248_0000));
    assert_eq!(
        (m.speed_wall_id, m.speed_wall_stat_type),
        (Some(5), Some(0))
    );
    let row = m.speed_wall.unwrap();
    let user = row.blaze_user.unwrap();
    assert_eq!(user.blaze_id, Some(42));
    assert_eq!(user.persona_name, Some(b"Local Driver".as_slice()));
    assert_eq!(user.relation_type, Some(1));
    assert_eq!(
        row.stats_int.unwrap().0,
        vec![(b"Events".as_slice(), 7), (b"Wins".as_slice(), -1)]
    );
    assert_eq!(
        row.stats_flt.unwrap().0,
        vec![(b"TopSpeed".as_slice(), 0x4320_0000)]
    );
    assert_eq!(
        row.stats_str.unwrap().0,
        vec![(b"Car".as_slice(), b"M3".as_slice())]
    );
    // A row without a string map omits it on the wire.
    let second = state
        .reply(&query(42, 5, 1, 9), a, 42, b"Local Driver")
        .unwrap();
    let m = SpeedWallBriefInfoResponse::decode(frame(&second).body, body_limits()).unwrap();
    let row = m.speed_wall.unwrap();
    assert!(row.stats_str.is_none());
    assert!(row.stats_int.unwrap().0.is_empty());
    // Another persona on the same account gets its own identity in the row.
    let other = state.reply(&query(43, 5, 0, 9), a, 43, b"Other").unwrap();
    let m = SpeedWallBriefInfoResponse::decode(frame(&other).body, body_limits()).unwrap();
    assert_eq!(m.speed_wall.unwrap().blaze_user.unwrap().blaze_id, Some(43));
    assert_ne!(other, reply);
    assert_eq!(
        state
            .reply(&query(42, 5, 0, 9), a, 42, b"Local Driver")
            .unwrap(),
        reply
    );
}

#[test]
fn other_walls_personas_accounts_and_malformed_frames_do_not_answer() {
    let a = account(1);
    let state = State::from_json(&document(a), a).unwrap();
    assert_eq!(
        state.reply(&query(42, 6, 0, 1), a, 42, b"N"),
        Err(Error::Unsupported)
    );
    assert_eq!(
        state.reply(&query(42, 5, 2, 1), a, 42, b"N"),
        Err(Error::Unsupported)
    );
    assert_eq!(
        state.reply(&query(43, 5, 0, 1), a, 42, b"N"),
        Err(Error::Identity)
    );
    assert_eq!(
        state.reply(&query(42, 5, 0, 1), account(2), 42, b"N"),
        Err(Error::Identity)
    );
    assert_eq!(
        state.reply(&query(42, 5, 0, 1), a, 42, b""),
        Err(Error::Identity)
    );
    assert_eq!(
        state.reply(&query(42, 5, 0, 1), a, 42, &[b'a'; 33]),
        Err(Error::Identity)
    );
    let q = query(42, 5, 0, 1);
    assert_eq!(
        state.reply(&[q.clone(), q.clone()].concat(), a, 42, b"N"),
        Err(Error::Ineligible)
    );
    for split in 0..q.len() {
        assert_eq!(
            state.reply(&q[..split], a, 42, b"N"),
            Err(Error::Ineligible)
        );
    }
    let partial = SpeedWallBriefInfoRequest {
        blaze_id: Some(42),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    let wire = nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2050,
                routing_b: 79,
                ..Default::default()
            },
            metadata: &[],
            body: &partial,
        },
        frame_limits(),
    )
    .unwrap();
    assert_eq!(state.reply(&wire, a, 42, b"N"), Err(Error::Ineligible));
    let foreign = nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2050,
                routing_b: 78,
                ..Default::default()
            },
            metadata: &[],
            body: &[],
        },
        frame_limits(),
    )
    .unwrap();
    assert_eq!(state.reply(&foreign, a, 42, b"N"), Err(Error::Ineligible));
}

#[test]
fn documents_are_validated_bounded_and_account_bound() {
    let a = account(1);
    let good = document(a);
    assert!(State::from_json(&good, a).is_ok());
    assert_eq!(
        State::from_json(&good, account(2)).err(),
        Some(ContentError::Invalid)
    );
    let mut v = good.clone();
    v["rows"][1]["stat_type"] = json!(0);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["rows"][0]["stat_type"] = json!(2);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["rows"][0]["integers"] = json!([["Events", 1], ["Events", 2]]);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["rows"][0]["strings"] = json!([["Car", "x".repeat(257)]]);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["format"] = json!("nfs-kickback-state");
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good;
    v["rows"][0]["extra"] = json!(1);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let row = Row {
        speed_wall_id: 1,
        stat_type: 0,
        rank: 0,
        rating_bits: 0,
        total_count: 0,
        integers: vec![],
        floats: vec![],
        strings: None,
    };
    let many: Vec<Row> = (0..17)
        .map(|i| Row {
            speed_wall_id: i,
            ..row.clone()
        })
        .collect();
    assert_eq!(State::new(a, many).err(), Some(ContentError::TooLarge));
    assert!(State::new(a, vec![]).unwrap().rows().is_empty());
}
