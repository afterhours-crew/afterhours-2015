// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, SparseVector, Vector};

impl SparseVector {
    pub fn from_vector(value: [f32; 3], fractional: u8) -> Result<Self, Error> {
        Ok(match Vector::from_position(value, [0.; 3], fractional)? {
            Vector::FloatBits(bits) => Self::FloatBits(bits),
            Vector::Packed { mode, values } => {
                let values = values.map(|v| (v != 0).then_some(v));
                Self::Packed {
                    mode: values.iter().any(Option::is_some).then_some(mode),
                    values,
                }
            }
        })
    }
}

impl Vector {
    pub fn from_position(
        position: [f32; 3],
        origin: [f32; 3],
        fractional: u8,
    ) -> Result<Self, Error> {
        if fractional > 16 || position.iter().chain(&origin).any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        let relative: [f32; 3] = std::array::from_fn(|i| position[i] - origin[i]);
        let maximum = relative.iter().map(|v| v.abs()).fold(0.0, f32::max);
        let mode = [1, 3, 5, 7, 9, 11, 14]
            .iter()
            .position(|width| maximum < (1u32 << (width - 1)) as f32);
        match mode {
            Some(index) => Ok(Self::Packed {
                mode: (index + 1) as u8,
                values: relative.map(|v| (v * (1u32 << fractional) as f32) as i32),
            }),
            None => Ok(Self::FloatBits(position.map(f32::to_bits))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_threshold_is_strict_and_supported_precision_stays_bounded() {
        for (index, width) in [1, 3, 5, 7, 9, 11, 14].into_iter().enumerate() {
            let limit = (1u32 << (width - 1)) as f32;
            let below = f32::from_bits(limit.to_bits() - 1);
            for fractional in [0, 5, 7, 10, 16] {
                for sign in [-1.0, 1.0] {
                    let packed =
                        Vector::from_position([below * sign, 0.0, 0.0], [0.0; 3], fractional)
                            .unwrap();
                    let Vector::Packed { mode, values } = packed else {
                        panic!("packed threshold")
                    };
                    assert_eq!(mode, (index + 1) as u8);
                    let bound = 1i32 << (width + fractional - 1);
                    assert!(values.into_iter().all(|v| (-bound..bound).contains(&v)));
                    let edge =
                        Vector::from_position([limit * sign, 0.0, 0.0], [0.0; 3], fractional)
                            .unwrap();
                    if index == 6 {
                        assert!(matches!(edge, Vector::FloatBits(_)));
                    } else {
                        assert!(
                            matches!(edge, Vector::Packed { mode, .. } if usize::from(mode) == index + 2)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn origin_is_explicit_signed_values_truncate_and_escape_is_absolute() {
        assert_eq!(
            Vector::from_position([-0.04, 0.04, 0.0], [0.0; 3], 5).unwrap(),
            Vector::Packed {
                mode: 1,
                values: [-1, 1, 0]
            }
        );
        assert_eq!(
            Vector::from_position([9000.0, 2.0, -3.0], [9000.0, 2.0, -3.0], 10).unwrap(),
            Vector::Packed {
                mode: 1,
                values: [0; 3]
            }
        );
        let position = [9000.0, -0.0, 1.0];
        assert_eq!(
            Vector::from_position(position, [1.0, 2.0, 3.0], 5).unwrap(),
            Vector::FloatBits(position.map(f32::to_bits))
        );
        assert_eq!(
            Vector::from_position([f32::MAX; 3], [-f32::MAX; 3], 16).unwrap(),
            Vector::FloatBits([f32::MAX.to_bits(); 3])
        );
    }

    #[test]
    fn invalid_domain_inputs_do_not_become_saturated_coordinates() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                Vector::from_position([value, 0.0, 0.0], [0.0; 3], 5),
                Err(Error::Shape)
            );
            assert_eq!(
                Vector::from_position([0.0; 3], [0.0, value, 0.0], 5),
                Err(Error::Shape)
            );
        }
        assert_eq!(
            Vector::from_position([0.0; 3], [0.0; 3], 17),
            Err(Error::Shape)
        );
    }
}
