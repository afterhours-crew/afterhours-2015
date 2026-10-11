// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::kickback::*;
use nfs_services::ContentError;
use nfs_services::kickback::{Error, State, Winner, body_limits, frame_limits};
use nfs_storage::AccountId;
use serde_json::json;

fn account(byte: u8) -> AccountId {
    AccountId::from_owned_config([byte; 16]).unwrap()
}
fn wire(command: u16, category: u8, corr: u32, body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2053,
                routing_b: command,
                category,
                correlation: corr,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        Default::default(),
    )
    .unwrap()
}
fn frame(b: &[u8]) -> Frame<'_> {
    let d = nfs_fire2::decode(b, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, b.len());
    d.frame
}
fn winner() -> Winner {
    Winner {
        datetime: 1_700_000_000,
        title: "Night run".into(),
        screenshot_id: 9_001,
        kickback_count: 12,
        persona_id: 777,
        player_provided_kickback: false,
        record_name: "record-a".into(),
        screenshot_type: 2,
    }
}
fn document(account: AccountId, winner: Option<serde_json::Value>) -> serde_json::Value {
    json!({"format":"nfs-kickback-state","version":1,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"account":account.hex(),
        "screenshot_count":3,"screenshot_count_max":20,"gallery":"empty",
        "winner":winner.unwrap_or(serde_json::Value::Null)})
}
fn winner_json() -> serde_json::Value {
    json!({"datetime":1_700_000_000,"title":"Night run","screenshot_id":9001,"kickback_count":12,
        "persona_id":777,"player_provided_kickback":false,"record_name":"record-a","screenshot_type":2})
}

#[test]
fn counters_winner_and_empty_gallery_come_from_named_state() {
    let a = account(1);
    let state = State::from_json(&document(a, Some(winner_json())), a).unwrap();
    assert_eq!(state.counters(), (3, 20));
    assert_eq!(state.winner(), Some(&winner()));
    let counters = state.reply(&wire(2, 0, 5, &[]), a, 42).unwrap();
    let f = frame(&counters);
    assert_eq!((f.fields.category, f.fields.correlation), (1, 5));
    let m = GetScreenshotCounterResponse::decode(f.body, body_limits()).unwrap();
    assert_eq!(
        (m.screenshot_count, m.screenshot_count_max),
        (Some(3), Some(20))
    );
    let tile = state.reply(&wire(17, 0, 6, &[]), a, 42).unwrap();
    let m = ScreenshotGalleryDataEx::decode(frame(&tile).body, body_limits()).unwrap();
    assert_eq!(m.is_last_week_winner, Some(true));
    assert_eq!(m.persona_id, Some(777));
    assert_eq!(m.record_name, Some(b"record-a".as_slice()));
    assert_eq!(m.title, Some(b"Night run".as_slice()));
    assert_eq!(m.unknown_field_count(), 0);
    let gallery = state.reply(&wire(22, 0, 7, &[]), a, 42).unwrap();
    let m = GetSnapshotGalleryTilesResponse::decode(frame(&gallery).body, body_limits()).unwrap();
    assert!(m.tile_identifiers.unwrap().0.is_empty());
    // Repeats are identical; another correlation only changes the header.
    assert_eq!(state.reply(&wire(22, 0, 7, &[]), a, 42).unwrap(), gallery);
    assert_ne!(state.reply(&wire(22, 0, 8, &[]), a, 42).unwrap(), gallery);
}

#[test]
fn missing_winner_answers_not_found_and_foreign_frames_do_not_answer() {
    let a = account(1);
    let state = State::from_json(&document(a, None), a).unwrap();
    let absent = state.reply(&wire(17, 0, 9, &[]), a, 42).unwrap();
    let d = nfs_fire2::decode(&absent, Default::default())
        .unwrap()
        .unwrap();
    assert_eq!(d.consumed, absent.len());
    assert_eq!(
        (
            d.frame.fields.routing_a,
            d.frame.fields.routing_b,
            d.frame.fields.category,
            d.frame.fields.correlation
        ),
        (2053, 17, 3, 9)
    );
    assert!(d.frame.body.is_empty());
    let m = nfs_protocol::metadata::Fire2Metadata::decode(d.frame.metadata, Default::default())
        .unwrap();
    assert_eq!(
        (m.context, m.error_code),
        (
            Some(0),
            Some(nfs_protocol::metadata::KICKBACK_ERR_NOT_FOUND)
        )
    );
    assert_eq!(
        nfs_protocol::metadata::error_name(2053, m.error_code.unwrap()),
        Some("KICKBACK_ERR_NOT_FOUND")
    );
    assert_eq!(
        state.reply(&wire(17, 0, 9, &[]), account(2), 42),
        Err(Error::Identity)
    );
    assert!(state.reply(&wire(2, 0, 1, &[]), a, 42).is_ok());
    assert_eq!(
        state.reply(&wire(2, 0, 1, &[]), account(2), 42),
        Err(Error::Identity)
    );
    assert_eq!(state.reply(&wire(2, 0, 1, &[]), a, 0), Err(Error::Identity));
    assert_eq!(
        state.reply(&wire(3, 0, 1, &[]), a, 42),
        Err(Error::Ineligible)
    );
    assert_eq!(
        state.reply(&wire(2, 1, 1, &[]), a, 42),
        Err(Error::Ineligible)
    );
    assert_eq!(
        state.reply(&wire(2, 0, 1, &[0x8a, 0xca, 0x64, 0, 1]), a, 42),
        Err(Error::Ineligible)
    );
    let q = wire(2, 0, 1, &[]);
    assert_eq!(
        state.reply(&[q.clone(), q.clone()].concat(), a, 42),
        Err(Error::Ineligible)
    );
    for split in 0..q.len() {
        assert_eq!(state.reply(&q[..split], a, 42), Err(Error::Ineligible));
    }
    let other = nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2050,
                routing_b: 2,
                ..Default::default()
            },
            metadata: &[],
            body: &[],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(state.reply(&other, a, 42), Err(Error::Ineligible));
}

#[test]
fn documents_are_validated_and_bound_to_the_account() {
    let a = account(1);
    let good = document(a, Some(winner_json()));
    assert!(State::from_json(&good, a).is_ok());
    assert_eq!(
        State::from_json(&good, account(2)).err(),
        Some(ContentError::Invalid)
    );
    let mut v = good.clone();
    v["version"] = json!(2);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["gallery"] = json!("official");
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["screenshot_count"] = json!(21);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["winner"]["record_name"] = json!("");
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["winner"]["screenshot_id"] = json!(0);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["winner"]["title"] = json!("x".repeat(257));
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good.clone();
    v["extra"] = json!(1);
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    let mut v = good;
    v.as_object_mut().unwrap().remove("winner");
    assert_eq!(State::from_json(&v, a).err(), Some(ContentError::Invalid));
    assert_eq!(State::new(a, 1, 0, None).err(), Some(ContentError::Invalid));
}
