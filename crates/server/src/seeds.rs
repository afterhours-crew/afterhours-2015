// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;

pub trait SeedSource: Send {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Failure>;
}
#[derive(Default)]
pub struct OsSeeds;

impl SeedSource for OsSeeds {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Failure> {
        for _ in 0..4 {
            openssl::rand::rand_bytes(out).map_err(|_| Failure::Reply)?;
            if out.iter().any(|b| *b != 0) {
                return Ok(());
            }
        }
        Err(Failure::Reply)
    }
}
pub struct FixedSeeds {
    label: u8,
    counter: u32,
}

impl FixedSeeds {
    pub fn new(label: u8) -> Self {
        Self { label, counter: 0 }
    }
}

impl SeedSource for FixedSeeds {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Failure> {
        self.counter = self.counter.wrapping_add(1);
        let counter = self.counter.to_le_bytes();
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = (self.label ^ counter[i % 4] ^ (i as u8).wrapping_mul(31)) | 1;
        }
        Ok(())
    }
}

pub fn fresh<const N: usize>(source: &mut dyn SeedSource) -> Result<[u8; N], Failure> {
    let mut seed = [0; N];
    source.fill(&mut seed)?;
    Ok(seed)
}
#[derive(Default)]
pub struct OsRandom;

impl nfs_world::application::Random for OsRandom {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        for _ in 0..4 {
            if openssl::rand::rand_bytes(&mut bytes).is_ok() && bytes != [0; 4] {
                break;
            }
        }
        u32::from_le_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_seeds_are_nonzero_and_differ() {
        let mut source = OsSeeds;
        let a: [u8; 48] = fresh(&mut source).unwrap();
        let b: [u8; 48] = fresh(&mut source).unwrap();
        assert!(a.iter().any(|v| *v != 0));
        assert_ne!(a, b);
    }

    #[test]
    fn fixed_seeds_repeat_per_label_and_never_zero() {
        let mut one = FixedSeeds::new(7);
        let mut two = FixedSeeds::new(7);
        let a: [u8; 16] = fresh(&mut one).unwrap();
        assert_eq!(a, fresh::<16>(&mut two).unwrap());
        assert!(a.iter().all(|v| *v != 0));
        assert_ne!(a, fresh::<16>(&mut one).unwrap());
    }
}
