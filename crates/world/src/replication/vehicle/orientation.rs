// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Quaternion25};

pub fn spawn_vector(forward: [f32; 3], speed: Option<f32>) -> Result<[Option<u32>; 3], Error> {
    if forward.iter().any(|v| !v.is_finite()) || speed.is_some_and(|v| !v.is_finite()) {
        return Err(Error::Shape);
    }
    let squares = forward.map(|v| v * v);
    let length = ((squares[1] + squares[0]) + squares[2]).sqrt();
    if !length.is_finite() || length == 0.0 {
        return Err(Error::Shape);
    }
    let reciprocal = 1.0 / length;
    let values = forward.map(|v| {
        let normalized = v * reciprocal;
        speed.map_or(normalized, |speed| normalized * speed)
    });
    if values.iter().any(|v| !v.is_finite()) {
        return Err(Error::Shape);
    }
    Ok(values.map(|v| (v != 0.0).then(|| v.to_bits())))
}

pub fn quaternion_from_basis(basis: [[f32; 3]; 3]) -> Result<[f32; 4], Error> {
    if basis.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Error::Shape);
    }
    let [a, b, c] = basis;
    let determinant = (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]))
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    if !determinant.is_finite() || determinant <= 0.0 {
        return Err(Error::Shape);
    }
    let mut q = [0.0; 4];
    let trace = (basis[0][0] + basis[1][1]) + basis[2][2];
    if trace > 0.0 {
        q[3] = (trace + 1.0).sqrt() * 0.5;
        let scale = 1.0 / (q[3] * 4.0);
        for (i, j, k) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
            q[i] = (basis[j][k] - basis[k][j]) * scale;
        }
    } else {
        let mut i = usize::from(basis[1][1] > basis[0][0]);
        if basis[2][2] > basis[i][i] {
            i = 2;
        }
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        q[i] = ((basis[i][i] - (basis[k][k] + basis[j][j])) + 1.0).sqrt() * 0.5;
        let scale = 1.0 / (q[i] * 4.0);
        q[3] = (basis[j][k] - basis[k][j]) * scale;
        q[j] = (basis[i][j] + basis[j][i]) * scale;
        q[k] = (basis[i][k] + basis[k][i]) * scale;
    }
    if q.iter().any(|v| !v.is_finite()) {
        return Err(Error::Shape);
    }
    Ok(q)
}

impl Quaternion25 {
    pub fn from_basis(basis: [[f32; 3]; 3]) -> Result<Self, Error> {
        Self::from_quaternion(quaternion_from_basis(basis)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    #[test]
    fn common_spawn_and_explicit_speed_are_distinct() {
        let direction = [3.0, -0.0, 4.0];
        assert_eq!(
            spawn_vector(direction, None).unwrap(),
            [Some(0.6f32.to_bits()), None, Some(0.8f32.to_bits())]
        );
        assert_eq!(spawn_vector(direction, Some(0.0)).unwrap(), [None; 3]);
        assert_eq!(
            spawn_vector(direction, Some(-5.0)).unwrap(),
            [Some((-3.0f32).to_bits()), None, Some((-4.0f32).to_bits())]
        );
        assert_eq!(direction, [3.0, -0.0, 4.0]);
    }

    #[test]
    fn matrix_conversion_handles_identity_and_all_largest_diagonal_branches() {
        assert_eq!(
            quaternion_from_basis(IDENTITY).unwrap(),
            [0.0, 0.0, 0.0, 1.0]
        );
        for axis in 0..3 {
            let mut basis = IDENTITY;
            for (i, row) in basis.iter_mut().enumerate() {
                if i != axis {
                    row[i] = -1.0;
                }
            }
            let mut expected = [0.0; 4];
            expected[axis] = 1.0;
            assert_eq!(quaternion_from_basis(basis).unwrap(), expected);
            let packed = Quaternion25::from_basis(basis).unwrap();
            assert_eq!(packed.axes[axis].mantissa, Some(0));
        }
    }

    #[test]
    fn matrix_conversion_preserves_rotation_sign_and_does_not_remove_scale() {
        let a = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        let b = [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]];
        let qa = quaternion_from_basis(a).unwrap();
        let qb = quaternion_from_basis(b).unwrap();
        assert!(qa[1] > 0.0 && qb[1] < 0.0);
        assert_eq!(qa[1], -qb[1]);
        assert_eq!(qa[3], qb[3]);
        let scaled = IDENTITY.map(|axis| axis.map(|v| v * 2.0));
        assert_ne!(
            quaternion_from_basis(scaled).unwrap(),
            quaternion_from_basis(IDENTITY).unwrap()
        );
    }

    #[test]
    fn invalid_or_degenerate_pose_is_rejected() {
        for value in [f32::NAN, f32::INFINITY] {
            assert!(spawn_vector([value, 0.0, 1.0], None).is_err());
            let mut basis = IDENTITY;
            basis[0][0] = value;
            assert!(quaternion_from_basis(basis).is_err());
        }
        assert!(spawn_vector([f32::MAX, 0.0, 0.0], None).is_err());
        assert!(quaternion_from_basis(IDENTITY.map(|axis| axis.map(|v| v * f32::MAX))).is_err());
        assert!(spawn_vector([0.0; 3], None).is_err());
        assert!(spawn_vector([1.0, 0.0, 0.0], Some(f32::NAN)).is_err());
        assert!(quaternion_from_basis([[0.0; 3]; 3]).is_err());
        let mut reflected = IDENTITY;
        reflected[0][0] = -1.0;
        assert!(quaternion_from_basis(reflected).is_err());
    }
}
