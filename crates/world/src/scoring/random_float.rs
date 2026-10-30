// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Fresh RandomFloat draws (0375f7c0 / 033f5330 / 03400140). The host supplies
//! seed context and entity identity. There is no retained per-entity generator:
//! identical deterministic context repeats the same value on every event.
use super::{Error, finite};

pub const MIN: u32 = 0x0b87_c2cf;
pub const MAX: u32 = 0x0b87_c3d1;
pub const OUT: u32 = 0x0b87_df4b;
pub const MAX_PENDING_SEEDS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Context {
    /// Resolved native mode-2 seed, after its optional seed-provider call.
    pub deterministic: u32,
    /// Native 02ab1f44 contribution, shifted left 16 with wrapping arithmetic.
    pub epoch: u32,
}

impl Context {
    pub fn seed(self, entity: u32) -> u32 {
        self.deterministic
            .wrapping_add(self.epoch << 16)
            .wrapping_add(entity)
    }
}

/// Exact 31-bit modular steps and Float32 conversion. Native normalizes zero
/// before masking, then normalizes it again at draw time. The float range is
/// [2^-23, 1], not the common [0, 1) convention.
pub fn unit(seed: u32) -> f32 {
    let seed = (if seed == 0 { 1 } else { seed }) & 0x7fff_ffff;
    let mut value = u64::from(if seed == 0 { 1 } else { seed });
    for multiplier in [0x5e30, 0x661f] {
        value = value * multiplier % 0x7fff_ffff;
    }
    ((((value as u32 | 128) >> 7) + 1) as f32) * f32::from_bits(0x3380_0000)
}

pub fn draw(seed: u32, min: f32, max: f32) -> Result<f32, Error> {
    finite(min)?;
    finite(max)?;
    // Preserve the native subss, mulss, addss rounding, including reversed bounds.
    let width = finite(max - min)?;
    finite(unit(seed) * width + min)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_integer_and_float_reference_vectors() {
        for (seed, bits) in [
            (0, 1_050_036_768),
            (1, 1_050_036_768),
            (2, 1_058_425_376),
            (12345, 1_059_991_192),
            (0x12345678, 1_041_185_808),
            (0x7fffffff, 872_415_232),
            (0x80000000, 1_050_036_768),
            (0xffffffff, 872_415_232),
        ] {
            assert_eq!(unit(seed).to_bits(), bits);
        }
    }
    #[test]
    fn seed_context_wraps_and_range_preserves_equal_and_reversed_bounds() {
        assert_eq!(
            Context {
                deterministic: u32::MAX,
                epoch: 0x10001
            }
            .seed(2),
            65537
        );
        assert_eq!(draw(1, 7., 7.), Ok(7.));
        assert_eq!(draw(1, 3., -2.), Ok(unit(1) * -5. + 3.));
        assert!(draw(1, -f32::MAX, f32::MAX).is_err());
        assert!(draw(1, f32::NAN, 2.).is_err());
    }
}
