// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_heat2::Encoder;

fn frame(body: &[u8], correlation: u32) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 9,
                routing_b: 20,
                category: 0,
                correlation,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn body(values: &[&[u8]]) -> Vec<u8> {
    FilterUserTextResponse {
        filtered_text_list: Some(FilteredUserTextList(
            values
                .iter()
                .map(|text| FilteredUserText {
                    result: Some(2),
                    filtered_text: Some(text),
                    ..Default::default()
                })
                .collect(),
        )),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap()
}
fn values(response: &[u8], correlation: u32) -> Vec<(i32, Vec<u8>)> {
    let d = nfs_fire2::decode(response, frame_limits())
        .unwrap()
        .unwrap();
    assert_eq!(d.consumed, response.len());
    assert_eq!(
        (
            d.frame.fields.routing_a,
            d.frame.fields.routing_b,
            d.frame.fields.category,
            d.frame.fields.correlation
        ),
        (9, 20, 1, correlation)
    );
    assert!(d.frame.metadata.is_empty());
    let result = FilterUserTextResponse::decode(d.frame.body, body_limits()).unwrap();
    result
        .filtered_text_list
        .unwrap()
        .0
        .into_iter()
        .map(|v| (v.result.unwrap(), v.filtered_text.unwrap().to_vec()))
        .collect()
}

#[test]
fn constructed_wire_uses_tlst_structs_with_result_and_text() {
    // Independently assembled tagged wire, using invented test text.
    let mut raw = vec![
        0xd2, 0xcc, 0xf4, 4, 3, 1, 0x92, 0x9c, 0xb4, 0, 2, 0xd7, 0x4e, 0x34, 1, 7,
    ];
    raw.extend_from_slice(b"TRACK7\0\0");
    assert_eq!(body(&[b"TRACK7"]), raw);
    let output = reply(&frame(&raw, 71)).unwrap();
    let d = nfs_fire2::decode(&output, frame_limits()).unwrap().unwrap();
    raw[10] = 0;
    assert_eq!(d.frame.body, raw);
    assert_eq!(values(&output, 71), [(0, b"TRACK7".to_vec())]);
}

#[test]
fn local_policy_preserves_order_duplicates_unicode_and_empty_text() {
    let maximum = [b'x'; MAX_TEXT_BYTES];
    let too_long = [b'y'; MAX_TEXT_BYTES + 1];
    let wire = frame(
        &body(&[
            b"DUP",
            "röd".as_bytes(),
            b"DUP",
            b"",
            &maximum,
            &too_long,
            b"bad\ntext",
            &[0xff],
        ]),
        72,
    );
    let result = values(&reply(&wire).unwrap(), 72);
    assert_eq!(
        result,
        [
            (0, b"DUP".to_vec()),
            (0, "röd".as_bytes().to_vec()),
            (0, b"DUP".to_vec()),
            (0, vec![]),
            (0, maximum.to_vec()),
            (3, vec![]),
            (4, vec![]),
            (4, vec![]),
        ]
    );
    assert_eq!(reply(&wire).unwrap(), reply(&wire).unwrap());
    assert!(values(&reply(&frame(&body(&[]), 73)).unwrap(), 73).is_empty());
}

#[test]
fn partial_trailing_frames_and_wrong_envelope_do_not_succeed() {
    let wire = frame(&body(&[b"OK"]), 5);
    for end in 0..wire.len() {
        assert_eq!(reply(&wire[..end]), Err(Error::Ineligible));
    }
    let mut trailing = wire.clone();
    trailing.push(0);
    assert_eq!(reply(&trailing), Err(Error::Ineligible));
    trailing.extend_from_slice(&wire);
    assert_eq!(reply(&trailing), Err(Error::Ineligible));
    let parsed = nfs_fire2::decode(&wire, frame_limits()).unwrap().unwrap();
    for fields in [
        Fields {
            category: 1,
            ..parsed.frame.fields
        },
        Fields {
            routing_a: 1,
            ..parsed.frame.fields
        },
        Fields {
            routing_b: 19,
            ..parsed.frame.fields
        },
        Fields {
            slot: 1,
            ..parsed.frame.fields
        },
        Fields {
            reserved: [1, 0],
            ..parsed.frame.fields
        },
    ] {
        let wrong = nfs_fire2::encode(
            Frame {
                fields,
                ..parsed.frame
            },
            frame_limits(),
        )
        .unwrap();
        assert_eq!(reply(&wrong), Err(Error::Ineligible));
    }
}

#[test]
fn missing_unknown_status_and_duplicate_known_fields_are_rejected() {
    assert_eq!(reply(&frame(&[], 1)), Err(Error::Ineligible));
    for (status, text) in [
        (None, Some(b"OK".as_slice())),
        (Some(2), None),
        (Some(99), Some(b"OK".as_slice())),
    ] {
        let b = FilterUserTextResponse {
            filtered_text_list: Some(FilteredUserTextList(vec![FilteredUserText {
                result: status,
                filtered_text: text,
                ..Default::default()
            }])),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        assert_eq!(reply(&frame(&b, 2)), Err(Error::Ineligible));
    }
    let b = body(&[b"OK"]);
    let duplicate = [b.as_slice(), b.as_slice()].concat();
    assert_eq!(reply(&frame(&duplicate, 3)), Err(Error::Ineligible));
    let mut wrong = Encoder::new(body_limits());
    wrong.integer([0xd2, 0xcc, 0xf4], 0).unwrap();
    assert_eq!(
        reply(&frame(&wrong.finish().unwrap(), 4)),
        Err(Error::Ineligible)
    );
}

#[test]
fn codec_preserves_unknown_fields_and_enum_without_logging_text() {
    let mut e = Encoder::new(body_limits());
    e.struct_list([0xd2, 0xcc, 0xf4], &[()], |w, _| {
        w.integer([0x92, 0x9c, 0xb4], 99)?;
        w.string([0xd7, 0x4e, 0x34], b"PRIVATE")?;
        w.integer([0xef, 0xff, 0xff], 7)
    })
    .unwrap();
    e.integer([0xef, 0xff, 0xff], 8).unwrap();
    let b = e.finish().unwrap();
    let decoded = FilterUserTextResponse::decode(&b, body_limits()).unwrap();
    assert_eq!(decoded.unknown_field_count(), 2);
    assert_eq!(decoded.encode(body_limits()).unwrap(), b);
    assert!(!format!("{decoded:?} {:?}", decoded.filtered_text_list).contains("PRIVATE"));
}

#[test]
fn bounds_limit_entries_text_depth_and_total_frame() {
    let maximum = body(&vec![b"OK".as_slice(); MAX_ENTRIES]);
    assert_eq!(
        values(&reply(&frame(&maximum, 9)).unwrap(), 9).len(),
        MAX_ENTRIES
    );
    let loose = nfs_heat2::Limits {
        max_collection: MAX_ENTRIES + 1,
        max_byte_string: MAX_INPUT_BYTES + 2,
        ..body_limits()
    };
    for values in [
        vec![b"OK".as_slice(); MAX_ENTRIES + 1],
        vec![&[b'x'; MAX_INPUT_BYTES + 1]],
    ] {
        let b = FilterUserTextResponse {
            filtered_text_list: Some(FilteredUserTextList(
                values
                    .iter()
                    .map(|v| FilteredUserText {
                        result: Some(2),
                        filtered_text: Some(v),
                        ..Default::default()
                    })
                    .collect(),
            )),
            ..Default::default()
        }
        .encode(loose)
        .unwrap();
        assert_eq!(reply(&frame(&b, 1)), Err(Error::Ineligible));
    }
    let mut e = Encoder::new(nfs_heat2::Limits {
        max_depth: 8,
        ..body_limits()
    });
    e.structure([0xef, 0xff, 0xff], |w| {
        w.structure([0xef, 0xff, 0xff], |w| {
            w.structure([0xef, 0xff, 0xff], |w| {
                w.structure([0xef, 0xff, 0xff], |w| w.integer([0xef, 0xff, 0xff], 1))
            })
        })
    })
    .unwrap();
    assert_eq!(
        reply(&frame(&e.finish().unwrap(), 1)),
        Err(Error::Ineligible)
    );
    assert_eq!(
        reply(&vec![0; MAX_BODY_BYTES + HEADER_LEN + 1]),
        Err(Error::Ineligible)
    );
}

#[test]
fn independent_callers_never_share_text_or_correlation() {
    std::thread::scope(|s| {
        for id in 1..=8 {
            s.spawn(move || {
                let text = format!("PLAYER{id}");
                let wire = frame(&body(&[text.as_bytes()]), id);
                for _ in 0..10 {
                    assert_eq!(
                        values(&reply(&wire).unwrap(), id),
                        [(0, text.as_bytes().to_vec())]
                    );
                }
            });
        }
    });
}
