// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Go-to-waypoint goals (registry index 9). The client creation reader
//! (`0x3f7eae0`) reads the goal's RPC identity, the six-bit allocation serial,
//! two 8-bit values, an 8-bit count of waypoints of three 32-bit floats each,
//! two Booleans, the reach radius as a 32-bit float and a final Boolean. Its
//! update reader (`0x3f7cf60`) reads nothing when that final Boolean is set;
//! otherwise it reads a presence bit and two 8-bit values. The final Boolean is
//! retained in the binding: an update alone cannot identify its wire layout.
use super::{Decoded, Error, Kind, Reader, Record, Writer};
use nfs_protocol::world::BitSpan;

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
    /// False when the final creation Boolean is set (intro goals). True for
    /// race route goals, whose updates carry an optional pair of progress bytes.
    pub progress_updates: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Update {
    /// No body, including no presence bit.
    NoProgress,
    /// Presence bit followed, when set, by two eight-bit values. The native
    /// reader stores them at +0x188 and +0x18c; their semantics remain unnamed.
    Progress(Option<[u8; 2]>),
}
impl Update {
    pub(super) fn progress_updates(self) -> bool {
        matches!(self, Self::Progress(_))
    }
    pub(super) fn read(r: &mut Reader<'_>, progress: bool) -> Result<Self, Error> {
        Ok(if progress {
            Self::Progress(if r.bit()? {
                Some([r.take(8)? as u8, r.take(8)? as u8])
            } else {
                None
            })
        } else {
            Self::NoProgress
        })
    }
    pub(super) fn write(self, w: &mut Writer) -> Result<(), Error> {
        if let Self::Progress(pair) = self {
            w.bit(pair.is_some())?;
            if let Some(pair) = pair {
                w.put(u64::from(pair[0]), 8)?;
                w.put(u64::from(pair[1]), 8)?;
            }
        }
        Ok(())
    }
    pub(super) fn merge(&mut self, new: Self) -> Result<(), Error> {
        match (self, new) {
            (Self::NoProgress, Self::NoProgress) => Ok(()),
            (Self::Progress(old), Self::Progress(new)) => {
                if new.is_some() {
                    *old = new;
                }
                Ok(())
            }
            _ => Err(Error::TypeMismatch),
        }
    }
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
        let progress_updates = !r.bit()?;
        Ok(Self {
            selector,
            serial,
            allocation_serial,
            waypoint,
            word,
            points,
            flags,
            radius,
            progress_updates,
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
        w.bit(!self.progress_updates)
    }
}

impl Record {
    /// Decode a waypoint goal using the mode retained from its creation.
    /// Creation records carry their own mode. Bound updates require it explicitly.
    pub fn decode_goto_waypoint(
        input: BitSpan<'_>,
        bound: Option<Kind>,
        progress_updates: Option<bool>,
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
        if kind != Kind::GotoWaypointGoal {
            return Err(Error::TypeMismatch);
        }
        let initial = if create {
            Some(Initial::read(&mut r)?)
        } else {
            None
        };
        let initial_bits = initial.as_ref().map(|_| r.pos);
        let mode = initial
            .as_ref()
            .map(|v| v.progress_updates)
            .or(progress_updates)
            .ok_or(Error::Unsupported)?;
        let update = Update::read(&mut r, mode)?;
        Ok(Decoded {
            record: Self {
                id,
                initial: initial.map(super::Initial::GotoWaypointGoal),
                update: super::Update::GotoWaypointGoal(update),
            },
            initial_bits,
            bits: r.pos,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::replication::{Initial as Creation, Update as RecordUpdate};

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
                progress_updates: false,
            })),
            update: RecordUpdate::GotoWaypointGoal(Update::NoProgress),
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
    fn invalid_values_and_missing_progress_are_refused() {
        // A clear final Boolean requires at least the update presence bit.
        assert_eq!(
            Record::decode(packed(false).span(), None),
            Err(Error::Truncated)
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
            update: RecordUpdate::GotoWaypointGoal(Update::NoProgress),
        };
        let mut joined = update.encode().unwrap();
        assert_eq!(joined.len(), 14);
        joined.put_span(sample().encode().unwrap().span());
        let first =
            Record::decode_goto_waypoint(joined.span(), Some(Kind::GotoWaypointGoal), Some(false))
                .unwrap();
        assert_eq!((first.record, first.bits), (update, 14));
        let rest = joined.span().slice(14, joined.len() - 14).unwrap();
        assert_eq!(Record::decode(rest, None).unwrap().record, sample());
    }

    fn progressing(pair: Option<[u8; 2]>) -> Record {
        let mut record = sample();
        let Some(Creation::GotoWaypointGoal(initial)) = &mut record.initial else {
            unreachable!()
        };
        initial.progress_updates = true;
        record.update = RecordUpdate::GotoWaypointGoal(Update::Progress(pair));
        record
    }

