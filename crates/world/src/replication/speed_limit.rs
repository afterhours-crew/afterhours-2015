// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Speed-limit goals (registry index 23). The client creation reader
//! (`0x3f73ce0`) reads the goal's RPC identity and six-bit allocation serial
//! (`0x3f7e910`), then the limit as a 32-bit float; the constructor's default
//! limit is 1000. The goal has no update reader.
use super::{Error, Reader, Writer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initial {
    pub selector: u16,
    pub serial: u16,
    /// Shared goal allocation counter, wrapping at 64.
    pub allocation_serial: u8,
    /// The limit as f32 bits (metres per second; E997 22.35), finite on the wire.
    pub limit: u32,
}
impl Initial {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let initial = Self {
            selector: r.take(9)? as u16,
            serial: r.take(10)? as u16,
            allocation_serial: r.take(6)? as u8,
            limit: r.take(32)?,
        };
        if !f32::from_bits(initial.limit).is_finite() {
            return Err(Error::Shape);
        }
        Ok(initial)
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.allocation_serial >= 64 {
            return Err(Error::Bound);
        }
        if !f32::from_bits(self.limit).is_finite() {
            return Err(Error::Shape);
        }
        w.put(u64::from(self.selector), 9)?;
        w.put(u64::from(self.serial), 10)?;
        w.put(u64::from(self.allocation_serial), 6)?;
        w.put(u64::from(self.limit), 32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::replication::{Initial as Creation, Kind, Record, Update};

    fn sample() -> Record {
        Record {
            id: 41,
            initial: Some(Creation::SpeedLimitGoal(Initial {
                selector: 0,
                serial: 1,
                allocation_serial: 6,
                limit: 22.35f32.to_bits(),
            })),
            update: Update::SpeedLimitGoal,
        }
    }

    #[test]
    fn native_layout_has_exact_boundary_and_rejects_every_truncation() {
        // Independently packed: reference 41/13, creation 1/1, type 23/5, RPC
        // selector 0/9 and serial 1/10, allocation 6/6, limit 22.35 as f32 bits.
        let mut wire = BitWriter::new();
        wire.put(41, 13)
            .put(1, 1)
            .put(23, 5)
            .put(0, 9)
            .put(1, 10)
            .put(6, 6);
        wire.put(u64::from(22.35f32.to_bits()), 32);
        let input = wire.span();
        let decoded = Record::decode(input, None).unwrap();
        assert_eq!(decoded.record, sample());
        assert_eq!((decoded.bits, decoded.initial_bits), (76, Some(76)));
        assert_eq!(decoded.record.encode().unwrap().span(), input);
        for bits in 0..76 {
            assert_eq!(
                Record::decode(input.slice(0, bits).unwrap(), None),
                Err(Error::Truncated)
            );
        }
    }

    #[test]
    fn updates_are_empty_and_invalid_values_are_refused() {
        let update = Record {
            id: 41,
            initial: None,
            update: Update::SpeedLimitGoal,
        };
        let wire = update.encode().unwrap();
        assert_eq!(wire.len(), 14);
        assert_eq!(
            Record::decode(wire.span(), Some(Kind::SpeedLimitGoal))
                .unwrap()
                .record,
            update
        );
        let changed = |change: &dyn Fn(&mut Initial)| {
            let mut record = sample();
            if let Some(Creation::SpeedLimitGoal(goal)) = record.initial.as_mut() {
                change(goal);
            }
            record.encode().err()
        };
        assert_eq!(
            changed(&|g| g.limit = f32::INFINITY.to_bits()),
            Some(Error::Shape)
        );
        assert_eq!(changed(&|g| g.allocation_serial = 64), Some(Error::Bound));
    }
}
