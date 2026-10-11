// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    bits::BitWriter,
    replication::{Initial, Kind, Record, Update as ReplicationUpdate},
};
fn record(groups: Option<Vec<Group>>) -> Record {
    Record {
        id: 17,
        initial: Some(Initial::ParticipantLocationMap),
        update: ReplicationUpdate::ParticipantLocationMap(Update { groups }),
    }
}
fn entry() -> Entry {
    Entry {
        participant: 9,
        x: Coordinate::from_raw(0x800).unwrap(),
        z: Coordinate::from_raw(0x123).unwrap(),
    }
}
#[test]
fn constructed_wire_empty_initial_unknown_group_and_concatenated_records() {
    let record = record(Some(vec![Group {
        value: 231,
        entries: vec![entry()],
    }]));
    let mut expected = BitWriter::new();
    for (v, n) in [
        (17, 13),
        (1, 1),
        (12, 5),
        (1, 1),
        (1, 8),
        (1, 16),
        (231, 8),
        (9, 13),
        (0x800, 12),
        (0x123, 12),
    ] {
        expected.put(v, n);
    }
    let wire = record.encode().unwrap();
    assert_eq!(wire, expected);
    let decoded = Record::decode(expected.span(), None).unwrap();
    assert_eq!(decoded.initial_bits, Some(19));
    assert_eq!(decoded.bits, 89);
    assert_eq!(decoded.record, record);
    for end in 0..wire.len() {
        assert!(
            Record::decode(wire.span().slice(0, end).unwrap(), None).is_err(),
            "prefix {end}"
        );
    }
    let absent = Record {
        id: 17,
        initial: None,
        update: ReplicationUpdate::ParticipantLocationMap(Update::default()),
    };
    let mut stream = expected.clone();
    stream.put_span(absent.encode().unwrap().span());
    let first = Record::decode(stream.span(), None).unwrap();
    let second = Record::decode(
        stream
            .span()
            .slice(first.bits, stream.len() - first.bits)
            .unwrap(),
        Some(Kind::ParticipantLocationMap),
    )
    .unwrap();
    assert_eq!(second.record, absent);
    assert_eq!(first.bits + second.bits, stream.len());
    assert_ne!(record_empty(false), record_empty(true));
}
fn record_empty(present: bool) -> BitWriter {
    record(present.then(Vec::new)).encode().unwrap()
}
#[test]
fn hostile_counts_and_invalid_references_are_bounded_before_allocating() {
    for count in [513, 65535] {
        let mut wire = BitWriter::new();
        for (v, n) in [(17, 13), (1, 1), (12, 5), (1, 1), (1, 8), (count, 16)] {
            wire.put(v, n);
        }
        assert_eq!(
            Record::decode(wire.span(), None).map(|_| ()),
            Err(Error::Bound)
        );
    }
    let group = Group {
        value: 1,
        entries: vec![entry(); MAX_ENTRIES],
    };
    assert!(record(Some(vec![group.clone()])).encode().is_ok());
    assert_eq!(
        record(Some(vec![
            group,
            Group {
                value: 2,
                entries: vec![entry()]
            }
        ]))
        .encode(),
        Err(Error::Bound)
    );
    assert_eq!(
        record(Some(vec![
            Group {
                value: 0,
                entries: vec![]
            };
            256
        ]))
        .encode(),
        Err(Error::Bound)
    );
    let mut invalid = entry();
    invalid.participant = 8192;
    assert_eq!(
        record(Some(vec![Group {
            value: 0,
            entries: vec![invalid]
        }]))
        .encode(),
        Err(Error::Shape)
    );
}
#[test]
fn coordinates_round_clamp_retain_negative_zero_and_reject_nonfinite() {
    for (value, raw) in [
        (0., 0),
        (-0., 0x800),
        (8000., 0x7ff),
        (-8000., 0xfff),
        (16000., 0x7ff),
        (-16000., 0xfff),
        (4000., 1024),
        (-4000., 3072),
    ] {
        assert_eq!(Coordinate::from_world(value).unwrap().raw(), raw);
    }
    assert_eq!(
        Coordinate::from_raw(0x800).unwrap().to_world().to_bits(),
        (-0f32).to_bits()
    );
    for raw in 0..4096 {
        assert_eq!(
            Coordinate::from_world(Coordinate::from_raw(raw).unwrap().to_world())
                .unwrap()
                .raw(),
            raw
        );
    }
    assert_eq!(Coordinate::from_raw(4096), Err(Error::Shape));
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(Coordinate::from_world(value), Err(Error::Shape));
    }
}
