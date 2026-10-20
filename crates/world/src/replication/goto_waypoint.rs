// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Go-to-waypoint goals (registry index 9). The client creation reader
//! (`0x3f7eae0`) reads the goal's RPC identity, the six-bit allocation serial,
//! two 8-bit values, an 8-bit count of waypoints of three 32-bit floats each,
//! two Booleans, the reach radius as a 32-bit float and a final Boolean. Its
//! update reader (`0x3f7cf60`) reads nothing when that final Boolean is set;
//! otherwise it reads a presence bit and two 8-bit values. Only goals with the
//! final Boolean set are modeled, so updates never carry a body.
use super::{Error, Reader, Writer};

/// Waypoints one goal may carry (the native count is eight bits).
pub const MAX_WAYPOINTS: usize = 255;

/// Float values are f32 bit patterns, finite on the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initial {
    pub selector: u16,
    pub serial: u16,
    /// Shared goal allocation counter, wrapping at 64.
    pub allocation_serial: u8,
    /// The first 8-bit value; progress updates rewrite it (unnamed).
    pub waypoint: u8,
    /// The second 8-bit value (unnamed).
    pub word: u8,
    pub points: Vec<[u32; 3]>,
    /// The two Booleans before the radius (unnamed).
    pub flags: [bool; 2],
    pub radius: u32,
}
fn finite(bits: u32) -> bool {
    f32::from_bits(bits).is_finite()
}
impl Initial {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let selector = r.take(9)? as u16;
        let serial = r.take(10)? as u16;
        let allocation_serial = r.take(6)? as u8;
        let waypoint = r.take(8)? as u8;
        let word = r.take(8)? as u8;
        let count = r.take(8)? as usize;
        if count > (r.span.len() - r.pos) / 96 {
            return Err(Error::Truncated);
        }
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            let point = [r.take(32)?, r.take(32)?, r.take(32)?];
            if !point.into_iter().all(finite) {
                return Err(Error::Shape);
            }
            points.push(point);
        }
        let flags = [r.bit()?, r.bit()?];
        let radius = r.take(32)?;
        if !finite(radius) {
            return Err(Error::Shape);
        }
        // Goals with progress updates are not modeled: their update layout
        // depends on this Boolean, which a bound update cannot see.
        if !r.bit()? {
            return Err(Error::Unsupported);
        }
        Ok(Self {
            selector,
            serial,
            allocation_serial,
            waypoint,
            word,
            points,
            flags,
            radius,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.allocation_serial >= 64 || self.points.len() > MAX_WAYPOINTS {
            return Err(Error::Bound);
        }
        if !self.points.iter().flatten().copied().all(finite) || !finite(self.radius) {
            return Err(Error::Shape);
        }
        w.put(u64::from(self.selector), 9)?;
        w.put(u64::from(self.serial), 10)?;
        w.put(u64::from(self.allocation_serial), 6)?;
        w.put(u64::from(self.waypoint), 8)?;
        w.put(u64::from(self.word), 8)?;
        w.put(self.points.len() as u64, 8)?;
        for point in &self.points {
            for v in point {
                w.put(u64::from(*v), 32)?;
            }
        }
        w.bit(self.flags[0])?;
        w.bit(self.flags[1])?;
        w.put(u64::from(self.radius), 32)?;
        w.bit(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::replication::{Initial as Creation, Kind, Record, Update};

    fn sample() -> Record {
        Record {
            id: 37,
            initial: Some(Creation::GotoWaypointGoal(Initial {
                selector: 0,
                serial: 1,
                allocation_serial: 5,
                waypoint: 0,
                word: 1,
                points: vec![[1.5f32.to_bits(), (-2f32).to_bits(), 300.25f32.to_bits()]],
                flags: [false, true],
                radius: 150f32.to_bits(),
            })),
            update: Update::GotoWaypointGoal,
        }
    }

    /// Independently packed in the native reader's order.
    fn packed(last: bool) -> BitWriter {
        let mut w = BitWriter::new();
        w.put(37, 13).put(1, 1).put(9, 5);
        w.put(0, 9)
            .put(1, 10)
            .put(5, 6)
            .put(0, 8)
            .put(1, 8)
            .put(1, 8);
        for v in [1.5f32, -2.0, 300.25] {
            w.put(u64::from(v.to_bits()), 32);
        }
        w.put(0, 1).put(1, 1).put(u64::from(150f32.to_bits()), 32);
        w.put(u64::from(last), 1);
        w
    }

    #[test]
    fn native_layout_has_exact_boundary_and_rejects_every_truncation() {
        let wire = packed(true);
        let input = wire.span();
        assert_eq!(input.len(), 13 + 1 + 5 + 9 + 10 + 6 + 24 + 96 + 2 + 32 + 1);
        let decoded = Record::decode(input, None).unwrap();
        assert_eq!(decoded.record, sample());
        assert_eq!(decoded.bits, input.len());
        assert_eq!(decoded.initial_bits, Some(input.len()));
        assert_eq!(decoded.record.encode().unwrap().span(), input);
        for bits in 0..input.len() {
            assert_eq!(
                Record::decode(input.slice(0, bits).unwrap(), None),
                Err(Error::Truncated)
            );
        }
    }

    #[test]
    fn progress_goals_and_invalid_values_are_refused() {
        // A clear final Boolean selects the progress update layout: unsupported.
        assert_eq!(
            Record::decode(packed(false).span(), None),
            Err(Error::Unsupported)
        );
        let changed = |change: &dyn Fn(&mut Initial)| {
            let mut record = sample();
            if let Some(Creation::GotoWaypointGoal(goal)) = record.initial.as_mut() {
                change(goal);
            }
            record.encode().err()
        };
        assert_eq!(
            changed(&|g| g.radius = f32::NAN.to_bits()),
            Some(Error::Shape)
        );
        assert_eq!(
            changed(&|g| g.points[0][1] = f32::INFINITY.to_bits()),
            Some(Error::Shape)
        );
        assert_eq!(
            changed(&|g| g.points = vec![[0; 3]; MAX_WAYPOINTS + 1]),
            Some(Error::Bound)
        );
        assert_eq!(changed(&|g| g.allocation_serial = 64), Some(Error::Bound));
    }

    #[test]
    fn bound_updates_are_empty_and_leave_the_next_record() {
        let update = Record {
            id: 37,
            initial: None,
            update: Update::GotoWaypointGoal,
        };
        let mut joined = update.encode().unwrap();
        assert_eq!(joined.len(), 14);
        joined.put_span(sample().encode().unwrap().span());
        let first = Record::decode(joined.span(), Some(Kind::GotoWaypointGoal)).unwrap();
        assert_eq!((first.record, first.bits), (update, 14));
        let rest = joined.span().slice(14, joined.len() - 14).unwrap();
        assert_eq!(Record::decode(rest, None).unwrap().record, sample());
    }
}
