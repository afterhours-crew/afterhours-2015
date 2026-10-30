// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::challenges::{native_u64, put_native_u64};
use super::{BitWriter, Error, MAX_SPEED_WALL_ENTRIES, Reader};

pub const EX_RECOMMENDATION_SYNC: u8 = 89;

/// Fields in wire order. The third word is the receiver's per-player map key.
/// Preserve the other words and float representation without assigning catalog
/// semantics. The native 64-bit integer representation sends its low word first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecommendationEntry {
    pub word: u32,
    pub value_bits: u32,
    pub key: u32,
    pub wide_word: u64,
    pub flags: [bool; 2],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecommendationSync {
    pub persona: u64,
    pub entries: Vec<RecommendationEntry>,
}

impl RecommendationSync {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let persona = native_u64(r)?;
        let count = r.take(32)? as usize;
        if count > MAX_SPEED_WALL_ENTRIES {
            return Err(Error::Bound);
        }
        if r.rest()?.len() < count * 162 {
            return Err(Error::Truncated);
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(RecommendationEntry {
                word: r.take(32)?,
                value_bits: r.take(32)?,
                key: r.take(32)?,
                wide_word: native_u64(r)?,
                flags: [r.take(1)? != 0, r.take(1)? != 0],
            });
        }
        Ok(Self { persona, entries })
    }

    pub(super) fn write(&self, w: &mut BitWriter) -> Result<(), Error> {
        if self.entries.len() > MAX_SPEED_WALL_ENTRIES {
            return Err(Error::Bound);
        }
        put_native_u64(w, self.persona);
        w.put(self.entries.len() as u64, 32);
        for entry in &self.entries {
            w.put(entry.word.into(), 32)
                .put(entry.value_bits.into(), 32)
                .put(entry.key.into(), 32);
            put_native_u64(w, entry.wide_word);
            w.put_bool(entry.flags[0]).put_bool(entry.flags[1]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Builder, Direction, Group, Message, assembled_message, parse};
    use super::*;

    #[test]
    fn recommendation_sync_preserves_word_order_unaligned_entries_and_neighbors() {
        // Hand-written field order, deliberately distinct low/high words and
        // a NaN payload: numeric normalization would lose information here.
        let mut body = BitWriter::new();
        body.put(0x89abcdef, 32).put(0x01234567, 32).put(2, 32);
        for (key, flags) in [(17, [true, false]), (19, [false, true])] {
            body.put(0x12345678, 32)
                .put(0x7fc01234, 32)
                .put(key, 32)
                .put(0x76543210, 32)
                .put(0xfedcba98, 32)
                .put_bool(flags[0])
                .put_bool(flags[1]);
        }
        let parsed = assembled_message(EX_RECOMMENDATION_SYNC, body.bytes()).unwrap();
        let Message::RecommendationSync(ref sync) = parsed else {
            panic!("typed recommendation")
        };
        assert_eq!(sync.persona, 0x0123456789abcdef);
        assert_eq!(sync.entries.len(), 2);
        assert_eq!(
            sync.entries[0],
            RecommendationEntry {
                word: 0x12345678,
                value_bits: 0x7fc01234,
                key: 17,
                wide_word: 0xfedcba9876543210,
                flags: [true, false],
            }
        );
        assert_eq!(
            (sync.entries[1].key, sync.entries[1].flags),
            (19, [false, true])
        );
        let mut encoded = BitWriter::new();
        sync.write(&mut encoded).unwrap();
        assert_eq!(encoded, body);
        let frame = Builder::new()
            .messages(
                None,
                vec![Group {
                    channel: 0,
                    sequence: Some(126),
                    messages: vec![Message::Empty, parsed, Message::Sentinel(-1)],
                }],
            )
            .build()
            .unwrap();
        let decoded = parse(frame.span(), Direction::FromClient).unwrap();
        assert!(decoded.complete);
        let messages = &decoded.messages.as_ref().unwrap().groups[0].messages;
        assert!(matches!(
            messages.as_slice(),
            [
                Message::Empty,
                Message::RecommendationSync(_),
                Message::Sentinel(-1)
            ]
        ));
        assert_eq!(
            messages[1],
            assembled_message(EX_RECOMMENDATION_SYNC, body.bytes()).unwrap()
        );
        let chunked = Builder::new()
            .messages(
                None,
                vec![Group {
                    channel: 0,
                    sequence: Some(0),
                    messages: super::super::chunks(EX_RECOMMENDATION_SYNC, body.bytes()).unwrap(),
                }],
            )
            .build()
            .unwrap();
        let chunks = parse(chunked.span(), Direction::FromClient).unwrap();
        let (_, received) = super::super::receive::Receiver::default()
            .prepare(chunks.messages.as_ref().unwrap(), 0)
            .unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].decode().unwrap(), messages[1]);
    }

    #[test]
    fn recommendation_sync_requires_complete_bounded_bodies_and_keeps_empty_requests() {
        for count in [0, 1, MAX_SPEED_WALL_ENTRIES] {
            let request = RecommendationSync {
                persona: 7,
                entries: vec![
                    RecommendationEntry {
                        word: 1,
                        value_bits: 0x80000000,
                        key: 2,
                        wide_word: 9,
                        flags: [true, true],
                    };
                    count
                ],
            };
            let mut wire = BitWriter::new();
            request.write(&mut wire).unwrap();
            let mut reader = Reader {
                span: wire.span(),
                pos: 0,
            };
            assert_eq!(RecommendationSync::read(&mut reader).unwrap(), request);
            assert!(reader.rest().unwrap().is_empty());
            if wire.bytes().len() <= super::super::receive::MAX_BYTES {
                assert_eq!(
                    assembled_message(EX_RECOMMENDATION_SYNC, wire.bytes()).unwrap(),
                    Message::RecommendationSync(request)
                );
            } else {
                // Chunk reassembly has its own smaller byte bound, which must
                // remain enforced even though the body codec accepts the count.
                assert_eq!(
                    assembled_message(EX_RECOMMENDATION_SYNC, wire.bytes()),
                    Err(Error::Bound)
                );
            }
            if count <= 1 {
                for length in 0..wire.len() {
                    let span = wire.span().slice(0, length).unwrap();
                    assert!(RecommendationSync::read(&mut Reader { span, pos: 0 }).is_err());
                }
            }
            let mut extra = wire.bytes().to_vec();
            extra.push(0);
            assert!(assembled_message(EX_RECOMMENDATION_SYNC, &extra).is_err());
        }
        let mut oversized = BitWriter::new();
        oversized
            .put(0, 64)
            .put((MAX_SPEED_WALL_ENTRIES + 1) as u64, 32);
        assert_eq!(
            assembled_message(EX_RECOMMENDATION_SYNC, oversized.bytes()),
            Err(Error::Bound)
        );
        let too_many = RecommendationSync {
            persona: 7,
            entries: vec![
                RecommendationEntry {
                    word: 0,
                    value_bits: 0,
                    key: 0,
                    wide_word: 0,
                    flags: [false; 2],
                };
                MAX_SPEED_WALL_ENTRIES + 1
            ],
        };
        assert_eq!(too_many.write(&mut BitWriter::new()), Err(Error::Bound));
    }
}
