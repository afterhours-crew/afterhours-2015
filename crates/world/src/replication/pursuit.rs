// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Pursuit information (registry index 17). Creation has no payload. The
//! native update reader (RVA 0x426d950, serializer vtable 0x21bd5e0) reads 12
//! presence-gated groups; the paired sender writer 0x462e870 writes the same
//! groups. Counts are six and four bits wide (obfuscated constants evaluated
//! explicitly). Vectors are three raw 32-bit words (0x3412860), floats raw
//! 32-bit words. Values are retained exactly; police semantics are unverified.
use super::{Error, Reader, Writer};

/// Six-bit count width.
pub const MAX_PAIRS: usize = 63;
/// Four-bit count width.
pub const MAX_ENTRIES: usize = 15;

pub type Vector = [u32; 3];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pair {
    pub reference: u16,
    pub value: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tagged {
    pub reference: u16,
    pub value: u8,
    pub flag: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    pub reference: u16,
    pub vector: Vector,
    pub flag: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Update {
    pub reference: Option<u16>,
    pub pairs: Option<Vec<Pair>>,
    pub references: Option<[u16; 7]>,
    pub vectors: Option<[Vector; 7]>,
    pub tagged: Option<Tagged>,
    pub entries: Option<Vec<Entry>>,
    pub four: Option<[u32; 4]>,
    pub single: Option<u32>,
    pub value4: Option<u8>,
    pub value3: Option<u8>,
    pub two: Option<[u32; 2]>,
    pub flag: Option<bool>,
}

fn vector(r: &mut Reader<'_>) -> Result<Vector, Error> {
    Ok([r.take(32)?, r.take(32)?, r.take(32)?])
}
fn put_vector(w: &mut Writer, v: &Vector) -> Result<(), Error> {
    v.iter().try_for_each(|c| w.put(u64::from(*c), 32))
}
fn words<const N: usize>(r: &mut Reader<'_>) -> Result<[u32; N], Error> {
    let mut out = [0; N];
    for word in &mut out {
        *word = r.take(32)?;
    }
    Ok(out)
}

impl Update {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            reference: r.optional(Reader::reference)?,
            pairs: r.optional(|r| {
                let count = r.take(6)? as usize;
                if count > r.span.len().saturating_sub(r.pos) / 15 {
                    return Err(Error::Truncated);
                }
                (0..count)
                    .map(|_| {
                        Ok(Pair {
                            reference: r.reference()?,
                            value: r.take(2)? as u8,
                        })
                    })
                    .collect()
            })?,
            references: r.optional(|r| {
                let mut out = [0; 7];
                for reference in &mut out {
                    *reference = r.reference()?;
                }
                Ok(out)
            })?,
            vectors: r.optional(|r| {
                let mut out = [[0; 3]; 7];
                for v in &mut out {
                    *v = vector(r)?;
                }
                Ok(out)
            })?,
            tagged: r.optional(|r| {
                Ok(Tagged {
                    reference: r.reference()?,
                    value: r.take(3)? as u8,
                    flag: r.bit()?,
                })
            })?,
            entries: r.optional(|r| {
                let count = r.take(4)? as usize;
                if count > r.span.len().saturating_sub(r.pos) / 110 {
                    return Err(Error::Truncated);
                }
                (0..count)
                    .map(|_| {
                        Ok(Entry {
                            reference: r.reference()?,
                            vector: vector(r)?,
                            flag: r.bit()?,
                        })
                    })
                    .collect()
            })?,
            four: r.optional(words::<4>)?,
            single: r.optional(|r| r.take(32))?,
            value4: r.optional(|r| Ok(r.take(4)? as u8))?,
            value3: r.optional(|r| Ok(r.take(3)? as u8))?,
            two: r.optional(words::<2>)?,
            flag: r.optional(Reader::bit)?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.reference, Writer::reference)?;
        w.optional(&self.pairs, |w, pairs| {
            if pairs.len() > MAX_PAIRS {
                return Err(Error::Bound);
            }
            w.put(pairs.len() as u64, 6)?;
            pairs.iter().try_for_each(|p| {
                w.reference(&p.reference)?;
                w.put(u64::from(p.value), 2)
            })
        })?;
        w.optional(&self.references, |w, refs| {
            refs.iter().try_for_each(|r| w.reference(r))
        })?;
        w.optional(&self.vectors, |w, vs| {
            vs.iter().try_for_each(|v| put_vector(w, v))
        })?;
        w.optional(&self.tagged, |w, t| {
            w.reference(&t.reference)?;
            w.put(u64::from(t.value), 3)?;
            w.bit(t.flag)
        })?;
        w.optional(&self.entries, |w, entries| {
            if entries.len() > MAX_ENTRIES {
                return Err(Error::Bound);
            }
            w.put(entries.len() as u64, 4)?;
            entries.iter().try_for_each(|e| {
                w.reference(&e.reference)?;
                put_vector(w, &e.vector)?;
                w.bit(e.flag)
            })
        })?;
        w.optional(&self.four, |w, v| {
            v.iter().try_for_each(|c| w.put(u64::from(*c), 32))
        })?;
        w.optional(&self.single, |w, v| w.put(u64::from(*v), 32))?;
        w.optional(&self.value4, |w, v| w.put(u64::from(*v), 4))?;
        w.optional(&self.value3, |w, v| w.put(u64::from(*v), 3))?;
        w.optional(&self.two, |w, v| {
            v.iter().try_for_each(|c| w.put(u64::from(*c), 32))
        })?;
        w.optional(&self.flag, |w, v| w.bit(*v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::BitWriter,
        replication::{Initial, Kind, Record, Update as RecordUpdate},
    };
    use nfs_protocol::world::BitSpan;

    fn full() -> Update {
        Update {
            reference: Some(260),
            pairs: Some(vec![Pair {
                reference: 17,
                value: 3,
            }]),
            references: Some([1, 2, 3, 4, 5, 6, 8191]),
            vectors: Some([[1, 2, 3]; 7]),
            tagged: Some(Tagged {
                reference: 9,
                value: 7,
                flag: true,
            }),
            entries: Some(vec![]),
            four: Some([0.5f32.to_bits(), 0, u32::MAX, 7]),
            single: Some(1.0f32.to_bits()),
            value4: Some(15),
            value3: Some(5),
            two: Some([3, 4]),
            flag: Some(false),
        }
    }

    #[test]
    fn creation_has_no_payload_and_every_group_round_trips() {
        let record = Record {
            id: 65,
            initial: Some(Initial::PursuitInfo),
            update: RecordUpdate::PursuitInfo(full()),
        };
        let encoded = record.encode().unwrap();
        // header 19, presence 12, values:
        // 13 + (6+15) + 91 + 672 + 17 + 4 + 128 + 32 + 4 + 3 + 64 + 1.
        assert_eq!(encoded.len(), 19 + 12 + 1050);
        let bytes = encoded.bytes();
        let span = BitSpan::new(bytes, 0, encoded.len()).unwrap();
        let decoded = Record::decode(span, None).unwrap();
        assert_eq!(decoded.record, record);
        assert_eq!(decoded.initial_bits, Some(19));
    }

    #[test]
    fn absent_groups_are_single_bits_and_counts_are_bounded() {
        let mut w = BitWriter::new();
        w.put(65, 13).put(0, 1).put(0, 12);
        let bytes = w.bytes();
        let span = BitSpan::new(bytes, 0, w.len()).unwrap();
        let decoded = Record::decode(span, Some(Kind::PursuitInfo)).unwrap();
        assert_eq!(
            decoded.record.update,
            RecordUpdate::PursuitInfo(Update::default())
        );
        assert_eq!(decoded.bits, 26);

        // A declared count larger than the remaining input is truncation.
        let mut w = BitWriter::new();
        w.put(65, 13).put(0, 1).put(0, 1).put(1, 1).put(63, 6);
        let bytes = w.bytes();
        let span = BitSpan::new(bytes, 0, w.len()).unwrap();
        assert_eq!(
            Record::decode(span, Some(Kind::PursuitInfo)).unwrap_err(),
            Error::Truncated
        );

        let mut update = full();
        update.entries = Some(vec![
            Entry {
                reference: 1,
                vector: [0; 3],
                flag: false,
            };
            MAX_ENTRIES + 1
        ]);
        let record = Record {
            id: 65,
            initial: None,
            update: RecordUpdate::PursuitInfo(update),
        };
        assert_eq!(record.encode().unwrap_err(), Error::Bound);
    }
}
