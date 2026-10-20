// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::autolog::*;
use nfs_services::ContentError;
use nfs_services::speedwall::{
    Error, Row, State, body_limits, frame_limits, walls_frame_limits, walls_limits,
};
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
fn unknown_stat_personas_accounts_and_malformed_frames_do_not_answer() {
    let a = account(1);
    let state = State::from_json(&document(a), a).unwrap();
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
fn missing_owned_row_completes_with_not_found_without_creating_progress() {
    use nfs_protocol::metadata::{AUTOLOG_ERR_NOT_FOUND, Fire2Metadata, error_name};
    let a = account(1);
    let empty = State::new(a, vec![]).unwrap();
    let populated = State::from_json(&document(a), a).unwrap();
    let limits = nfs_fire2::Limits::new(128, 32, 64).unwrap();
    for (state, wall, stat) in [(&empty, 5, 0), (&empty, 5, 1), (&populated, 6, 0)] {
        let before = state.rows().to_vec();
        for correlation in [1, 0x00ff_ffff] {
            let request = query(42, wall, stat, correlation);
            let reply = state.reply(&request, a, 42, b"Local").unwrap();
            assert_eq!(state.reply(&request, a, 42, b"Local").unwrap(), reply);
            let decoded = nfs_fire2::decode(&reply, limits).unwrap().unwrap();
            assert_eq!(decoded.consumed, reply.len());
            let f = decoded.frame;
            assert_eq!(
                (
                    f.fields.routing_a,
                    f.fields.routing_b,
                    f.fields.category,
                    f.fields.correlation
                ),
                (2050, 79, 3, correlation)
            );
            assert!(f.body.is_empty());
            let metadata = Fire2Metadata::decode(f.metadata, body_limits()).unwrap();
            assert_eq!(metadata.context, Some(0));
            assert_eq!(metadata.error_code, Some(AUTOLOG_ERR_NOT_FOUND));
            assert_eq!(metadata.unknown_field_count(), 0);
        }
        assert_eq!(state.rows(), before);
    }
    assert_eq!(
        error_name(2050, AUTOLOG_ERR_NOT_FOUND),
        Some("AUTOLOG_ERR_NOT_FOUND")
    );
    assert_eq!(error_name(2053, AUTOLOG_ERR_NOT_FOUND), None);
    // Empty state must not weaken account/persona isolation or enum validation.
    assert_eq!(
        empty.reply(&query(43, 5, 0, 1), a, 42, b"N"),
        Err(Error::Identity)
    );
    assert_eq!(
        empty.reply(&query(42, 5, 0, 1), account(2), 42, b"N"),
        Err(Error::Identity)
    );
    assert_eq!(
        empty.reply(&query(42, 5, -1, 1), a, 42, b"N"),
        Err(Error::Unsupported)
    );
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

fn walls_query(persona: i64, walls: &[u32], options: [i32; 2], corr: u32) -> Vec<u8> {
    let body = InGameSpeedWallsRequest {
        blaze_id: Some(persona),
        speed_wall_ids: Some(SpeedWallIds(walls.to_vec())),
        usge: Some(options[0]),
        uspg: Some(options[1]),
        ..Default::default()
    }
    // Looser than the service's limits, so oversized requests can be built.
    .encode(nfs_heat2::Limits {
        max_collection: 128,
        max_values: 512,
        max_bytes: 8192,
        ..body_limits()
    })
    .unwrap();
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2050,
                routing_b: 20,
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

#[test]
fn in_game_walls_list_both_stat_types_with_own_rows_where_present() {
    let a = account(1);
    let state = State::from_json(&document(a), a).unwrap();
    let reply = state
        .reply_walls(
            &walls_query(42, &[5, 6], [0, 0], 11),
            a,
            42,
            b"Local Driver",
        )
        .unwrap();
    let d = nfs_fire2::decode(&reply, walls_frame_limits())
        .unwrap()
        .unwrap();
    assert_eq!(d.consumed, reply.len());
    let f = d.frame;
    assert_eq!(
        (f.fields.routing_b, f.fields.category, f.fields.correlation),
        (20, 1, 11)
    );
    let m = InGameSpeedWallResponse::decode(f.body, walls_limits()).unwrap();
    assert_eq!(m.unknown_field_count(), 0);
    let walls = m.speed_walls.unwrap().0;
    assert_eq!(
        walls.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        vec![0, 1]
    );
    for (stat, list) in &walls {
        assert_eq!(
            list.iter().map(|w| w.speed_wall_id).collect::<Vec<_>>(),
            vec![Some(5), Some(6)],
            "request order"
        );
        // Wall 5 has an own row on both stat types; wall 6 has none.
        let rows = &list[0].rows.as_ref().unwrap().0;
        let user = rows[0].blaze_user.as_ref().unwrap();
        assert_eq!(
            (user.blaze_id, user.persona_name, user.relation_type),
            (Some(42), Some(b"Local Driver".as_slice()), Some(1))
        );
        if *stat == 0 {
            assert_eq!(
                rows[0].stats_int.as_ref().unwrap().0,
                vec![(b"Events".as_slice(), 7), (b"Wins".as_slice(), -1)]
            );
        }
        assert!(list[1].rows.is_none());
    }
}

#[test]
fn in_game_walls_for_a_fresh_account_are_empty_walls() {
    let a = account(2);
    let state = State::new(a, vec![]).unwrap();
    let reply = state
        .reply_walls(
            &walls_query(42, &[1_761_118_161], [0, 0], 3),
            a,
            42,
            b"Local",
        )
        .unwrap();
    let d = nfs_fire2::decode(&reply, walls_frame_limits())
        .unwrap()
        .unwrap();
    let m = InGameSpeedWallResponse::decode(d.frame.body, walls_limits()).unwrap();
    let walls = m.speed_walls.unwrap().0;
    assert_eq!(walls.len(), 2);
    for (_, list) in walls {
        let [wall] = list.as_slice() else {
            panic!("one wall")
        };
        assert_eq!(wall.speed_wall_id, Some(1_761_118_161));
        assert!(wall.rows.is_none());
    }
}

#[test]
fn in_game_walls_refuse_foreign_personas_options_and_bounds() {
    let a = account(1);
    let state = State::from_json(&document(a), a).unwrap();
    assert_eq!(
        state.reply_walls(&walls_query(43, &[5], [0, 0], 1), a, 42, b"Local"),
        Err(Error::Identity)
    );
    assert_eq!(
        state.reply_walls(&walls_query(42, &[5], [0, 0], 1), account(9), 42, b"Local"),
        Err(Error::Identity)
    );
    assert_eq!(
        state.reply_walls(&walls_query(42, &[5], [1, 0], 1), a, 42, b"Local"),
        Err(Error::Unsupported)
    );
    assert_eq!(
        state.reply_walls(&walls_query(42, &[], [0, 0], 1), a, 42, b"Local"),
        Err(Error::Unsupported)
    );
    let many: Vec<u32> = (0..17).collect();
    assert!(
        state
            .reply_walls(&walls_query(42, &many, [0, 0], 1), a, 42, b"Local")
            .is_err()
    );
    // The brief query is not this route, and vice versa.
    assert_eq!(
        state.reply_walls(&query(42, 5, 0, 1), a, 42, b"Local"),
        Err(Error::Ineligible)
    );
    assert_eq!(
        state.reply(&walls_query(42, &[5], [0, 0], 1), a, 42, b"Local"),
        Err(Error::Ineligible)
    );
}
