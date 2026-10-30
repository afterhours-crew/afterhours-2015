// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ScriptedInputGoal (registry 19): native client reader 03f7ec10 and server
//! writer 03fbbed0. No update body. Command interpreter 03fbfed0 preserves the
//! four authored controls in throttle, steering, brake, handbrake order.
use super::{Error, Reader, Writer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initial {
    pub selector: u16,
    pub serial: u16,
    pub allocation_serial: u8,
    /// Finite IEEE-754 f32 bits. Native does not clamp these at serialization.
    pub throttle: u32,
    pub steering: u32,
    pub brake: u32,
    pub handbrake: u32,
    pub use_nos: bool,
}
impl Initial {
    fn controls(&self) -> [u32; 4] {
        [self.throttle, self.steering, self.brake, self.handbrake]
    }
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let result = Self {
            selector: r.take(9)? as u16,
            serial: r.take(10)? as u16,
            allocation_serial: r.take(6)? as u8,
            throttle: r.take(32)?,
            steering: r.take(32)?,
            brake: r.take(32)?,
            handbrake: r.take(32)?,
            use_nos: r.bit()?,
        };
        if result
            .controls()
            .into_iter()
            .any(|v| !f32::from_bits(v).is_finite())
        {
            return Err(Error::Shape);
        }
        Ok(result)
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.allocation_serial >= 64 {
            return Err(Error::Bound);
        }
        if self
            .controls()
            .into_iter()
            .any(|v| !f32::from_bits(v).is_finite())
        {
            return Err(Error::Shape);
        }
        w.put(u64::from(self.selector), 9)?;
        w.put(u64::from(self.serial), 10)?;
        w.put(u64::from(self.allocation_serial), 6)?;
        for value in self.controls() {
            w.put(u64::from(value), 32)?;
        }
        w.bit(self.use_nos)
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
            initial: Some(Creation::ScriptedInputGoal(Initial {
                selector: 7,
                serial: 6,
                allocation_serial: 63,
                throttle: 1f32.to_bits(),
                steering: (-0.5f32).to_bits(),
                brake: 0.25f32.to_bits(),
                handbrake: (-0.0f32).to_bits(),
                use_nos: true,
            })),
            update: Update::ScriptedInputGoal,
        }
    }
    #[test]
    fn native_wire_order_boundary_and_every_truncation() {
        // Independent pack of the traced native reader's exact field order.
        let mut wire = BitWriter::new();
        wire.put(41, 13)
            .put(1, 1)
            .put(19, 5)
            .put(7, 9)
            .put(6, 10)
            .put(63, 6);
        for value in [0x3f80_0000, 0xbf00_0000, 0x3e80_0000, 0x8000_0000] {
            wire.put(value, 32);
        }
        wire.put(1, 1);
        let decoded = Record::decode(wire.span(), None).unwrap();
        assert_eq!(decoded.record, sample());
        assert_eq!((decoded.bits, decoded.initial_bits), (173, Some(173)));
        assert_eq!(sample().encode().unwrap().span(), wire.span());
        for bits in 0..173 {
            assert_eq!(
                Record::decode(wire.span().slice(0, bits).unwrap(), None),
                Err(Error::Truncated)
            );
        }
        wire.put(0x5a, 8);
        assert_eq!(Record::decode(wire.span(), None).unwrap().bits, 173);
    }
    #[test]
    fn no_update_body_and_invalid_fields_are_rejected() {
        let update = Record {
            id: 41,
            initial: None,
            update: Update::ScriptedInputGoal,
        };
        let wire = update.encode().unwrap();
        assert_eq!(wire.len(), 14);
        assert_eq!(
            Record::decode(wire.span(), Some(Kind::ScriptedInputGoal))
                .unwrap()
                .record,
            update
        );
        for field in 0..7 {
            let mut record = sample();
            let Some(Creation::ScriptedInputGoal(goal)) = record.initial.as_mut() else {
                unreachable!()
            };
            match field {
                0 => goal.selector = 512,
                1 => goal.serial = 1024,
                2 => goal.allocation_serial = 64,
                3 => goal.throttle = f32::NAN.to_bits(),
                4 => goal.steering = f32::INFINITY.to_bits(),
                5 => goal.brake = f32::NEG_INFINITY.to_bits(),
                _ => goal.handbrake = f32::NAN.to_bits(),
            }
            assert!(record.encode().is_err());
        }
    }
}
