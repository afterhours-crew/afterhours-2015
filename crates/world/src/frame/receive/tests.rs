// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::frame::{self, Direction};

fn group(channel: u8, sequence: u8, messages: Vec<Message<'_>>) -> Messages<'_> {
    Messages {
        state: None,
        groups: vec![Group {
            channel,
            sequence: channel.is_multiple_of(2).then_some(sequence),
            messages,
        }],
    }
}

#[test]
fn fragments_across_frames_keep_order_skip_retries_and_wrap_sequences() {
    let bytes = vec![42; 70];
    let chunks = frame::chunks(23, &bytes).unwrap();
    let first = group(0, 126, vec![Message::Empty, chunks[0].clone()]);
    let (receiver, first_output) = Receiver::default().prepare(&first, 10).unwrap();
    assert!(matches!(
        first_output.as_slice(),
        [Received::Direct(Message::Empty)]
    ));
    assert!(receiver.prepare(&first, 11).unwrap().1.is_empty());
    let gap = group(0, 1, vec![chunks[2].clone()]);
    assert!(matches!(receiver.prepare(&gap, 12), Err(Error::Sequence)));
    // The first chunk repeats at sequence 127; only the suffix is new.
    let next = group(
        0,
        127,
        vec![
            chunks[0].clone(),
            chunks[1].clone(),
            chunks[2].clone(),
            Message::Empty,
        ],
    );
    let (receiver, output) = receiver.prepare(&next, 13).unwrap();
    assert!(matches!(&output[0], Received::Assembled {target:23, bytes:b} if b == &bytes));
    assert!(matches!(output[1], Received::Direct(Message::Empty)));
    assert!(receiver.prepare(&next, 14).unwrap().1.is_empty());
    assert!(receiver.alive(100_000));
}

#[test]
fn channels_and_connections_do_not_share_partial_messages() {
    let chunks = frame::chunks(23, &[1; 40]).unwrap();
    let first = group(0, 0, vec![chunks[0].clone()]);
    let (one, _) = Receiver::default().prepare(&first, 100).unwrap();
    let end = group(0, 1, vec![chunks[1].clone()]);
    assert!(Receiver::default().prepare(&end, 101).is_err());
    assert!(
        one.prepare(&group(2, 0, vec![chunks[1].clone()]), 101)
            .is_err()
    );
    let (one, out) = one
        .prepare(&group(2, 0, vec![Message::Empty]), 101)
        .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(one.prepare(&end, 102).unwrap().1.len(), 1);
}

#[test]
fn missing_chunk_cannot_be_silently_followed_by_commit_or_new_start() {
    let chunks = frame::chunks(47, &[0; 40]).unwrap();
    let first = group(0, 5, vec![chunks[0].clone()]);
    let (receiver, _) = Receiver::default().prepare(&first, 20).unwrap();
    for next in [Message::Empty, chunks[0].clone()] {
        assert!(matches!(
            receiver.prepare(&group(0, 6, vec![next]), 21),
            Err(Error::Interrupted)
        ));
    }
    // Rejected frames never advanced the receiver or consumed its partial body.
    assert!(
        receiver
            .prepare(&group(0, 6, vec![chunks[1].clone()]), 22)
            .is_ok()
    );
    assert!(receiver.alive(20 + RECEIVE_MS - 1));
    assert!(!receiver.alive(20 + RECEIVE_MS));
    assert!(matches!(
        receiver.prepare(&group(0, 6, vec![chunks[1].clone()]), 20 + RECEIVE_MS),
        Err(Error::Timeout)
    ));
    assert!(matches!(receiver.prepare(&first, 19), Err(Error::Clock)));
}

#[test]
fn malformed_chunks_and_bounded_maximum() {
    let chunk = |ordinal, target, last, bytes| Message::Chunk {
        ordinal,
        target,
        last,
        bytes,
    };
    for invalid in [
        chunk(0, Some(23), true, vec![]),
        chunk(0, Some(23), false, vec![0; 31]),
        chunk(0, Some(23), true, vec![0; 33]),
        chunk(0, None, true, vec![0]),
        chunk(1, Some(23), true, vec![0]),
        chunk(0, Some(EX_CHUNK), true, vec![0]),
        chunk(0, Some(REGISTRY_SIZE), true, vec![0]),
    ] {
        assert!(
            Receiver::default()
                .prepare(&group(0, 0, vec![invalid]), 0)
                .is_err()
        );
    }
    let mut receiver = Receiver::default();
    for ordinal in 0..=255u8 {
        let message = chunk(
            ordinal,
            (ordinal == 0).then_some(23),
            ordinal == 255,
            vec![ordinal; 32],
        );
        let (next, out) = receiver
            .prepare(&group(0, ordinal & 127, vec![message]), u64::from(ordinal))
            .unwrap();
        receiver = next;
        if ordinal == 255 {
            assert!(matches!(&out[0], Received::Assembled {bytes,..} if bytes.len()==MAX_BYTES));
        } else {
            assert!(out.is_empty());
        }
    }
}

#[test]
fn reconstructed_known_message_validates_exact_extent_and_partial_byte() {
    let create = Message::CreatePlayer {
        name: b"Test".to_vec(),
        flag: false,
        slot: 0,
    };
    let frame = frame::Builder::new()
        .messages(None, group(0, 0, vec![create.clone()]).groups)
        .build()
        .unwrap();
    let decoded = frame::parse(frame.span(), Direction::FromClient).unwrap();
    assert_eq!(decoded.messages.unwrap().groups[0].messages, [create]);
    assert_eq!(
        frame::assembled_message(frame::EX_STATE, &[0b0101_1111]).unwrap(),
        Message::State(5)
    );
    assert!(frame::assembled_message(frame::EX_STATE, &[0, 0]).is_err());
    assert!(frame::assembled_message(frame::EX_TIME, &[0]).is_err());
    assert!(frame::assembled_message(EX_CHUNK, &[0]).is_err());
    assert!(matches!(
        frame::assembled_message(23, &[1, 2]).unwrap(),
        Message::Opaque { index: 23, .. }
    ));
}
