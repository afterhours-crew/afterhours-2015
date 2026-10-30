// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Host completion data consumed by the client's Speedwall presentation.
//! The three numeric statistics retain their IEEE-754 bits. String length is
//! ten bits, followed by the bytes without a terminator; final byte padding is
//! unspecified. The reliable MessageEX chunk transport is supplied by the edge.
use crate::{bits::BitWriter, replication::Error};
use nfs_protocol::world::BitSpan;

pub const CONTENT_TARGET: u8 = 93;
pub const MAX_VEHICLE_BYTES: usize = 1023;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Progress {
    pub score_bits: u32,
    /// Elapsed milliseconds represented as an f32, rather than seconds.
    pub time_ms_bits: u32,
    pub rep_bits: u32,
    /// One-based result position.
    pub position: u32,
    pub event_id: u32,
    /// Installed RaceVehicleShortNameId token, not translated display text.
    pub vehicle: Vec<u8>,
}

impl Progress {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.vehicle.len() > MAX_VEHICLE_BYTES || self.vehicle.contains(&0) {
            return Err(Error::Bound);
        }
        let mut out = BitWriter::new();
        for value in [
            self.score_bits,
            self.time_ms_bits,
            self.rep_bits,
            self.position,
            self.event_id,
        ] {
            out.put(u64::from(value), 32);
        }
        out.put(self.vehicle.len() as u64, 10);
        for &byte in &self.vehicle {
            out.put(u64::from(byte), 8);
        }
        Ok(out.bytes().to_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > (170 + MAX_VEHICLE_BYTES * 8).div_ceil(8) {
            return Err(Error::Bound);
        }
        let span = BitSpan::new(bytes, 0, bytes.len() * 8).map_err(|_| Error::Shape)?;
        let get = |at, width| span.read_u32(at, width).map_err(|_| Error::Shape);
        let length = get(160, 10)? as usize;
        let bits = 170 + length * 8;
        if span.len() < bits || span.len() - bits >= 8 {
            return Err(Error::Shape);
        }
        let vehicle = (0..length)
            .map(|i| get(170 + i * 8, 8).map(|v| v as u8))
            .collect::<Result<Vec<_>, _>>()?;
        if vehicle.contains(&0) {
            return Err(Error::Shape);
        }
        Ok(Self {
            score_bits: get(0, 32)?,
            time_ms_bits: get(32, 32)?,
            rep_bits: get(64, 32)?,
            position: get(96, 32)?,
            event_id: get(128, 32)?,
            vehicle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Progress {
        Progress {
            score_bits: 0f32.to_bits(),
            time_ms_bits: 123_456f32.to_bits(),
            rep_bits: 7500f32.to_bits(),
            position: 3,
            event_id: 0x8765_4321,
            vehicle: b"ID_CAR_CONSTRUCTED_SHORT_NAME".to_vec(),
        }
    }

    #[test]
    fn completion_fields_and_unaligned_string_preserve_bits() {
        let mut value = example();
        let bytes = value.encode().unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(&bytes[12..20], &[0, 0, 0, 3, 0x87, 0x65, 0x43, 0x21]);
        assert_eq!(Progress::decode(&bytes).unwrap(), value);
        let mut padded = bytes;
        *padded.last_mut().unwrap() |= 0x3f;
        assert_eq!(Progress::decode(&padded).unwrap(), value);
        value.score_bits = 0x7fc0_1234;
        assert_eq!(Progress::decode(&value.encode().unwrap()).unwrap(), value);
    }

    #[test]
    fn incomplete_extra_and_oversized_strings_are_rejected() {
        let mut value = example();
        let bytes = value.encode().unwrap();
        for end in 0..bytes.len() {
            assert!(Progress::decode(&bytes[..end]).is_err());
        }
        let mut extra = bytes;
        extra.push(0);
        assert!(Progress::decode(&extra).is_err());
        value.vehicle = vec![b'X'; MAX_VEHICLE_BYTES];
        assert_eq!(Progress::decode(&value.encode().unwrap()).unwrap(), value);
        value.vehicle.push(b'X');
        assert_eq!(value.encode(), Err(Error::Bound));
        value.vehicle = vec![0];
        assert!(value.encode().is_err());
        value.vehicle.clear();
        assert_eq!(Progress::decode(&value.encode().unwrap()).unwrap(), value);
    }
}
