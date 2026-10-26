// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{BitWriter, Error, Reader};

/// Local bound shared by the challenge synchronization collections.
pub const MAX_CHALLENGE_SYNC_ENTRIES: usize = 1024;
pub const EX_CHALLENGE_PLAYER_SYNC: u8 = 23;
pub const EX_CHALLENGE_AWARDS: u8 = 50;
pub const EX_CHALLENGE_FULL: u8 = 104;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChallengePlayerSync {
    pub persona: u64,
    /// Native TimeValue bits. No epoch conversion or rotation is implied.
    pub monthly_start: u64,
    pub day_id: u32,
    pub challenge_ids: Vec<u32>,
}

/// Definition words in native order. Their wire identity is preserved without
/// assigning unresolved catalog meanings; the flag follows the first word.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChallengeFullEntry {
    pub words: [u32; 7],
    pub flag: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChallengeFull {
    pub entries: Vec<ChallengeFullEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChallengeAwards {
    pub ids: Vec<u64>,
}

fn native_u64(r: &mut Reader<'_>) -> Result<u64, Error> {
    let low = r.take(32)?;
    Ok(u64::from(low) | (u64::from(r.take(32)?) << 32))
}

fn put_native_u64(w: &mut BitWriter, value: u64) {
    w.put(value & u64::from(u32::MAX), 32).put(value >> 32, 32);
}

fn count(r: &mut Reader<'_>, entry_bits: usize) -> Result<usize, Error> {
    let count = r.take(32)? as usize;
    if count > MAX_CHALLENGE_SYNC_ENTRIES {
        return Err(Error::Bound);
    }
    if r.rest()?.len() < count * entry_bits {
        return Err(Error::Truncated);
    }
    Ok(count)
}

fn put_count(w: &mut BitWriter, count: usize) -> Result<(), Error> {
    if count > MAX_CHALLENGE_SYNC_ENTRIES {
        return Err(Error::Bound);
    }
    w.put(count as u64, 32);
    Ok(())
}

impl ChallengePlayerSync {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let persona = native_u64(r)?;
        let monthly_start = native_u64(r)?;
        let day_id = r.take(32)?;
        let count = count(r, 32)?;
        let challenge_ids = (0..count).map(|_| r.take(32)).collect::<Result<_, _>>()?;
        Ok(Self {
            persona,
            monthly_start,
            day_id,
            challenge_ids,
        })
    }

    pub(super) fn write(&self, w: &mut BitWriter) -> Result<(), Error> {
        if self.challenge_ids.len() > MAX_CHALLENGE_SYNC_ENTRIES {
            return Err(Error::Bound);
        }
        put_native_u64(w, self.persona);
        put_native_u64(w, self.monthly_start);
        w.put(u64::from(self.day_id), 32);
        put_count(w, self.challenge_ids.len())?;
        for &id in &self.challenge_ids {
            w.put(u64::from(id), 32);
        }
        Ok(())
    }
}

impl ChallengeFull {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let count = count(r, 225)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let first = r.take(32)?;
            let flag = r.take(1)? != 0;
            entries.push(ChallengeFullEntry {
                words: [
                    first,
                    r.take(32)?,
                    r.take(32)?,
                    r.take(32)?,
                    r.take(32)?,
                    r.take(32)?,
                    r.take(32)?,
                ],
                flag,
            });
        }
        Ok(Self { entries })
    }

    pub(super) fn write(&self, w: &mut BitWriter) -> Result<(), Error> {
        put_count(w, self.entries.len())?;
        for entry in &self.entries {
            w.put(u64::from(entry.words[0]), 32).put_bool(entry.flag);
            for &word in &entry.words[1..] {
                w.put(u64::from(word), 32);
            }
        }
        Ok(())
    }
}