    #[test]
    fn progress_creation_and_bound_updates_follow_the_native_presence_bit() {
        for pair in [None, Some([0, 0]), Some([21, 255])] {
            let mut wire = packed(false);
            wire.put_bool(pair.is_some());
            if let Some(pair) = pair {
                wire.put(u64::from(pair[0]), 8).put(u64::from(pair[1]), 8);
            }
            let decoded = Record::decode(wire.span(), None).unwrap();
            assert_eq!(decoded.record, progressing(pair));
            assert_eq!(decoded.initial_bits, Some(packed(false).len()));
            assert_eq!(decoded.bits, wire.len());
            assert_eq!(decoded.record.encode().unwrap().span(), wire.span());
            for end in 0..wire.len() {
                assert_eq!(
                    Record::decode(wire.span().slice(0, end).unwrap(), None),
                    Err(Error::Truncated)
                );
            }

            let mut expected = progressing(pair);
            expected.initial = None;
            let mut update = BitWriter::new();
            update.put(37, 13).put_bool(false).put_bool(pair.is_some());
            if let Some(pair) = pair {
                update.put(u64::from(pair[0]), 8).put(u64::from(pair[1]), 8);
            }
            let end = update.len();
            update.put_span(sample().encode().unwrap().span());
            let decoded = Record::decode_goto_waypoint(
                update.span(),
                Some(Kind::GotoWaypointGoal),
                Some(true),
            )
            .unwrap();
            assert_eq!(decoded.bits, end);
            assert_eq!(decoded.record, expected);
            let encoded = decoded.record.encode().unwrap();
            for bit in 0..end {
                assert_eq!(
                    encoded.span().read_u32(bit, 1),
                    update.span().read_u32(bit, 1)
                );
            }
            assert_eq!(
                Record::decode(update.span(), Some(Kind::GotoWaypointGoal)),
                Err(Error::Unsupported),
                "a kind alone cannot select the update layout"
            );
            for bits in 0..end {
                assert!(
                    Record::decode_goto_waypoint(
                        update.span().slice(0, bits).unwrap(),
                        Some(Kind::GotoWaypointGoal),
                        Some(true)
                    )
                    .is_err()
                );
            }
            assert_eq!(
                Record::decode(update.span().after(end).unwrap(), None)
                    .unwrap()
                    .record,
                sample()
            );
        }
    }

    fn section(records: Vec<Record>, deleted: Vec<u16>) -> super::super::section::Section {
        super::super::section::Section {
            float_bits: None,
            flag: false,
            setup: (!records.is_empty()).then_some(super::super::section::Setup::Packed {
                tag: None,
                width: None,
                axes: [None; 3],
            }),
            records,
            deleted,
        }
    }

    #[test]
    fn bindings_keep_goal_modes_transactional_and_forget_them_on_removal() {
        use super::super::section::Bindings;
        let mut bindings = Bindings::default();
        let mut fixed = sample();
        fixed.id = 38;
        let initial = section(vec![progressing(Some([0, 0])), fixed.clone()], vec![]);
        let wire = initial.encode().unwrap();
        assert_eq!(bindings.decode(wire.span()).unwrap().section, initial);
        let mut update = progressing(Some([2, 3]));
        update.initial = None;
        fixed.initial = None;
        let changes = section(vec![update.clone(), fixed.clone()], vec![]);
        let wire = changes.encode().unwrap();
        let before = bindings.clone();
        assert!(
            bindings
                .decode(wire.span().slice(0, wire.len() - 1).unwrap())
                .is_err()
        );
        assert_eq!(bindings, before);
        assert_eq!(bindings.decode(wire.span()).unwrap().section, changes);
        let mut applied = before.clone();
        applied.apply(&changes).unwrap();
        assert_eq!(applied, bindings);

        update.update = RecordUpdate::GotoWaypointGoal(Update::NoProgress);
        assert_eq!(
            bindings.apply(&section(vec![update], vec![])),
            Err(Error::TypeMismatch)
        );
        assert_eq!(bindings, before);

        // The same ID can be reused with the other mode after a deletion.
        let replacement = section(vec![sample()], vec![37]);
        bindings
            .decode(replacement.encode().unwrap().span())
            .unwrap();
        applied.apply(&replacement).unwrap();
        assert_eq!(applied, bindings);
        let mut no_progress = sample();
        no_progress.initial = None;
        let update = section(vec![no_progress], vec![]);
        assert_eq!(
            bindings
                .decode(update.encode().unwrap().span())
                .unwrap()
                .section,
            update
        );
        bindings.reset();
        assert_eq!(bindings, Bindings::default());
    }

    #[test]
    fn state_preserves_progress_on_absent_updates_and_rejects_mode_changes() {
        let mut world = super::super::state::World::default();
        let goal = progressing(Some([0, 0]));
        let created = world.spawn(goal.initial.unwrap(), goal.update).unwrap();
        let update = RecordUpdate::GotoWaypointGoal(Update::Progress(Some([2, 3])));
        assert!(world.change(created.id, update.clone()).unwrap().is_some());
        assert!(world.change(created.id, update.clone()).unwrap().is_none());
        assert!(
            world
                .change(
                    created.id,
                    RecordUpdate::GotoWaypointGoal(Update::Progress(None))
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(world.get(created.id).unwrap().current, update);
        assert_eq!(
            world.change(
                created.id,
                RecordUpdate::GotoWaypointGoal(Update::NoProgress)
            ),
            Err(Error::TypeMismatch)
        );
        assert_eq!(world.get(created.id).unwrap().current, update);
        let mut invalid = progressing(None);
        invalid.update = RecordUpdate::GotoWaypointGoal(Update::NoProgress);
        assert_eq!(invalid.encode(), Err(Error::TypeMismatch));
    }
}
