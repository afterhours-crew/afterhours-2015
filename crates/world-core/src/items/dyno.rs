// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Derived, Error};
use crate::bits::BitWriter;
use nfs_protocol::world::BitSpan;

/// Client-calculated dyno cache, not server physics or an upgrade price.
/// Integer order is top speed, horsepower, torque, quarter-mile speed.
/// Time words retain the normalized RPC's decoded f32 bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DynoStats {
    integers: [u32; 4],
    times: [u32; 3],
}
const MAXIMUM: [u32; 4] = [300, 1500, 1500, 200];
const TIME_MAXIMUM: [f32; 3] = [15.0, 35.0, 20.0];

impl DynoStats {
    /// Validate and normalize the same bounded values carried by the RPC.
    pub fn new(integers: [u32; 4], times: [f32; 3]) -> Result<Self, Error> {
        if integers
            .iter()
            .zip(MAXIMUM)
            .any(|(&v, max)| !(50..=max).contains(&v))
            || times
                .iter()
                .zip(TIME_MAXIMUM)
                .any(|(&v, max)| !v.is_finite() || !(0.0..=max).contains(&v))
        {
            return Err(Error::Shape);
        }
        let value = Self {
            integers,
            times: times.map(f32::to_bits),
        };
        Self::read(value.encode().span()).map(|(value, _)| value)
    }
    pub fn integers(self) -> [u32; 4] {
        self.integers
    }
    pub fn times(self) -> [f32; 3] {
        self.times.map(f32::from_bits)
    }

    /// Read exactly the dyno fields; returns their bit extent before RPC padding.
    pub fn read(span: BitSpan<'_>) -> Result<(Self, usize), Error> {
        let mut at = 0;
        let mut take = |width| {
            let value = span.read_u32(at, width).map_err(|_| Error::Truncated)?;
            at += usize::from(width);
            Ok::<_, Error>(value)
        };
        let mut integers = [0; 4];
        for (i, width) in [(0, 8), (1, 11), (2, 11)] {
            integers[i] = take(width)? + 50;
            if integers[i] > MAXIMUM[i] {
                return Err(Error::Shape);
            }
        }
        let mut times = [0; 3];
        for i in 0..3 {
            if i == 2 {
                integers[3] = take(8)? + 50;
                if integers[3] > MAXIMUM[3] {
                    return Err(Error::Shape);
                }
            }
            let zero = take(1)? != 0;
            let unit = if zero {
                0.0
            } else {
                let mantissa = take(23)?;
                let biased = if mantissa == 0 {
                    0x4000_0000
                } else {
                    0x3f80_0000 | mantissa
                };
                f32::from_bits(biased) - 1.0
            };
            times[i] = (unit * TIME_MAXIMUM[i]).to_bits();
        }
        Ok((Self { integers, times }, at))
    }
    pub fn encode(self) -> BitWriter {
        let mut out = BitWriter::new();
        out.put(u64::from(self.integers[0] - 50), 8)
            .put(u64::from(self.integers[1] - 50), 11)
            .put(u64::from(self.integers[2] - 50), 11);
        for (i, max) in TIME_MAXIMUM.into_iter().enumerate() {
            if i == 2 {
                out.put(u64::from(self.integers[3] - 50), 8);
            }
            let biased = (f32::from_bits(self.times[i]) / max + 1.0).to_bits();
            let zero = biased == 1.0f32.to_bits();
            out.put_bool(zero);
            if !zero {
                out.put(u64::from(biased & 0x7f_ffff), 23);
            }
        }
        out
    }
    /// Change only the cached results. Preserve appearance, flags and item graph.
    pub fn apply(self, derived: &mut Derived) -> Result<(), Error> {
        let Derived::RaceVehicle {
            max_hp,
            max_torque,
            zero_to_sixty,
            zero_to_one_hundred,
            top_speed_mph,
            quarter_mile_mph,
            quarter_mile_time,
            ..
        } = derived
        else {
            return Err(Error::Shape);
        };
        *top_speed_mph = self.integers[0];
        *max_hp = self.integers[1];
        *max_torque = self.integers[2];
        *quarter_mile_mph = self.integers[3];
        *zero_to_sixty = self.times[0];
        *zero_to_one_hundred = self.times[1];
        *quarter_mile_time = self.times[2];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_results_preserve_appearance_and_flags() {
        let raw: Vec<_> = (1u32..=17).flat_map(u32::to_le_bytes).collect();
        let mut value = Derived::decode(super::super::Layout::RaceVehicle, &raw).unwrap();
        let stats = DynoStats::new([180, 400, 500, 120], [4.5, 10., 12.5]).unwrap();
        stats.apply(&mut value).unwrap();
        let result = value.encode().unwrap();
        assert_eq!(&result[..36], &raw[..36]);
        assert_eq!(&result[64..], &raw[64..]);
        let times = stats.times();
        let expected: Vec<_> = [
            400,
            500,
            times[0].to_bits(),
            times[1].to_bits(),
            180,
            120,
            times[2].to_bits(),
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
        assert_eq!(&result[36..64], &expected);
        assert_eq!(stats.apply(&mut Derived::Empty), Err(Error::Shape));
    }
    #[test]
    fn native_integer_ranges_and_variable_float_extents() {
        let low = DynoStats::new([50; 4], [0.; 3]).unwrap();
        assert_eq!(low.encode().len(), 41);
        assert_eq!(low.encode().bytes(), &[0, 0, 0, 3, 0, 128]);
        let high = DynoStats::new(MAXIMUM, TIME_MAXIMUM).unwrap();
        assert_eq!(high.encode().len(), 110);
        assert_eq!(high.times(), TIME_MAXIMUM);
        for value in [
            low,
            high,
            DynoStats::new([177, 400, 510, 124], [4.25, 10.5, 12.75]).unwrap(),
        ] {
            let encoded = value.encode();
            assert_eq!(DynoStats::read(encoded.span()), Ok((value, encoded.len())));
            for end in 0..encoded.len() {
                assert!(DynoStats::read(encoded.span().slice(0, end).unwrap()).is_err());
            }
        }
        let tiny = DynoStats::new([50; 4], [f32::MIN_POSITIVE; 3]).unwrap();
        assert_eq!(tiny, low);
        for invalid in [f32::NAN, f32::INFINITY, -1.0, 100.0] {
            assert!(DynoStats::new([50; 4], [invalid; 3]).is_err());
        }
        for i in 0..4 {
            for invalid in [49, MAXIMUM[i] + 1] {
                let mut values = [50; 4];
                values[i] = invalid;
                assert!(DynoStats::new(values, [0.; 3]).is_err());
            }
        }
        let mut bad = BitWriter::new();
        bad.put(255, 8);
        assert_eq!(DynoStats::read(bad.span()), Err(Error::Shape));
    }
}
