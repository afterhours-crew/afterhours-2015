// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn bits(w: &BitWriter, offset: usize, width: u8) -> u32 {
    w.span().read_u32(offset, width).unwrap()
}

#[test]
fn initializer_has_independent_field_offsets_and_parses_back() {
    let w = initializer(1800, 60.0, 0);
    assert_eq!(w.len(), 232, "232-bit single-fragment frame");
    let f = |o, n| bits(&w, o, n);
    assert_eq!(f(0, 6), 3, "mask: TimeSync and MessageEX");
    assert_eq!((f(6, 1), f(7, 1)), (1, 0), "init branch, flag 0");
    assert_eq!(f(8, 32), 60.0f32.to_bits());
    assert_eq!((f(40, 8), f(48, 4)), (16, 1));
    assert_eq!(f(52, 32), 5.0f32.to_bits());
    assert_eq!(f(84, 4), 2, "two messages");
    assert_eq!((f(88, 1), f(89, 12)), (1, 1728), "channel state advertised");
    assert_eq!(
        (f(101, 1), f(102, 3), f(105, 7)),
        (1, 0, 0),
        "group, channel 0, sequence 0"
    );
    assert_eq!(f(112, 7), 80);
    assert_eq!(
        (f(119, 1), f(120, 32), f(152, 32)),
        (1, 1800, 60.0f32.to_bits())
    );
    assert_eq!((f(184, 1), f(185, 7)), (1, 56));
    assert_eq!(f(192, 32), u32::MAX, "sentinel -1");
    assert_eq!(
        (f(224, 1), f(225, 7)),
        (0, 0),
        "no more messages; zero alignment"
    );

    let frame = parse(w.span(), Direction::FromHost).unwrap();
    assert_eq!(frame.mask, 3);
    assert_eq!(frame.time_sync, Some(TimeSync::Init(TimeSyncInit::DEFAULT)));
    let m = frame.messages.unwrap();
    assert_eq!(m.state, Some(INITIAL_CHANNEL_STATE));
    assert_eq!(
        m.groups,
        vec![Group {
            channel: 0,
            sequence: Some(0),
            messages: vec![
                Message::Time {
                    flag: true,
                    tick: 1800,
                    time: 60.0
                },
                Message::Sentinel(-1)
            ],
        }]
    );
    assert!(frame.complete && frame.ghost.is_none() && frame.movement.is_none());
}

fn client_frame() -> (BitWriter, Vec<u8>) {
    let inner: Vec<u8> = (0..36).map(|i| i as u8 * 3).collect();
    let builder = Builder::new().messages(
        Some(0),
        vec![Group {
            channel: 0,
            sequence: Some(0),
            messages: vec![
                Message::Chunk {
                    ordinal: 0,
                    target: Some(23),
                    last: false,
                    bytes: inner[..32].to_vec(),
                },
                Message::Chunk {
                    ordinal: 1,
                    target: None,
                    last: true,
                    bytes: inner[32..].to_vec(),
                },
                Message::CreatePlayer {
                    name: b"driver1".to_vec(),
                    flag: true,
                    slot: 0,
                },
            ],
        }],
    );
    let mut w = BitWriter::new();
    w.put(0b1010, 6);
    builder.append(&mut w).unwrap();
    assert_eq!(w.len(), 451, "the message group ends at bit 451");
    w.put(0, 1).put(0, 1).put(1, 13).put(0, 1).put(0x1_2345, 33);
    w.align();
    (w, inner)
}

#[test]
fn client_frame_parses_chunks_create_player_and_the_ghost_prefix() {
    let (w, inner) = client_frame();
    assert_eq!(w.len(), 504, "504-bit frame");
    let frame = parse(w.span(), Direction::FromClient).unwrap();
    assert_eq!(frame.mask, 0b1010);
    assert!(frame.time_sync.is_none() && frame.movement.is_none());
    let m = frame.messages.as_ref().unwrap();
    assert_eq!(m.state, Some(0));
    let [group] = m.groups.as_slice() else {
        panic!()
    };
    assert_eq!((group.channel, group.sequence), (0, Some(0)));
    assert_eq!(group.messages.len(), 3);
    assert_eq!(
        reassemble(&[&group.messages[0], &group.messages[1]]),
        Some((23, inner))
    );
    assert_eq!(
        group.messages[2],
        Message::CreatePlayer {
            name: b"driver1".to_vec(),
            flag: true,
            slot: 0
        }
    );
    let ghost = frame.ghost.as_ref().unwrap();
    assert_eq!(ghost.prefix.record_count(), 1);
    assert!(ghost.prefix.deleted().is_empty());
    assert_eq!(
        ghost.records.len(),
        33 + 4,
        "opaque record bits plus alignment"
    );
    assert!(frame.complete, "Ghost is the last handler");
}

