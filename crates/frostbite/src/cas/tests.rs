// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::block;

fn limits() -> Limits {
    Limits::default()
}

fn command(kind: u32, count: u32) -> [u8; 4] {
    ((kind << 28) | count).to_be_bytes()
}

#[test]
fn blocks_for_each_codec() {
    let payload: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    let raw = [
        block(&payload[..1000], 0),
        block(&payload[1000..1500], 2),
        block(&payload[1500..], 9),
    ]
    .concat();
    assert_eq!(read_blocks(&raw, &limits()).unwrap(), payload);
    let codecs: Vec<u8> = {
        let mut pos = 0;
        let mut out = Vec::new();
        while pos < raw.len() {
            let (_, next, codec) = read_block(&raw, pos).unwrap();
            out.push(codec);
            pos = next;
        }
        out
    };
    assert_eq!(codecs, [0, 2, 9]);
}

#[test]
fn unknown_codec_and_truncation_fail() {
    assert!(matches!(
        read_blocks(&block(b"abc", 5), &limits()),
        Err(Error::Unsupported(_))
    ));
    let stored = block(b"abc", 0);
    assert!(read_blocks(&stored[..stored.len() - 1], &limits()).is_err());
    assert!(read_blocks(&stored[..5], &limits()).is_err());
}

#[test]
fn size_field_mismatch_fails() {
    let mut bad = block(b"abcdef", 0);
    bad[..4].copy_from_slice(&7u32.to_be_bytes());
    assert!(matches!(
        read_blocks(&bad, &limits()),
        Err(Error::Malformed(_))
    ));
    let mut lz4 = block(b"abcdefabcdefabcdef", 9);
    lz4[..4].copy_from_slice(&5u32.to_be_bytes());
    assert!(read_blocks(&lz4, &limits()).is_err());
}

#[test]
fn decompressed_size_is_bounded() {
    let raw = [block(&[1; 600], 9), block(&[2; 600], 9)].concat();
    let small = Limits {
        max_asset_bytes: 1000,
        ..limits()
    };
    assert!(matches!(read_blocks(&raw, &small), Err(Error::Bound(_))));
}

#[test]
fn catalog_entries() {
    let mut entry: Vec<u8> = (0..20).collect();
    entry.extend(32u32.to_le_bytes());
    entry.extend(26u32.to_le_bytes());
    entry.extend(0x101u32.to_le_bytes());
    let mut duplicate = entry.clone();
    duplicate[20..24].copy_from_slice(&99u32.to_le_bytes());
    let data = [CATALOG_MAGIC.as_slice(), &entry, &duplicate].concat();
    let entries = parse_catalog(&data, &limits()).unwrap();
    let sha1: [u8; 20] = std::array::from_fn(|i| i as u8);
    assert_eq!(
        entries[&sha1],
        Record {
            offset: 32,
            size: 26,
            archive: 1
        }
    );
    assert_eq!(entries.len(), 1);
    assert!(
        parse_catalog(
            &[CATALOG_MAGIC.as_slice(), &entry[..31]].concat(),
            &limits()
        )
        .is_err()
    );
    assert!(parse_catalog(b"NyanNyanNyanNya!", &limits()).is_err());
}

#[test]
fn copy_skip_and_delta_blocks() {
    let base = [block(b"AAAA", 0), block(b"BBBB", 0), block(b"CCCC", 0)].concat();
    let delta = [
        command(0, 1).to_vec(),
        command(4, 1).to_vec(),
        command(3, 1).to_vec(),
        block(b"ZZ", 0),
    ]
    .concat();
    assert_eq!(
        read_patched(&base, &delta, &limits()).unwrap(),
        b"AAAAZZCCCC"
    );
}

#[test]
fn merge_type_one() {
    let base = block(b"0123456789", 0);
    let delta = [
        command(1, 1).to_vec(),
        [0, 3, 0, 2].to_vec(),
        block(b"xy", 9),
    ]
    .concat();
    assert_eq!(
        read_patched(&base, &delta, &limits()).unwrap(),
        b"012xy56789"
    );
}

#[test]
fn merge_type_two() {
    let base = block(b"0123456789", 0);
    let edits = [0u8, 2, 1, 2, b'a', b'b'];
    let delta = [
        command(2, edits.len() as u32).to_vec(),
        10u16.to_be_bytes().to_vec(),
        edits.to_vec(),
    ]
    .concat();
    assert_eq!(
        read_patched(&base, &delta, &limits()).unwrap(),
        b"01ab3456789"
    );
}

#[test]
fn unknown_and_truncated_commands_fail() {
    let base = block(b"0", 0);
    assert!(matches!(
        read_patched(&base, &command(7, 0), &limits()),
        Err(Error::Unsupported(_))
    ));
    assert!(read_patched(&base, &[0, 0], &limits()).is_err());
    let truncated_inline = [
        command(2, 4).to_vec(),
        10u16.to_be_bytes().to_vec(),
        vec![0, 2, 1, 9],
    ]
    .concat();
    assert!(read_patched(&block(b"0123456789", 0), &truncated_inline, &limits()).is_err());
    // Copying more base blocks than exist fails rather than reading past the record.
    assert!(read_patched(&base, &command(0, 2), &limits()).is_err());
}

#[test]
fn patched_size_is_bounded() {
    let base = [block(&[1; 600], 0), block(&[2; 600], 0)].concat();
    let small = Limits {
        max_asset_bytes: 1000,
        ..limits()
    };
    assert!(matches!(
        read_patched(&base, &command(0, 2), &small),
        Err(Error::Bound(_))
    ));
}