impl ChallengeAwards {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let count = count(r, 64)?;
        Ok(Self {
            ids: (0..count)
                .map(|_| native_u64(r))
                .collect::<Result<_, _>>()?,
        })
    }

    pub(super) fn write(&self, w: &mut BitWriter) -> Result<(), Error> {
        put_count(w, self.ids.len())?;
        for &id in &self.ids {
            put_native_u64(w, id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Builder, Direction, Group, Message, assembled_message, message, parse};
    use super::*;
    use nfs_protocol::world::BitSpan;

    fn check(expected: Message<'_>, wire: BitWriter) {
        let mut r = Reader {
            span: wire.span(),
            pos: 0,
        };
        assert_eq!(message(&mut r, expected.index()).unwrap(), expected);
        assert_eq!(r.pos, wire.len());
        assert_eq!(
            assembled_message(expected.index(), wire.bytes()).unwrap(),
            expected
        );
        let mut encoded = BitWriter::new();
        Builder::put_message(&mut encoded, &expected).unwrap();
        let mut expected_wire = BitWriter::new();
        expected_wire
            .put(expected.index().into(), 7)
            .put_span(wire.span());
        assert_eq!(encoded, expected_wire);
        for bits in 0..wire.len() {
            let mut r = Reader {
                span: BitSpan::new(wire.bytes(), 0, bits).unwrap(),
                pos: 0,
            };
            assert!(
                message(&mut r, expected.index()).is_err(),
                "accepted prefix {bits}"
            );
        }
        let mut trailing = wire.clone();
        trailing.align();
        trailing.put(0, 8);
        assert_eq!(
            assembled_message(expected.index(), trailing.bytes()),
            Err(Error::Shape)
        );
        let following = Message::Sentinel(-13);
        let frame = Builder::new()
            .messages(
                None,
                vec![Group {
                    channel: 0,
                    sequence: Some(0),
                    messages: vec![expected.clone(), following.clone()],
                }],
            )
            .build()
            .unwrap();
        let parsed = parse(frame.span(), Direction::FromHost).unwrap();
        assert_eq!(
            parsed.messages.unwrap().groups[0].messages,
            vec![expected, following]
        );
    }

    #[test]
    fn challenge_request_preserves_low_word_first_identity_and_period() {
        let mut wire = BitWriter::new();
        wire.put(0x89abcdef, 32)
            .put(0x12345678, 32)
            .put(0x76543210, 32)
            .put(0xfedcba98, 32)
            .put(17, 32)
            .put(3, 32)
            .put(9, 32)
            .put(0, 32)
            .put(u32::MAX.into(), 32);
        check(
            Message::ChallengePlayerSync(ChallengePlayerSync {
                persona: 0x1234567889abcdef,
                monthly_start: 0xfedcba9876543210,
                day_id: 17,
                challenge_ids: vec![9, 0, u32::MAX],
            }),
            wire,
        );
    }

    #[test]
    fn full_challenge_records_keep_the_flag_between_first_and_second_words() {
        let mut wire = BitWriter::new();
        wire.put(2, 32)
            .put(19, 32)
            .put(1, 1)
            .put(2, 32)
            .put(3, 32)
            .put(4, 32)
            .put(5, 32)
            .put(6, 32)
            .put(u32::MAX.into(), 32)
            .put(20, 32)
            .put(0, 1)
            .put(11, 32)
            .put(12, 32)
            .put(13, 32)
            .put(14, 32)
            .put(15, 32)
            .put(16, 32);
        check(
            Message::ChallengeFull(ChallengeFull {
                entries: vec![
                    ChallengeFullEntry {
                        words: [19, 2, 3, 4, 5, 6, u32::MAX],
                        flag: true,
                    },
                    ChallengeFullEntry {
                        words: [20, 11, 12, 13, 14, 15, 16],
                        flag: false,
                    },
                ],
            }),
            wire,
        );
    }

    #[test]
    fn challenge_award_ids_use_the_native_u64_word_order() {
        let mut wire = BitWriter::new();
        wire.put(2, 32)
            .put(0x11223344, 32)
            .put(0xaabbccdd, 32)
            .put(0, 32)
            .put(7, 32);
        check(
            Message::ChallengeAwards(ChallengeAwards {
                ids: vec![0xaabbccdd11223344, 7 << 32],
            }),
            wire,
        );
    }

    #[test]
    fn synchronization_counts_are_bounded_before_allocation() {
        for index in [
            EX_CHALLENGE_PLAYER_SYNC,
            EX_CHALLENGE_FULL,
            EX_CHALLENGE_AWARDS,
        ] {
            let mut wire = BitWriter::new();
            if index == EX_CHALLENGE_PLAYER_SYNC {
                wire.put(0, 64).put(0, 64).put(0, 32);
            }
            let empty_prefix = wire.clone();
            wire.put((MAX_CHALLENGE_SYNC_ENTRIES + 1) as u64, 32);
            let mut r = Reader {
                span: wire.span(),
                pos: 0,
            };
            assert_eq!(message(&mut r, index), Err(Error::Bound));
            let mut empty = empty_prefix;
            empty.put(0, 32);
            let mut r = Reader {
                span: empty.span(),
                pos: 0,
            };
            let value = message(&mut r, index).unwrap();
            assert_eq!(r.pos, empty.len());
            let mut encoded = BitWriter::new();
            Builder::put_message(&mut encoded, &value).unwrap();
            let mut expected_wire = BitWriter::new();
            expected_wire.put(index.into(), 7).put_span(empty.span());
            assert_eq!(encoded, expected_wire);
        }
        for count in [MAX_CHALLENGE_SYNC_ENTRIES, MAX_CHALLENGE_SYNC_ENTRIES + 1] {
            let values = [
                Message::ChallengePlayerSync(ChallengePlayerSync {
                    persona: 1,
                    monthly_start: 2,
                    day_id: 3,
                    challenge_ids: vec![4; count],
                }),
                Message::ChallengeFull(ChallengeFull {
                    entries: vec![
                        ChallengeFullEntry {
                            words: [1; 7],
                            flag: true
                        };
                        count
                    ],
                }),
                Message::ChallengeAwards(ChallengeAwards {
                    ids: vec![5; count],
                }),
            ];
            for value in values {
                let mut wire = BitWriter::new();
                let result = Builder::put_message(&mut wire, &value);
                if count > MAX_CHALLENGE_SYNC_ENTRIES {
                    assert_eq!(result, Err(Error::Bound));
                } else {
                    result.unwrap();
                    let mut r = Reader {
                        span: wire.span().after(7).unwrap(),
                        pos: 0,
                    };
                    assert_eq!(message(&mut r, value.index()).unwrap(), value);
                }
            }
        }
    }
}
