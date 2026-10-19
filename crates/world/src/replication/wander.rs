// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Wandering is a distinct goal, with no path asset or update body. The host
//! supplies the three constructor flags in native wire order. Creating a goal
//! does not itself bind an actor or delegate vehicle simulation.
use super::{Error, Reader, Writer};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Initial {
    /// Shared goal allocation counter, wrapping at 64.
    pub allocation_serial: u8,
    pub flags: [bool; 3],
}
impl Initial {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            allocation_serial: r.take(6)? as u8,
            flags: [r.bit()?, r.bit()?, r.bit()?],
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.allocation_serial >= 64 {
            return Err(Error::Bound);
        }
        w.put(u64::from(self.allocation_serial), 6)?;
        for flag in self.flags {
            w.bit(flag)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::{Initial as Creation, Kind, Record, Update, state};
    use nfs_protocol::world::BitSpan;

    fn sample() -> Record {
        Record {
            id: 37,
            initial: Some(Creation::WanderGoal(Initial {
                allocation_serial: 45,
                flags: [true, false, true],
            })),
            update: Update::WanderGoal,
        }
    }

    #[test]
    fn native_layout_has_exact_boundary_and_rejects_every_truncation() {
        // Independently packed: reference37/13, creation1/1, type27/5,
        // allocation45/6, flags1,0,1. No asset, mask or update follows.
        let bytes = [0x01, 0x2f, 0x76, 0xd0];
        let input = BitSpan::new(&bytes, 0, 28).unwrap();
        let decoded = Record::decode(input, None).unwrap();
        assert_eq!(decoded.record, sample());
        assert_eq!((decoded.bits, decoded.initial_bits), (28, Some(28)));
        assert_eq!(decoded.record.encode().unwrap().span(), input);
        for bits in 0..28 {
            assert_eq!(
                Record::decode(input.slice(0, bits).unwrap(), None),
                Err(Error::Truncated)
            );
        }
    }

    #[test]
    fn empty_updates_do_not_consume_the_following_record() {
        let update = Record {
            id: 37,
            initial: None,
            update: Update::WanderGoal,
        };
        let mut joined = update.encode().unwrap();
        joined.put_span(sample().encode().unwrap().span());
        assert!(matches!(
            Record::decode(joined.span(), None),
            Err(Error::UnknownObject)
        ));
        let first = Record::decode(joined.span(), Some(Kind::WanderGoal)).unwrap();
        assert_eq!(first.record, update);
        assert_eq!((first.bits, first.initial_bits), (14, None));
        assert_eq!(
            Record::decode(joined.span().slice(14, 28).unwrap(), None)
                .unwrap()
                .record,
            sample()
        );
    }

    #[test]
    fn counter_bounds_and_snapshot_retention_are_atomic() {
        let mut state = state::World::default();
        let Creation::WanderGoal(mut initial) = sample().initial.unwrap() else {
            unreachable!()
        };
        initial.allocation_serial = 64;
        assert!(matches!(
            state.spawn(Creation::WanderGoal(initial), Update::WanderGoal),
            Err(Error::Bound)
        ));
        initial.allocation_serial = 63;
        let created = state
            .spawn(Creation::WanderGoal(initial), Update::WanderGoal)
            .unwrap();
        assert_eq!(created.id, 1);
        assert!(
            state
                .change(created.id, Update::WanderGoal)
                .unwrap()
                .is_none()
        );
        assert_eq!(state.snapshot(), vec![created]);
    }
}