#[test]
fn chunking_round_trips_through_reassembly() {
    for len in [1usize, 31, 32, 33, 64, 86, 252, 651] {
        let bytes: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
        let parts = chunks(85, &bytes).unwrap();
        assert_eq!(parts.len(), len.div_ceil(32));
        assert!(matches!(
            parts[0],
            Message::Chunk {
                ordinal: 0,
                target: Some(85),
                ..
            }
        ));
        let refs: Vec<&Message<'_>> = parts.iter().collect();
        assert_eq!(reassemble(&refs), Some((85, bytes)));
        let group = Group {
            channel: 0,
            sequence: Some(3),
            messages: parts[..parts.len().min(CHUNKS_PER_FRAME)].to_vec(),
        };
        let built = Builder::new()
            .messages(Some(INITIAL_CHANNEL_STATE), vec![group])
            .build()
            .unwrap();
        let parsed = parse(built.span(), Direction::FromHost).unwrap();
        assert_eq!(
            parsed.messages.unwrap().groups[0].messages.len(),
            parts.len().min(15)
        );
    }
    assert!(chunks(85, &[]).is_none());
    assert!(chunks(85, &[0; 32 * 64 + 1]).is_none());
    assert!(chunks(107, &[1]).is_none());
}

#[test]
fn reassembly_rejects_wrong_order_and_bad_chunk_shapes() {
    let (w, _) = client_frame();
    let frame = parse(w.span(), Direction::FromClient).unwrap();
    let g = &frame.messages.as_ref().unwrap().groups[0];
    assert_eq!(reassemble(&[&g.messages[1], &g.messages[0]]), None);
    assert_eq!(reassemble(&[&g.messages[0]]), None, "missing final chunk");
    assert_eq!(reassemble(&[&g.messages[2]]), None);
    let bad = [
        Message::Chunk {
            ordinal: 0,
            target: Some(23),
            last: false,
            bytes: vec![0; 31],
        },
        Message::Chunk {
            ordinal: 1,
            target: Some(23),
            last: true,
            bytes: vec![0; 4],
        },
        Message::Chunk {
            ordinal: 0,
            target: Some(23),
            last: true,
            bytes: vec![],
        },
        Message::CreatePlayer {
            name: vec![],
            flag: false,
            slot: 9,
        },
    ];
    for m in bad {
        let b = Builder::new().messages(
            None,
            vec![Group {
                channel: 1,
                sequence: None,
                messages: vec![m],
            }],
        );
        assert_eq!(b.build().err(), Some(Error::Shape));
    }
    let no_sequence = Builder::new().messages(
        None,
        vec![Group {
            channel: 0,
            sequence: None,
            messages: vec![Message::Empty],
        }],
    );
    assert_eq!(no_sequence.build().err(), Some(Error::Shape));
}

#[test]
fn unknown_shapes_and_truncation_stop_the_parse() {
    let mut w = BitWriter::new();
    w.put(0b10, 6)
        .put(1, 4)
        .put(0, 1)
        .put(1, 1)
        .put(1, 3)
        .put(99, 7);
    assert_eq!(
        parse(w.span(), Direction::FromClient).err(),
        Some(Error::Unsupported(99))
    );
    let mut zero = BitWriter::new();
    zero.put(0b10, 6).put(1, 4).put(0, 1).put(0, 1);
    assert_eq!(
        parse(zero.span(), Direction::FromClient).err(),
        Some(Error::Shape)
    );
    let mut short = BitWriter::new();
    short.put(0b1, 6).put(5, 10);
    assert_eq!(
        parse(short.span(), Direction::FromClient).err(),
        Some(Error::Truncated)
    );
    let mut files = BitWriter::new();
    files.put(0b1_0000, 6).put(1, 2);
    assert_eq!(
        parse(files.span(), Direction::FromClient).err(),
        Some(Error::File(crate::files::Error::Truncated))
    );
    let empty = BitWriter::new();
    assert_eq!(
        parse(empty.span(), Direction::FromClient).err(),
        Some(Error::Truncated)
    );
}

