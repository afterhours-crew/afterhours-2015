// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Byte-grounded fixtures for `2053/8 addPendingScreenshot` and
//! `2053/9 commitScreenshot`: the exact request and reply bodies of the first
//! two race finishes (correlations 199/200 and 229/230) and the third finish
//! with a negative `X` (correlation 258) in the official E1026 Amy capture.
use nfs_protocol::kickback::*;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
const DESC: &[u8] = b"2:4:6:0:ID_CAR_NISSAN_SKYLINEGTRNISMOZTUNE_2005_SHORT_NAME";
/// (request body, X, reply body, ID) per race; NAME 1761118161 and RECN
/// "1761118161" for races 1 and 2, NAME 1761118193 for race 3.
const RACES: [(&str, i32, &str, u64); 3] = [
    (
        "875d2f0001925ce3013b323a343a363a303a49445f4341525f4e495353414e5f534b594c494e454754524e49534d4f5a54554e455f323030355f53484f52545f4e414d4500ba1b6500919fc48f0dcf40000001e00000009f02e4000000a601",
        159,
        "a640000089d8bded01ca58ee010b3137363131313831363100",
        249_017_865,
    ),
    (
        "875d2f0001925ce3013b323a343a363a303a49445f4341525f4e495353414e5f534b594c494e454754524e49534d4f5a54554e455f323030355f53484f52545f4e414d4500ba1b6500919fc48f0dcf40000001e000000037e4000000a601",
        55,
        "a6400000a7d8bded01ca58ee010b3137363131313831363100",
        249_017_895,
    ),
    (
        "875d2f0001925ce3013b323a343a353a303a49445f4341525f4e495353414e5f534b594c494e454754524e49534d4f5a54554e455f323030355f53484f52545f4e414d4500ba1b6500b19fc48f0dcf40000001e0000000f319e40000008102",
        -1651,
        "a64000008ad9bded01ca58ee010b3137363131313831393300",
        249_017_930,
    ),
];

#[test]
fn kickback_add_pending_screenshot_official_bodies_round_trip() {
    for (index, (request, x, reply, id)) in RACES.iter().enumerate() {
        let bytes = hex(request);
        let m = AddPendingScreenshotRequest::decode(&bytes, Default::default()).unwrap();
        assert_eq!(m.unknown_field_count(), 0);
        assert_eq!(m.automatic, Some(1), "race {}", index + 1);
        assert_eq!(m.st, Some(1));
        assert_eq!(m.x, Some(*x));
        let (name, y, desc) = if index < 2 {
            (1_761_118_161, 102, DESC.to_vec())
        } else {
            let mut d = DESC.to_vec();
            d[4] = b'5';
            (1_761_118_193, 129, d)
        };
        assert_eq!(m.name, Some(name));
        assert_eq!(m.y, Some(y));
        assert_eq!(m.description, Some(desc.as_slice()));
        assert_eq!(m.encode(Default::default()).unwrap(), bytes);

        let bytes = hex(reply);
        let r = AddPendingScreenshotResponse::decode(&bytes, Default::default()).unwrap();
        assert_eq!(r.unknown_field_count(), 0);
        assert_eq!(r.screenshot_id, Some(*id));
        assert_eq!(r.record_name, Some(name.to_string().as_bytes()));
        assert_eq!(r.encode(Default::default()).unwrap(), bytes);
    }
}

#[test]
fn kickback_commit_screenshot_official_bodies_round_trip() {
    for (request, reply, id) in [
        (
            "a640000089d8bded01",
            "ce38c00001ce38ed008601",
            249_017_865u64,
        ),
        ("a6400000a7d8bded01", "ce38c00001ce38ed008601", 249_017_895),
    ] {
        let bytes = hex(request);
        let m = CommitScreenshotRequest::decode(&bytes, Default::default()).unwrap();
        assert_eq!((m.unknown_field_count(), m.screenshot_id), (0, Some(id)));
        assert_eq!(m.encode(Default::default()).unwrap(), bytes);
        let bytes = hex(reply);
        let r = CommitScreenshotResponse::decode(&bytes, Default::default()).unwrap();
        assert_eq!(
            (
                r.screenshot_count,
                r.screenshot_count_max,
                r.unknown_field_count()
            ),
            (Some(1), Some(70), 0)
        );
        assert_eq!(r.encode(Default::default()).unwrap(), bytes);
    }
    // The commit reply tags SCC/SCCM differ from the counters query's SSC/SCCM.
    let commit_reply = hex("ce38c00001ce38ed008601");
    let counters = GetScreenshotCounterResponse::decode(&commit_reply, Default::default()).unwrap();
    assert_eq!(counters.screenshot_count, None);
    assert_eq!(counters.unknown_field_count(), 1);
}

#[test]
fn kickback_screenshot_requests_keep_unknown_fields_and_reject_wrong_types() {
    // Unknown trailing field (tag "ZZZZ"-like, integer 5) is counted, not refused.
    let mut bytes = hex(RACES[0].0);
    bytes.extend_from_slice(&hex("fef7df0005"));
    let m = AddPendingScreenshotRequest::decode(&bytes, Default::default()).unwrap();
    assert_eq!(m.unknown_field_count(), 1);
    assert_eq!(m.name, Some(1_761_118_161));
    assert!(AddPendingScreenshotRequest::decode(&hex("ba1b65010100"), Default::default()).is_err());
    assert!(CommitScreenshotRequest::decode(&hex("a64000010100"), Default::default()).is_err());
    let empty = CommitScreenshotRequest::decode(&[], Default::default()).unwrap();
    assert_eq!(empty.screenshot_id, None);
}
