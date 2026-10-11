// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Axis25, Error, Quaternion14, Quaternion25};

impl Quaternion25 {
    pub fn from_quaternion(quaternion: [f32; 4]) -> Result<Self, Error> {
        if quaternion.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        let scale = if quaternion[..3].iter().any(|v| v.abs() > 1.0) {
            let squares = quaternion.map(|v| v * v);
            let length = ((squares[2] + squares[3]) + (squares[0] + squares[1])).sqrt();
            if !length.is_finite() || length == 0.0 {
                return Err(Error::Shape);
            }
            1.0 / length
        } else {
            1.0
        };
        Ok(Self {
            negative_w: quaternion[3].is_sign_negative(),
            axes: std::array::from_fn(|i| {
                let value = (quaternion[i] * scale).clamp(-1.0, 1.0);
                let biased = (value.abs() + 1.0).to_bits();
                Axis25 {
                    negative: value.is_sign_negative(),
                    mantissa: (biased != 1.0f32.to_bits()).then_some(biased & 0x7f_ffff),
                }
            }),
        })
    }
}

impl Quaternion14 {
    pub fn from_quaternion(quaternion: [f32; 4]) -> Result<Self, Error> {
        if quaternion.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        Ok(Self {
            negative_w: quaternion[3].is_sign_negative(),
            axes: std::array::from_fn(|i| {
                let value = quaternion[i].clamp(-1.0, 1.0);
                let rounded = (value * 8191.0 + 0.5f32.copysign(value)) as i16;
                (rounded != 0).then_some(rounded)
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_keeps_signs_presence_and_native_endpoint_distinct() {
        let rotation = Quaternion25::from_quaternion([-0.0, 1.0, -1.0, -0.0]).unwrap();
        assert!(rotation.negative_w);
        assert_eq!(
            rotation.axes,
            [
                Axis25 {
                    negative: true,
                    mantissa: None
                },
                Axis25 {
                    negative: false,
                    mantissa: Some(0)
                },
                Axis25 {
                    negative: true,
                    mantissa: Some(0)
                },
            ]
        );
        let tiny = 2.0f32.powi(-25);
        assert!(
            Quaternion25::from_quaternion([tiny, -tiny, 0.0, 1.0])
                .unwrap()
                .axes
                .iter()
                .all(|v| v.mantissa.is_none())
        );
        let one_step = Quaternion25::from_quaternion([2.0f32.powi(-23), 0.0, 0.0, 1.0]).unwrap();
        assert_eq!(one_step.axes[0].mantissa, Some(1));
    }
    #[test]
    fn root_normalization_is_conditional_and_includes_w() {
        let normalized = Quaternion25::from_quaternion([3.0, 0.0, 0.0, -4.0]).unwrap();
        assert!(normalized.negative_w);
        assert_eq!(normalized.axes[0].mantissa, Some(0x4c_cccd));
        let unchanged = Quaternion25::from_quaternion([0.5, 0.0, 0.0, 4.0]).unwrap();
        assert_eq!(unchanged.axes[0].mantissa, Some(0x40_0000));
    }
    #[test]
    fn physics_rounds_signed_ties_and_omits_quantized_zero() {
        let tie: f32 = 0.5 / 8191.0;
        let below = f32::from_bits(tie.to_bits() - 2);
        assert_eq!(
            Quaternion14::from_quaternion([tie, -tie, below, -0.0]).unwrap(),
            Quaternion14 {
                negative_w: true,
                axes: [Some(1), Some(-1), None]
            }
        );
        assert_eq!(
            Quaternion14::from_quaternion([f32::MAX, -f32::MAX, -below, 1.0])
                .unwrap()
                .axes,
            [Some(8191), Some(-8191), None]
        );
        assert_eq!(
            Quaternion14::from_quaternion([0.5, -0.5, -0.0, 1.0])
                .unwrap()
                .axes,
            [Some(4096), Some(-4096), None]
        );
    }
    #[test]
    fn invalid_rotations_are_rejected_before_integer_conversion() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for i in 0..4 {
                let mut q = [0.0, 0.0, 0.0, 1.0];
                q[i] = value;
                assert_eq!(Quaternion25::from_quaternion(q), Err(Error::Shape));
                assert_eq!(Quaternion14::from_quaternion(q), Err(Error::Shape));
            }
        }
        assert_eq!(
            Quaternion25::from_quaternion([f32::MAX; 4]),
            Err(Error::Shape)
        );
    }
}
