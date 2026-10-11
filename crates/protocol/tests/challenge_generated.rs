// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_heat2::{Limits, decode};
use nfs_protocol::challenge::*;

fn model() -> GeneratedChallengesDataResponse<'static> {
    GeneratedChallengesDataResponse {
        blaze_id: Some(42),
        awards: Some(ChallengeAwards(vec![
            (
                9,
                vec![AwardData {
                    id: Some(u64::MAX),
                    item_guid: Some(b"item"),
                    obtained: Some(false),
                    kind: Some(u32::MAX),
                    unlock_value: Some(3),
                    ..Default::default()
                }],
            ),
            (1, vec![]),
        ])),
        day_id: Some(5),
        monthly_ranks: Some(MonthlyRanks(vec![MonthlyRankInstance {
            id: Some(2),
            rank_unlock: Some(4),
            ..Default::default()
        }])),
        progress: Some(ChallengeProgressList(vec![ChallengeProgress {
            complete: Some(false),
            current_count_one: Some(1),
            current_count_two: Some(0),
            id: Some(9),
            ..Default::default()
        }])),
        monthly_rank: Some(2),
        monthly_start: Some(-7),
        challenges: Some(ChallengeInstances(vec![ChallengeInstance {
            car_id: Some(1),
            count_one: Some(3),
            count_two: Some(4),
            day: Some(b"day"),
            event_id: Some(6),
            id: Some(9),
            kind: Some(u32::MAX),
            weekly: Some(true),
            type_override_id: Some(7),
            ..Default::default()
        }])),
        ..Default::default()
    }
}

#[test]
fn fresh_typed_values_round_trip_without_response_bytes() {
    let bytes = model().encode(Limits::default()).unwrap();
    let decoded = GeneratedChallengesDataResponse::decode(&bytes, Limits::default()).unwrap();
    assert_eq!(decoded.encode(Limits::default()).unwrap(), bytes);
    let awards = decoded.awards.as_ref().unwrap();
    assert_eq!(
        awards.0.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [9, 1]
    );
    assert_eq!(awards.0[0].1[0].id, Some(u64::MAX));
    assert_eq!(awards.0[0].1[0].kind, Some(u32::MAX));
    assert!(awards.0[1].1.is_empty());
    assert_eq!(decoded.monthly_start, Some(-7));
    assert_eq!(decoded.challenges.as_ref().unwrap().0[0].weekly, Some(true));
    assert!(
        ChallengesDataResponse::decode(&bytes, Limits::default())
            .unwrap()
            .has_all_fields()
    );
    assert!(
        GeneratedChallengesDataResponse::decode(&[], Limits::default())
            .unwrap()
            .encode(Limits::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn independent_award_map_unknowns_absence_and_invalid_values() {
    // CHAW{7:[{AWID=1},{}],8:[]}; only the map field is present.
    let golden = [
        0x8e, 0x88, 0x77, 5, 0, 3, 2, 7, 3, 2, 0x87, 0x7a, 0x64, 0, 1, 0, 0, 8, 3, 0,
    ];
    let parsed = GeneratedChallengesDataResponse::decode(&golden, Limits::default()).unwrap();
    assert!(parsed.blaze_id.is_none());
    assert_eq!(parsed.encode(Limits::default()).unwrap(), golden);
    for end in 1..golden.len() {
        assert!(
            GeneratedChallengesDataResponse::decode(&golden[..end], Limits::default()).is_err()
        );
    }
    assert!(
        GeneratedChallengesDataResponse::decode(&[golden, golden].concat(), Limits::default())
            .is_err()
    );
    let unknown_bytes = [0xff, 0xff, 0xff, 0, 5];
    let unknown = decode(&unknown_bytes, Limits::default())
        .unwrap()
        .fields()
        .next()
        .unwrap()
        .unwrap();
    let mut value = model();
    value.unknown.push(unknown);
    value.awards.as_mut().unwrap().0[0].1[0]
        .unknown
        .push(unknown);
    let bytes = value.encode(Limits::default()).unwrap();
    let decoded = GeneratedChallengesDataResponse::decode(&bytes, Limits::default()).unwrap();
    assert_eq!(decoded.unknown_field_count(), 2);
    assert_eq!(decoded.encode(Limits::default()).unwrap(), bytes);
    value.awards.as_mut().unwrap().0.push((9, vec![]));
    assert!(value.encode(Limits::default()).is_err());
    let invalid_boolean = [0x8e, 0x8c, 0x32, 4, 3, 1, 0x8e, 0x88, 0xed, 0, 2, 0];
    assert!(GeneratedChallengesDataResponse::decode(&invalid_boolean, Limits::default()).is_err());
    let mut duplicate = golden;
    duplicate[17] = 7;
    assert!(GeneratedChallengesDataResponse::decode(&duplicate, Limits::default()).is_err());
}

#[test]
fn destination_limits_apply_to_generated_and_parsed_models() {
    let value = model();
    let bytes = value.encode(Limits::default()).unwrap();
    for limits in [
        Limits {
            max_bytes: bytes.len() - 1,
            ..Limits::default()
        },
        Limits {
            max_collection: 1,
            ..Limits::default()
        },
        Limits {
            max_values: 8,
            ..Limits::default()
        },
        Limits {
            max_depth: 2,
            ..Limits::default()
        },
        Limits {
            max_byte_string: 3,
            ..Limits::default()
        },
    ] {
        assert!(value.encode(limits).is_err());
        assert!(GeneratedChallengesDataResponse::decode(&bytes, limits).is_err());
    }
}
