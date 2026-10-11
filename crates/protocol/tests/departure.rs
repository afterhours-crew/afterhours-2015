// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Departure codecs against independently packed synthetic bytes.
use nfs_heat2::Limits;
use nfs_protocol::{Error, gamemanager::*, users::ObjectId};

fn leave() -> Vec<u8> {
    // BTPL (30722, 2, 5), CNTX 0, GID 57, PID 42, REAS 8, SCTX "".
    vec![
        0x8b, 0x4c, 0x2c, 9, 0x82, 0xe0, 3, 2, 5, //
        0x8e, 0xed, 0x38, 0, 0, //
        0x9e, 0x99, 0, 0, 57, //
        0xc2, 0x99, 0, 0, 42, //
        0xca, 0x58, 0x73, 0, 8, //
        0xce, 0x3d, 0x38, 1, 1, 0,
    ]
}
fn removed() -> Vec<u8> {
    // CNTX 0, GID 57, LFPJ false, PID 42, REAS 7.
    vec![
        0x8e, 0xed, 0x38, 0, 0, //
        0x9e, 0x99, 0, 0, 57, //
        0xb2, 0x6c, 0x2a, 0, 0, //
        0xc2, 0x99, 0, 0, 42, //
        0xca, 0x58, 0x73, 0, 7,
    ]
}

#[test]
fn leave_request_round_trips_its_six_fields() {
    let b = leave();
    let q = LeaveGameByGroupRequest::decode(&b, Limits::default()).unwrap();
    assert_eq!(q.blaze_object_type_and_id, Some(ObjectId(30722, 2, 5)));
    assert_eq!(
        (
            q.player_removed_title_context,
            q.game_id,
            q.player_id,
            q.player_removed_reason,
            q.title_context_string
        ),
        (
            Some(0),
            Some(57),
            Some(42),
            Some(GROUP_LEFT),
            Some(b"".as_slice())
        )
    );
    assert_eq!(q.unknown_field_count(), 0);
    assert_eq!(q.encode(Limits::default()).unwrap(), b);
}

#[test]
fn removal_notification_encodes_the_independent_layout() {
    let n = NotifyPlayerRemoved {
        player_removed_title_context: Some(0),
        game_id: Some(57),
        is_lockable_for_preferred_joins: Some(false),
        player_id: Some(42),
        player_removed_reason: Some(PLAYER_LEFT),
        ..Default::default()
    };
    assert_eq!(n.encode(Limits::default()).unwrap(), removed());
    let bytes = removed();
    let d = NotifyPlayerRemoved::decode(&bytes, Limits::default()).unwrap();
    assert_eq!(d.player_removed_reason, Some(PLAYER_LEFT));
    assert_eq!(d.is_lockable_for_preferred_joins, Some(false));
}

#[test]
fn unknown_fields_survive_and_duplicates_or_truncation_fail() {
    let mut b = leave();
    b.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1]);
    let q = LeaveGameByGroupRequest::decode(&b, Limits::default()).unwrap();
    assert_eq!(q.unknown_field_count(), 1);
    assert_eq!(q.encode(Limits::default()).unwrap(), b);
    let mut b = leave();
    b.extend_from_slice(&[0x9e, 0x99, 0, 0, 58]);
    assert!(matches!(
        LeaveGameByGroupRequest::decode(&b, Limits::default()),
        Err(Error::DuplicateTag(_))
    ));
    let b = leave();
    for split in 1..b.len() {
        if let Ok(q) = LeaveGameByGroupRequest::decode(&b[..split], Limits::default()) {
            assert_ne!(q.encode(Limits::default()).unwrap(), b);
        }
    }
}
