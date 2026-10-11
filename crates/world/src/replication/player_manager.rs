// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! PlayerManagerGhost's grouped player/controllable associations.
use super::{Error, Reader, Writer};
/// Supported protocol profile: seventy entries in groups of eight.
pub const SLOTS: usize = 70;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Seat {
    pub controllable: u16,
    pub index: Option<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub player: u16,
    pub controllable: u16,
    /// True omits the first seated controllable.
    pub detached: bool,
    pub primary: Option<Seat>,
    pub secondary: Option<Seat>,
}
impl Entry {
    pub fn unbound(player: u16) -> Self {
        Self {
            player,
            controllable: 0,
            detached: true,
            primary: None,
            secondary: None,
        }
    }
    pub fn driving(player: u16, vehicle: u16) -> Self {
        let seat = Seat {
            controllable: vehicle,
            index: Some(0),
        };
        Self {
            player,
            controllable: 0,
            detached: false,
            primary: Some(seat.clone()),
            secondary: Some(seat),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Update {
    pub component_present: bool,
    /// None is unchanged; Some(None) clears the slot.
    pub entries: Vec<Option<Option<Entry>>>,
}
impl Default for Update {
    fn default() -> Self {
        Self {
            component_present: false,
            entries: vec![None; SLOTS],
        }
    }
}
impl Update {
    pub fn empty_snapshot() -> Self {
        Self {
            component_present: true,
            entries: vec![Some(None); SLOTS],
        }
    }
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let mut out = Self {
            component_present: r.bit()?,
            ..Self::default()
        };
        for group in out.entries.chunks_mut(8) {
            if !r.bit()? {
                continue;
            }
            for slot in group {
                if !r.bit()? {
                    continue;
                }
                *slot = Some(if !r.bit()? {
                    None
                } else {
                    let player = r.reference()?;
                    let controllable = r.reference()?;
                    let detached = r.bit()?;
                    let primary = if detached { None } else { Some(read_seat(r)?) };
                    let secondary = r.optional(read_seat)?;
                    Some(Entry {
                        player,
                        controllable,
                        detached,
                        primary,
                        secondary,
                    })
                });
            }
        }
        Ok(out)
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.entries.len() != SLOTS {
            return Err(Error::Shape);
        }
        w.bit(self.component_present)?;
        for group in self.entries.chunks(8) {
            let present = group.iter().any(Option::is_some);
            w.bit(present)?;
            if !present {
                continue;
            }
            for slot in group {
                w.bit(slot.is_some())?;
                let Some(entry) = slot else {
                    continue;
                };
                w.bit(entry.is_some())?;
                let Some(e) = entry else {
                    continue;
                };
                if e.detached != e.primary.is_none() {
                    return Err(Error::Shape);
                }
                w.reference(&e.player)?;
                w.reference(&e.controllable)?;
                w.bit(e.detached)?;
                if let Some(seat) = &e.primary {
                    write_seat(w, seat)?;
                }
                w.optional(&e.secondary, write_seat)?;
            }
        }
        Ok(())
    }
    pub(super) fn merge(&mut self, other: &Self) -> Result<(), Error> {
        if self.entries.len() != SLOTS || other.entries.len() != SLOTS {
            return Err(Error::Shape);
        }
        self.component_present |= other.component_present;
        for (a, b) in self.entries.iter_mut().zip(&other.entries) {
            if b.is_some() {
                *a = b.clone();
            }
        }
        Ok(())
    }
}
fn read_seat(r: &mut Reader<'_>) -> Result<Seat, Error> {
    Ok(Seat {
        controllable: r.reference()?,
        index: r.optional(|r| Ok(r.take(4)? as u8))?,
    })
}
fn write_seat(w: &mut Writer, s: &Seat) -> Result<(), Error> {
    w.reference(&s.controllable)?;
    w.optional(&s.index, |w, v| w.put(u64::from(*v), 4))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::BitWriter,
        replication::{Initial, Kind, Record, Update as Delta},
    };
    use nfs_protocol::world::BitSpan;
    #[test]
    fn grouped_delta_roundtrip_truncation_and_concatenation() {
        let mut update = Update::default();
        update.entries[3] = Some(Some(Entry::driving(12, 150)));
        update.entries[69] = Some(None);
        let record = Record {
            id: 17,
            initial: None,
            update: Delta::PlayerManager(update),
        };
        let encoded = record.encode().unwrap();
        let decoded = Record::decode(encoded.span(), Some(Kind::PlayerManager)).unwrap();
        assert_eq!(decoded.record, record);
        assert_eq!(decoded.bits, encoded.len());
        for n in 0..encoded.len() {
            assert!(
                Record::decode(
                    BitSpan::new(encoded.bytes(), 0, n).unwrap(),
                    Some(Kind::PlayerManager)
                )
                .is_err()
            );
        }
        let mut both = BitWriter::new();
        both.put_span(encoded.span()).put_span(encoded.span());
        let first = Record::decode(both.span(), Some(Kind::PlayerManager)).unwrap();
        let second = Record::decode(
            both.span().after(first.bits).unwrap(),
            Some(Kind::PlayerManager),
        )
        .unwrap();
        assert_eq!(second.record, record);
    }
    #[test]
    fn snapshot_clear_and_invalid_shape() {
        let mut update = Update::empty_snapshot();
        update.entries[0] = Some(Some(Entry::unbound(2)));
        let make = |u| Record {
            id: 1,
            initial: Some(Initial::PlayerManager),
            update: Delta::PlayerManager(u),
        };
        let encoded = make(update.clone()).encode().unwrap();
        assert_eq!(
            Record::decode(encoded.span(), None).unwrap().record,
            make(update.clone())
        );
        let mut delta = Update::default();
        delta.entries[0] = Some(None);
        update.merge(&delta).unwrap();
        assert_eq!(update.entries[0], Some(None));
        update.entries.pop();
        assert!(make(update).encode().is_err());
        let mut bad = Update::default();
        let mut entry = Entry::driving(2, 3);
        entry.primary.as_mut().unwrap().index = Some(16);
        bad.entries[0] = Some(Some(entry));
        assert!(make(bad.clone()).encode().is_err());
        bad.entries[0].as_mut().unwrap().as_mut().unwrap().primary = None;
        assert!(make(bad).encode().is_err());
    }
}
