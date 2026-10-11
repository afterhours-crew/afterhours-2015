// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Update};

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    guid: [u32; 4],
    maximum: f32,
}
impl Profile {
    pub fn new(guid: [u32; 4], points: &[i32]) -> Result<Self, Error> {
        if points.len() > 256 || points.iter().any(|v| *v < 0) {
            return Err(Error::Bound);
        }
        let (blocks, rest) = points.as_chunks::<8>();
        let mut a = [0_f32; 4];
        let mut b = [0_f32; 4];
        for block in blocks {
            for i in 0..4 {
                a[i] += block[i] as f32;
                b[i] += block[i + 4] as f32;
            }
        }
        for i in 0..4 {
            a[i] += b[i];
        }
        let mut maximum = (a[2] + a[0]) + (a[3] + a[1]);
        for value in rest {
            maximum += *value as f32;
        }
        Ok(Self { guid, maximum })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    profile: Option<Profile>,
    maximum: f32,
    current: f32,
}
impl State {
    pub fn new(max_health: f32, profile: Option<Profile>) -> Result<Self, Error> {
        if !max_health.is_finite() || max_health < 0. {
            return Err(Error::Bound);
        }
        let maximum = profile.as_ref().map_or(max_health, |p| p.maximum);
        Ok(Self {
            profile,
            maximum,
            current: maximum,
        })
    }
    pub fn maximum(&self) -> f32 {
        self.maximum
    }
    pub fn set_current(&mut self, value: f32) -> Result<(), Error> {
        if !value.is_finite() {
            return Err(Error::Bound);
        }
        self.current = value;
        Ok(())
    }
    pub fn update(&self, mask: u8) -> Result<Update, Error> {
        if mask & !3 != 0 {
            return Err(Error::Bound);
        }
        Ok(Update::GuidFloat {
            guid: self
                .profile
                .as_ref()
                .filter(|_| mask & 1 != 0)
                .map(|p| p.guid),
            value: (mask & 2 != 0).then_some(self.current.to_bits()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_profile_overrides_component_maximum_and_masks_are_independent() {
        let s = State::new(500., Some(Profile::new([1, 2, 3, 4], &[75, 25]).unwrap())).unwrap();
        assert_eq!(s.maximum(), 100.);
        assert_eq!(
            s.update(3),
            Ok(Update::GuidFloat {
                guid: Some([1, 2, 3, 4]),
                value: Some(100_f32.to_bits())
            })
        );
        assert_eq!(
            s.update(1),
            Ok(Update::GuidFloat {
                guid: Some([1, 2, 3, 4]),
                value: None
            })
        );
        assert_eq!(
            s.update(2),
            Ok(Update::GuidFloat {
                guid: None,
                value: Some(100_f32.to_bits())
            })
        );
        assert_eq!(
            s.update(0),
            Ok(Update::GuidFloat {
                guid: None,
                value: None
            })
        );
    }
    #[test]
    fn absent_profile_differs_from_present_nonexported_and_empty_profiles() {
        let absent = State::new(42., None).unwrap();
        assert_eq!(
            absent.update(3),
            Ok(Update::GuidFloat {
                guid: None,
                value: Some(42_f32.to_bits())
            })
        );
        let zero = State::new(42., Some(Profile::new([0; 4], &[]).unwrap())).unwrap();
        assert_eq!(
            zero.update(3),
            Ok(Update::GuidFloat {
                guid: Some([0; 4]),
                value: Some(0)
            })
        );
    }
    #[test]
    fn segment_sum_preserves_simd_rounding_instead_of_integer_or_linear_sum() {
        let p = Profile::new([0; 4], &[16_777_216, 1, 1, 1, 1, 1, 1, 1]).unwrap();
        assert_eq!(p.maximum, 16_777_222.);
        for n in 0..=256 {
            assert_eq!(Profile::new([0; 4], &vec![1; n]).unwrap().maximum, n as f32);
        }
    }
    #[test]
    fn updates_are_isolated_and_invalid_mutations_leave_state_intact() {
        let mut a = State::new(100., None).unwrap();
        let b = a.clone();
        a.set_current(-5.).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            a.update(2),
            Ok(Update::GuidFloat {
                guid: None,
                value: Some((-5_f32).to_bits())
            })
        );
        let before = a.clone();
        assert_eq!(a.set_current(f32::NAN), Err(Error::Bound));
        assert_eq!(a, before);
        assert_eq!(a.update(4), Err(Error::Bound));
        for max in [-1., f32::NAN, f32::INFINITY] {
            assert_eq!(State::new(max, None), Err(Error::Bound));
        }
        assert_eq!(Profile::new([0; 4], &[-1]), Err(Error::Bound));
        assert_eq!(Profile::new([0; 4], &[1; 257]), Err(Error::Bound));
    }
}