#[test]
fn direction_specific_handlers_and_opaque_bodies_parse() {
    let mut w = BitWriter::new();
    w.put(0b101, 6).put(0x3_0000_0005, 38);
    w.put(0xabcd, 96 - 32).put(0x1234_5678, 32).put(1, 6);
    w.put(0x2a, 16).put(3, 4).put(8, 16).put(0x5a, 8);
    w.align();
    let frame = parse(w.span(), Direction::FromClient).unwrap();
    assert_eq!(frame.time_sync, Some(TimeSync::Sample(0x3_0000_0005)));
    let mv = frame.movement.unwrap();
    assert_eq!(mv.header.unwrap().len(), 96);
    assert_eq!((mv.records[0].id, mv.records[0].component), (0x2a, 3));
    assert_eq!(mv.records[0].data.read_u32(0, 8).unwrap(), 0x5a);
    let mut h = BitWriter::new();
    h.put(0b101, 6).put(0, 1).put(1, 38).put(2, 38).put(3, 38);
    h.put(1, 8).put(7, 16).put(2, 4).put(0xff, 8);
    h.align();
    let frame = parse(h.span(), Direction::FromHost).unwrap();
    assert_eq!(frame.time_sync, Some(TimeSync::Times([1, 2, 3])));
    let mv = frame.movement.unwrap();
    assert!(mv.header.is_none());
    assert_eq!((mv.records[0].id, mv.records[0].data.len()), (7, 8));
    let mut o = BitWriter::new();
    o.put(0b10, 6).put(2, 4).put(0, 1).put(1, 1).put(1, 3);
    o.put(70, 7).put(0xdead_beef, 32).put(1, 1);
    o.put(47, 7)
        .put(0, 32)
        .put(0, 32)
        .put(0, 8)
        .put(2, 9)
        .put(0xbeef, 16)
        .put(0, 1);
    o.align();
    let frame = parse(o.span(), Direction::FromClient).unwrap();
    let g = &frame.messages.unwrap().groups[0];
    assert_eq!((g.channel, g.sequence), (1, None));
    assert!(matches!(g.messages[0], Message::Opaque { index: 70, body } if body.len() == 32));
    assert!(
        matches!(g.messages[1], Message::Opaque { index: 47, body } if body.len() == 64 + 8 + 9 + 16)
    );
    let mut gh = BitWriter::new();
    gh.put(0b11000, 6)
        .put(0, 1)
        .put(0, 1)
        .put(0, 13)
        .put(1, 2)
        .put(0, 10)
        .put(0, 24);
    gh.align();
    let frame = parse(gh.span(), Direction::FromClient).unwrap();
    assert!(frame.complete);
    assert_eq!(frame.files.len(), 1);
    assert_eq!(frame.ghost.unwrap().prefix.record_count(), 0);
}

#[test]
fn host_time_sync_times_build_and_parse() {
    let w = Builder::new()
        .time_sync_times([97228, 97250, 97286])
        .build()
        .unwrap();
    assert_eq!(w.len(), 128, "128-bit Times frames");
    let parsed = parse(w.span(), Direction::FromHost).unwrap();
    assert_eq!(parsed.mask, 1);
    assert_eq!(
        parsed.time_sync,
        Some(TimeSync::Times([97228, 97250, 97286]))
    );
    assert!(parsed.messages.is_none());
}

