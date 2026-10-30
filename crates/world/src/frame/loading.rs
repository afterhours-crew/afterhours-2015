// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{BitSpan, BitWriter, Error, Reader};

pub const EX_LEVEL_LOADED_ACK: u8 = 51;
/// Existing local bound for loaded/unloaded sublevel lists.
pub const MAX_SUBLEVEL_REPORTS: usize = super::MAX_MESSAGES * 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubLevelReport {
    /// Preserve the native word; it is not an inferred completion percentage.
    pub word: u32,
    pub levels: Vec<u16>,
}

impl SubLevelReport {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let word = r.take(32)?;
        let count = r.take(32)? as usize;
        if count > MAX_SUBLEVEL_REPORTS {
            return Err(Error::Bound);
        }
        if r.rest()?.len() < count * 16 {
            return Err(Error::Truncated);
        }
        let levels = (0..count)
            .map(|_| r.take(16).map(|v| v as u16))
            .collect::<Result<_, _>>()?;
        Ok(Self { word, levels })
    }

    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let mut r = Reader { span: body, pos: 0 };
        let value = Self::read(&mut r)?;
        if r.pos != body.len() {
            return Err(Error::Shape);
        }
        Ok(value)
    }

    pub fn encode(&self) -> Result<BitWriter, Error> {
        if self.levels.len() > MAX_SUBLEVEL_REPORTS {
            return Err(Error::Bound);
        }
        let mut w = BitWriter::new();
        w.put(u64::from(self.word), 32)
            .put(self.levels.len() as u64, 32);
        for &level in &self.levels {
            w.put(u64::from(level), 16);
        }
        Ok(w)
    }
}

/// Level-loaded acknowledgement. The native reader/writer serialize the 16-byte
/// identity as a u32, two u16 values and eight bytes. Its meaning is not inferred
/// from an arbitrary digest algorithm; preserve the native in-memory byte order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelLoadedAck {
    pub level: Vec<u8>,
    pub identity: [u8; 16],
}

impl LevelLoadedAck {
    /// Encode the body for canonical MessageEX chunking, without its index.
    pub fn encode(&self) -> Result<BitWriter, Error> {
        let mut out = BitWriter::new();
        self.write(&mut out)?;
        Ok(out)
    }

    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let level = super::string(r)?;
        let mut identity = [0; 16];
        identity[..4].copy_from_slice(&r.take(32)?.to_le_bytes());
        identity[4..6].copy_from_slice(&(r.take(16)? as u16).to_le_bytes());
        identity[6..8].copy_from_slice(&(r.take(16)? as u16).to_le_bytes());
        for byte in &mut identity[8..] {
            *byte = r.take(8)? as u8;
        }
        Ok(Self { level, identity })
    }

    pub(super) fn write(&self, w: &mut BitWriter) -> Result<(), Error> {
        if self.level.len() > super::MAX_STRING_BYTES {
            return Err(Error::Bound);
        }
        w.put(self.level.len() as u64, 10);
        for &byte in &self.level {
            w.put(u64::from(byte), 8);
        }
        w.put(
            u64::from(u32::from_le_bytes(
                self.identity[..4].try_into().expect("fixed identity"),
            )),
            32,
        )
        .put(
            u64::from(u16::from_le_bytes(
                self.identity[4..6].try_into().expect("fixed identity"),
            )),
            16,
        )
        .put(
            u64::from(u16::from_le_bytes(
                self.identity[6..8].try_into().expect("fixed identity"),
            )),
            16,
        );
        for &byte in &self.identity[8..] {
            w.put(u64::from(byte), 8);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{self, Message};

    #[test]
    fn loaded_identity_has_native_field_order_and_exact_extent() {
        let value = LevelLoadedAck {
            level: b"test/level".to_vec(),
            identity: [
                0x78, 0x56, 0x34, 0x12, 0xbc, 0x9a, 0xf0, 0xde, 1, 2, 3, 4, 5, 6, 7, 8,
            ],
        };
        let mut expected = BitWriter::new();
        expected.put(10, 10);
        for byte in b"test/level" {
            expected.put(u64::from(*byte), 8);
        }
        expected.put(0x12345678, 32).put(0x9abc, 16).put(0xdef0, 16);
        for byte in 1..=8 {
            expected.put(byte, 8);
        }
        let encoded = value.encode().unwrap();
        assert_eq!(encoded, expected);
        assert_eq!(
            frame::assembled_message(EX_LEVEL_LOADED_ACK, expected.bytes()).unwrap(),
            Message::LevelLoadedAck(value)
        );
        for length in 0..expected.len() {
            assert!(
                LevelLoadedAck::read(&mut Reader {
                    span: expected.span().slice(0, length).unwrap(),
                    pos: 0
                })
                .is_err()
            );
        }
        let mut extra = expected.bytes().to_vec();
        extra.push(0);
        assert!(frame::assembled_message(EX_LEVEL_LOADED_ACK, &extra).is_err());
        let mut oversized = LevelLoadedAck {
            level: vec![0; 1024],
            identity: [0; 16],
        };
        assert_eq!(oversized.write(&mut BitWriter::new()), Err(Error::Bound));
        oversized.level.clear();
        assert!(oversized.write(&mut BitWriter::new()).is_ok());
    }

    #[test]
    fn sublevel_list_checks_counts_truncations_and_trailing_bits() {
        let bytes = [0x12, 0x34, 0x56, 0x78, 0, 0, 0, 2, 0x01, 0xff, 0x80, 0x01];
        let value = SubLevelReport {
            word: 0x12345678,
            levels: vec![511, 32769],
        };
        assert_eq!(value.encode().unwrap().bytes(), &bytes);
        let span = BitSpan::new(&bytes, 0, 96).unwrap();
        assert_eq!(SubLevelReport::decode(span), Ok(value));
        for len in 0..96 {
            assert!(SubLevelReport::decode(span.slice(0, len).unwrap()).is_err());
        }
        let mut extra = BitWriter::new();
        extra.put_span(span).put(0, 1);
        assert_eq!(SubLevelReport::decode(extra.span()), Err(Error::Shape));
        let mut count = BitWriter::new();
        count.put(0, 32).put((MAX_SUBLEVEL_REPORTS + 1) as u64, 32);
        assert_eq!(SubLevelReport::decode(count.span()), Err(Error::Bound));
        assert_eq!(
            SubLevelReport {
                word: 0,
                levels: vec![1; MAX_SUBLEVEL_REPORTS + 1]
            }
            .encode(),
            Err(Error::Bound)
        );
    }
}
