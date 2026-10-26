// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Client-to-server information. The slot count belongs to connection context,
//! not the wire. Preserve vector codes without guessing their origin or use.
use super::{Decoded, Error, Initial, Kind, Reader, Record, Writer};
use nfs_protocol::world::BitSpan;

pub const MAX_SLOTS: usize = 64;
const WIDTHS: [u8; 7] = [1, 3, 5, 7, 9, 11, 14];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Vector {
    Raw([u32; 3]),
    /// Original two's-complement bit patterns, before origin/scaling.
    Packed {
        tag: u8,
        components: [u32; 3],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Disabled,
    Enabled(Vec<Option<Vector>>),
}

impl Update {
    fn read(r: &mut Reader<'_>, slots: usize) -> Result<Self, Error> {
        if slots == 0 || slots > MAX_SLOTS {
            return Err(Error::Bound);
        }
        if !r.bit()? {
            return Ok(Self::Disabled);
        }
        let mut values = Vec::with_capacity(slots);
        for _ in 0..slots {
            values.push(r.optional(|r| {
                let tag = r.take(3)? as u8;
                let width = if tag == 0 {
                    32
                } else {
                    WIDTHS[usize::from(tag - 1)]
                };
                let components = [r.take(width)?, r.take(width)?, r.take(width)?];
                Ok(if tag == 0 {
                    Vector::Raw(components)
                } else {
                    Vector::Packed { tag, components }
                })
            })?);
        }
        Ok(Self::Enabled(values))
    }

    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        let Self::Enabled(values) = self else {
            return w.bit(false);
        };
        if values.is_empty() || values.len() > MAX_SLOTS {
            return Err(Error::Bound);
        }
        w.bit(true)?;
        for value in values {
            w.optional(value, |w, vector| {
                let (tag, width, components) = match vector {
                    Vector::Raw(words) => (0, 32, words),
                    Vector::Packed { tag, components } if (1..=7).contains(tag) => {
                        (*tag, WIDTHS[usize::from(*tag - 1)], components)
                    }
                    _ => return Err(Error::Shape),
                };
                w.put(u64::from(tag), 3)?;
                for &word in components {
                    w.put(u64::from(word), usize::from(width))?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

impl Record {
    /// Direction-specific decode. Context must supply its real slot count;
    /// no auto-detection from trailing bits or assumption about multiplayer.
    pub fn decode_client_info(
        input: BitSpan<'_>,
        bound: Option<Kind>,
        slots: usize,
    ) -> Result<Decoded, Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        let id = r.reference()?;
        let create = r.bit()?;
        let kind = if create {
            Kind::from_index(r.take(5)? as u8)?
        } else {
            bound.ok_or(Error::UnknownObject)?
        };
        if kind != Kind::ClientToServerInfo {
            return Err(Error::Unsupported);
        }
        let initial_bits = create.then_some(r.pos);
        let update = Update::read(&mut r, slots)?;
        Ok(Decoded {
            record: Record {
                id,
                initial: create.then_some(Initial::ClientToServerInfo),
                update: super::Update::ClientToServerInfo(update),
            },
            initial_bits,
            bits: r.pos,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::BitWriter,
        replication::section::{Bindings, Setup},
    };

    fn wire(tag: u8) -> BitWriter {
        let mut w = BitWriter::new();
        // Section count=1, no receive-side packed setup; class 4 has no init.
        w.put(0, 1)
            .put(0, 1)
            .put(1, 13)
            .put(0, 1)
            .put(2, 13)
            .put(1, 1)
            .put(4, 5)
            .put(1, 1)
            .put(1, 1)
            .put(tag.into(), 3);
        let width = if tag == 0 {
            32
        } else {
            WIDTHS[usize::from(tag - 1)]
        };
        let width = usize::from(width);
        w.put(1, width).put(0, width).put(1, width);
        w
    }

    #[test]
    fn direction_context_and_each_vector_width_preserve_exact_boundaries() {
        for tag in 0..=7 {
            let w = wire(tag);
            let mut bindings = Bindings::from_client(1).unwrap();
            let decoded = bindings.decode(w.span()).unwrap();
            assert_eq!(decoded.bits, w.len());
            assert_eq!(decoded.section.setup, Some(Setup::ContextOnly));
            assert_eq!(decoded.section.encode().unwrap(), w);
            assert_eq!(bindings.get(2), Some(Kind::ClientToServerInfo));
            // A failed duplicate or truncated later record never mutates state.
            let before = bindings.clone();
            assert_eq!(bindings.decode(w.span()), Err(Error::DuplicateObject));
            assert_eq!(bindings, before);
            assert!(Bindings::default().decode(w.span()).is_err());
            for len in 16..w.len() {
                let mut empty = Bindings::from_client(1).unwrap();
                assert!(
                    empty
                        .decode(BitSpan::new(w.bytes(), 0, len).unwrap())
                        .is_err()
                );
                assert!(empty.is_empty());
            }
        }
    }

    #[test]
    fn context_count_is_required_and_updates_need_the_original_binding() {
        assert!(Bindings::from_client(0).is_err());
        assert!(Bindings::from_client(MAX_SLOTS + 1).is_err());
        let mut w = BitWriter::new();
        w.put(2, 13).put(0, 1).put(1, 1).put(0, 1).put(0, 1);
        assert_eq!(
            Record::decode_client_info(w.span(), None, 2),
            Err(Error::UnknownObject)
        );
        let decoded =
            Record::decode_client_info(w.span(), Some(Kind::ClientToServerInfo), 2).unwrap();
        assert_eq!(decoded.bits, w.len());
        assert_eq!(
            decoded.record.update,
            super::super::Update::ClientToServerInfo(Update::Enabled(vec![None, None]))
        );
        assert_eq!(decoded.record.encode().unwrap(), w);
        assert!(Record::decode_client_info(w.span(), Some(Kind::ClientToServerInfo), 3).is_err());
        let mut disabled = BitWriter::new();
        disabled.put(2, 13).put(0, 1).put(0, 1);
        let decoded =
            Record::decode_client_info(disabled.span(), Some(Kind::ClientToServerInfo), 2).unwrap();
        assert_eq!(
            decoded.record.update,
            super::super::Update::ClientToServerInfo(Update::Disabled)
        );
        assert_eq!(decoded.bits, disabled.len());
    }
}
