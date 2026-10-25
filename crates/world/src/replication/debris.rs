// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Debris manager (registry index 5), a client singleton built by factory RVA
//! 0x37d5130 with its Ghost at +0x1c8 (serializer vtable 0x1fb7458). The
//! creation reader (0x36e61d0) reads two 32-bit words; the update reader
//! (0x36e3b90) reads two presence-gated two-bit values. Values are retained
//! without an unverified gameplay meaning.
use super::{Error, Reader, Writer};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Initial {
    pub words: [u32; 2],
}
impl Initial {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            words: [r.take(32)?, r.take(32)?],
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.put(u64::from(self.words[0]), 32)?;
        w.put(u64::from(self.words[1]), 32)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Update {
    pub values: [Option<u8>; 2],
}
impl Update {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            values: [
                r.optional(|r| Ok(r.take(2)? as u8))?,
                r.optional(|r| Ok(r.take(2)? as u8))?,
            ],
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        for value in &self.values {
            w.optional(value, |w, v| w.put(u64::from(*v), 2))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::BitWriter,
        replication::{Initial as RecordInitial, Kind, Record, Update as RecordUpdate},
    };
    use nfs_protocol::world::BitSpan;

    fn record(values: [Option<u8>; 2]) -> Record {
        Record {
            id: 2,
            initial: Some(RecordInitial::DebrisManager(Initial {
                words: [0, 0xdead_beef],
            })),
            update: RecordUpdate::DebrisManager(Update { values }),
        }
    }

    #[test]
    fn native_layout_round_trips_with_the_following_record_boundary() {
        // Hand-assembled from the native readers: id, create, index 5,
        // two words, then presence + value for each of the two fields.
        let mut w = BitWriter::new();
        w.put(2, 13)
            .put(1, 1)
            .put(5, 5)
            .put(0, 32)
            .put(0xdead_beef, 32);
        w.put(1, 1).put(3, 2).put(0, 1);
        let expected = w.bytes();
        let encoded = record([Some(3), None]).encode().unwrap();
        assert_eq!(encoded.len(), 13 + 1 + 5 + 64 + 3 + 1);
        assert_eq!(encoded.bytes(), expected);
        let span = BitSpan::new(expected, 0, encoded.len()).unwrap();
        let decoded = Record::decode(span, None).unwrap();
        assert_eq!(decoded.record, record([Some(3), None]));
        assert_eq!(decoded.bits, encoded.len());
        assert_eq!(decoded.initial_bits, Some(13 + 1 + 5 + 64));
    }

    #[test]
    fn updates_need_the_bound_kind_and_reject_truncation_and_wide_values() {
        let mut w = BitWriter::new();
        w.put(2, 13)
            .put(0, 1)
            .put(1, 1)
            .put(2, 2)
            .put(1, 1)
            .put(1, 2);
        let bytes = w.bytes();
        let span = BitSpan::new(bytes, 0, w.len()).unwrap();
        assert_eq!(
            Record::decode(span, None).unwrap_err(),
            Error::UnknownObject
        );
        let decoded = Record::decode(span, Some(Kind::DebrisManager)).unwrap();
        assert_eq!(
            decoded.record.update,
            RecordUpdate::DebrisManager(Update {
                values: [Some(2), Some(1)]
            })
        );
        let short = BitSpan::new(bytes, 0, w.len() - 1).unwrap();
        assert_eq!(
            Record::decode(short, Some(Kind::DebrisManager)).unwrap_err(),
            Error::Truncated
        );
        assert_eq!(record([Some(4), None]).encode().unwrap_err(), Error::Shape);
    }
}