#[test]
fn state_reports_and_inline_collections_round_trip() {
    let w = Builder::new()
        .messages(
            None,
            vec![Group {
                channel: 0,
                sequence: Some(3),
                messages: vec![Message::State(7)],
            }],
        )
        .build()
        .unwrap();
    assert_eq!(w.len(), 40, "34 bits of content, zero-padded");
    assert_eq!(bits(&w, 22, 7), u32::from(EX_STATE));
    assert_eq!(bits(&w, 29, 4), 7);
    let parsed = parse(w.span(), Direction::FromHost).unwrap();
    assert_eq!(parsed.rest, 34);
    let group = &parsed.messages.unwrap().groups[0];
    assert_eq!(group.messages, vec![Message::State(7)]);
    assert!(
        Builder::new()
            .messages(
                None,
                vec![Group {
                    channel: 0,
                    sequence: Some(0),
                    messages: vec![Message::State(11)],
                }]
            )
            .build()
            .is_err(),
        "values above 10 are not a state report"
    );
    let collection = |enum3: u64| {
        let mut w = BitWriter::new();
        w.put(0b10, 6)
            .put(1, 4)
            .put(0, 1)
            .put(1, 1)
            .put(0, 3)
            .put(0, 7);
        w.put(u64::from(EX_COLLECTION), 7);
        w.put(0x1234, 34).put(1, 32);
        w.put(5, 16)
            .put(6, 16)
            .put(0, 16)
            .put(9, 32)
            .put(254, 8)
            .put(0, 1);
        w.put(enum3, 3).put(0, 32).put(1, 1).put(0, 2).put(0, 32);
        w.put(3, 10).put_bytes(b"abc");
        w.put(0, 1);
        w.align();
        w
    };
    let w = collection(3);
    let parsed = parse(w.span(), Direction::FromHost).unwrap();
    let group = &parsed.messages.unwrap().groups[0];
    assert!(matches!(
        group.messages[0],
        Message::Opaque { index: EX_COLLECTION, body } if body.len() == 34 + 32 + 159 + 34
    ));
    assert_eq!(
        parse(collection(7).span(), Direction::FromHost).err(),
        Some(Error::Shape)
    );
    let mut w = BitWriter::new();
    w.put(6, 34).put(2, 32);
    for h in [7u64, 9] {
        w.put(h, 16)
            .put(h + 1, 16)
            .put(0, 16)
            .put(h + 4, 32)
            .put(254, 8);
        w.put(0, 1)
            .put(3, 3)
            .put(0, 32)
            .put(1, 1)
            .put(0, 2)
            .put(0, 32);
        w.put(0, 10);
    }
    w.align();
    assert_eq!(collection_handles(w.bytes()), Some(vec![7, 9]));
    assert_eq!(collection_handles(&[0xff; 12]), None);
    assert_eq!(
        sublevel_report_ids(BitSpan::new(&[0, 0, 0, 1, 0, 0, 0, 1, 0, 42], 0, 80).unwrap()),
        vec![42]
    );
}

#[test]
fn host_movement_grants_build_in_the_official_layout_and_parse() {
    // E101 host frame at 37986 ms: cars 293 and 294, one selector 0 each.
    let w = Builder::new()
        .movement(vec![(293, vec![0]), (294, vec![0])])
        .build()
        .unwrap();
    assert_eq!(bits(&w, 0, 6), 1 << MOVEMENT);
    assert_eq!(bits(&w, 6, 8), 2);
    assert_eq!(
        (bits(&w, 14, 16), bits(&w, 30, 4), bits(&w, 34, 4)),
        (293, 1, 0)
    );
    assert_eq!(
        (bits(&w, 38, 16), bits(&w, 54, 4), bits(&w, 58, 4)),
        (294, 1, 0)
    );
    assert_eq!(w.len(), 64);
    let parsed = parse(w.span(), Direction::FromHost).unwrap();
    let movement = parsed.movement.unwrap();
    assert_eq!(movement.header, None);
    let records: Vec<_> = movement
        .records
        .iter()
        .map(|r| (r.id, r.data.len(), r.data.read_u32(0, 4).unwrap()))
        .collect();
    assert_eq!(records, vec![(293, 4, 0), (294, 4, 0)]);

    // Combined with messages, the section follows the message groups.
    let both = Builder::new()
        .messages(
            None,
            vec![Group {
                channel: 0,
                sequence: Some(3),
                messages: vec![Message::Empty],
            }],
        )
        .movement(vec![(424, vec![0, 2])])
        .build()
        .unwrap();
    let parsed = parse(both.span(), Direction::FromHost).unwrap();
    assert_eq!(parsed.mask, (1 << MESSAGES) | (1 << MOVEMENT));
    let record = &parsed.movement.unwrap().records[0];
    assert_eq!((record.id, record.data.len()), (424, 8));
    assert_eq!(record.data.read_u32(4, 4).unwrap(), 2);

    for bad in [
        vec![(1, vec![])],
        vec![(1, vec![16])],
        vec![(1, vec![0; 16])],
    ] {
        assert_eq!(
            Builder::new().movement(bad).build().err(),
            Some(Error::Shape)
        );
    }
    let many = (0..=MAX_MOVEMENT_RECORDS as u16)
        .map(|i| (i, vec![0]))
        .collect();
    assert_eq!(
        Builder::new().movement(many).build().err(),
        Some(Error::Bound)
    );
}
