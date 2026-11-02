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

mod screenshots {
    use super::*;
    use nfs_services::kickback::{MAX_PENDING_SCREENSHOTS, PendingScreenshots};

    /// Official E1026 race-1 request: AUTO 1, DESC "2:4:6:0:ID_CAR_NISSAN_…",
    /// NAME 1761118161, ST 1, X 159, Y 102; reply RECN "1761118161".
    const ADD: &str = "875d2f0001925ce3013b323a343a363a303a49445f4341525f4e495353414e5f534b594c494e454754524e49534d4f5a54554e455f323030355f53484f52545f4e414d4500ba1b6500919fc48f0dcf40000001e00000009f02e4000000a601";
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn commit(id: u64, corr: u32) -> Vec<u8> {
        let body = CommitScreenshotRequest {
            screenshot_id: Some(id),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        wire(9, 0, corr, &body)
    }
    fn add_with_name(name: u64, corr: u32) -> Vec<u8> {
        let body = AddPendingScreenshotRequest {
            name: Some(name),
            automatic: Some(1),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        wire(8, 0, corr, &body)
    }
    fn state(a: AccountId) -> State {
        State::from_json(&document(a, None), a).unwrap()
    }

    #[test]
    fn kickback_screenshot_id_comes_from_the_clock_and_record_name_from_the_request() {
        let a = account(1);
        let s = state(a);
        let mut pending = PendingScreenshots::default();
        let t = 1_761_118_166_123_456; // 5 s after the client's NAME clock
        let reply = s
            .reply_screenshot(&mut pending, &wire(8, 0, 20, &hex(ADD)), a, 42, t)
            .unwrap();
        let f = frame(&reply);
        assert_eq!(
            (f.fields.routing_b, f.fields.category, f.fields.correlation),
            (8, 1, 20)
        );
        let m = AddPendingScreenshotResponse::decode(f.body, body_limits()).unwrap();
        assert_eq!(m.screenshot_id, Some(1_761_118_166));
        assert_eq!(m.record_name, Some(b"1761118161".as_slice()));
        assert_eq!(m.unknown_field_count(), 0);
        assert!(pending.contains(1_761_118_166));
        // Same clock again: the id still moves forward; a repeat is a new record.
        let again = s
            .reply_screenshot(&mut pending, &wire(8, 0, 21, &hex(ADD)), a, 42, t)
            .unwrap();
        let m = AddPendingScreenshotResponse::decode(frame(&again).body, body_limits()).unwrap();
        assert_eq!(m.screenshot_id, Some(1_761_118_167));
        assert_eq!(pending.len(), 2);
        // Deterministic: a fresh session with the same clock allocates the same id.
        let mut other = PendingScreenshots::default();
        let replay = s
            .reply_screenshot(&mut other, &wire(8, 0, 20, &hex(ADD)), a, 42, t)
            .unwrap();
        assert_eq!(replay, reply);
        // A clock before 1970 or at zero still yields a positive id.
        let mut early = PendingScreenshots::default();
        let r = s
            .reply_screenshot(&mut early, &wire(8, 0, 22, &hex(ADD)), a, 42, -5)
            .unwrap();
        let m = AddPendingScreenshotResponse::decode(frame(&r).body, body_limits()).unwrap();
        assert_eq!(m.screenshot_id, Some(1));
    }

    #[test]
    fn kickback_screenshot_commit_answers_counters_once_then_not_found() {
        let a = account(1);
        let s = state(a); // counters 3 / 20 from the document
        let mut pending = PendingScreenshots::default();
        let r = s
            .reply_screenshot(&mut pending, &wire(8, 0, 30, &hex(ADD)), a, 42, 7_000_000)
            .unwrap();
        let id = AddPendingScreenshotResponse::decode(frame(&r).body, body_limits())
            .unwrap()
            .screenshot_id
            .unwrap();
        let ok = s
            .reply_screenshot(&mut pending, &commit(id, 31), a, 42, 8_000_000)
            .unwrap();
        let f = frame(&ok);
        assert_eq!(
            (f.fields.routing_b, f.fields.category, f.fields.correlation),
            (9, 1, 31)
        );
        let m = CommitScreenshotResponse::decode(f.body, body_limits()).unwrap();
        assert_eq!(
            (m.screenshot_count, m.screenshot_count_max),
            (Some(3), Some(20))
        );
        assert!(pending.is_empty());
        for bad in [id, id + 1, 0] {
            let err = s
                .reply_screenshot(&mut pending, &commit(bad, 32), a, 42, 9_000_000)
                .unwrap();
            let d = nfs_fire2::decode(&err, Default::default())
                .unwrap()
                .unwrap();
            assert_eq!((d.frame.fields.routing_b, d.frame.fields.category), (9, 3));
            assert!(d.frame.body.is_empty());
            let m =
                nfs_protocol::metadata::Fire2Metadata::decode(d.frame.metadata, Default::default())
                    .unwrap();
            assert_eq!(
                m.error_code,
                Some(nfs_protocol::metadata::KICKBACK_ERR_NOT_FOUND)
            );
        }
    }

    #[test]
    fn kickback_screenshot_pending_set_is_bounded_and_foreign_requests_do_not_answer() {
        let a = account(1);
        let s = state(a);
        let mut pending = PendingScreenshots::default();
        let mut ids = Vec::new();
        for i in 0..=MAX_PENDING_SCREENSHOTS as u64 {
            let r = s
                .reply_screenshot(&mut pending, &add_with_name(100 + i, 40), a, 42, 1_000_000)
                .unwrap();
            let m = AddPendingScreenshotResponse::decode(frame(&r).body, body_limits()).unwrap();
            assert_eq!(m.record_name, Some((100 + i).to_string().as_bytes()));
            ids.push(m.screenshot_id.unwrap());
        }
        assert_eq!(pending.len(), MAX_PENDING_SCREENSHOTS);
        assert!(!pending.contains(ids[0]), "oldest record forgotten");
        assert!(pending.contains(ids[1]) && pending.contains(ids[MAX_PENDING_SCREENSHOTS]));
        // Unknown fields are tolerated; a request without NAME is not answered.
        let mut extra = hex(ADD);
        extra.extend_from_slice(&hex("fef7df0005"));
        assert!(
            s.reply_screenshot(&mut pending, &wire(8, 0, 41, &extra), a, 42, 1)
                .is_ok()
        );
        let no_name = AddPendingScreenshotRequest {
            automatic: Some(1),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        assert_eq!(
            s.reply_screenshot(&mut pending, &wire(8, 0, 42, &no_name), a, 42, 1),
            Err(Error::Ineligible)
        );
        assert_eq!(
            s.reply_screenshot(&mut pending, &wire(8, 0, 43, &hex(ADD)), account(2), 42, 1),
            Err(Error::Identity)
        );
        assert_eq!(
            s.reply_screenshot(&mut pending, &wire(8, 1, 44, &hex(ADD)), a, 42, 1),
            Err(Error::Ineligible)
        );
        assert_eq!(
            s.reply_screenshot(&mut pending, &wire(2, 0, 45, &[]), a, 42, 1),
            Err(Error::Ineligible)
        );
        assert!(nfs_services::kickback::owns_screenshots(2053, 8));
        assert!(nfs_services::kickback::owns_screenshots(2053, 9));
        assert!(!nfs_services::kickback::owns_screenshots(2053, 2));
        assert!(!nfs_services::kickback::owns(2053, 8));
    }
}
