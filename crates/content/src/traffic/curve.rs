// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed cubic road geometry and its distance-to-parameter table.
use super::{Error, MAX_SEGMENT_POINTS, bad};

#[derive(Clone, Debug)]
pub struct Curve {
    controls: [[f32; 3]; 4],
    times: Vec<f32>,
    distances: Vec<f32>,
}

impl Curve {
    pub fn new(
        controls: [[f32; 3]; 4],
        times: Vec<f32>,
        distances: Vec<f32>,
    ) -> Result<Self, Error> {
        if controls
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 100_000.)
            || times.is_empty()
            || times.len() >= MAX_SEGMENT_POINTS
            || times.len() != distances.len()
            || times.last() != Some(&1.)
        {
            return Err(bad("curve shape"));
        }
        let mut previous = [0., 0.];
        for (&t, &d) in times.iter().zip(&distances) {
            if !t.is_finite()
                || t <= previous[0]
                || t > 1.
                || !d.is_finite()
                || d <= previous[1]
                || d > 100_000.
            {
                return Err(bad("curve distance table"));
            }
            previous = [t, d];
        }
        Ok(Self {
            controls,
            times,
            distances,
        })
    }

    pub fn length(&self) -> f32 {
        *self.distances.last().expect("validated table")
    }
    pub fn point_count(&self) -> usize {
        self.times.len() + 1
    }
    pub fn distance_at_point(&self, point: usize) -> Option<f32> {
        if point == 0 {
            Some(0.)
        } else {
            self.distances.get(point - 1).copied()
        }
    }
    pub fn position(&self, t: f32) -> [f32; 3] {
        // De Casteljau evaluation remains stable near either endpoint.
        let mut p = self.controls;
        for n in (1..4).rev() {
            for i in 0..n {
                p[i] = std::array::from_fn(|axis| p[i][axis] * (1. - t) + p[i + 1][axis] * t);
            }
        }
        p[0]
    }
    /// Position and analytic forward tangent at an authored arc distance.
    pub fn sample(&self, distance: f32) -> Option<([f32; 3], [f32; 3])> {
        if !distance.is_finite() || distance < 0. || distance > self.length() {
            return None;
        }
        let i = self
            .distances
            .partition_point(|&d| d < distance)
            .min(self.distances.len() - 1);
        let start_d = if i == 0 { 0. } else { self.distances[i - 1] };
        let start_t = if i == 0 { 0. } else { self.times[i - 1] };
        let t = start_t
            + (self.times[i] - start_t) * ((distance - start_d) / (self.distances[i] - start_d));
        let p = self.controls;
        let mut tangent = std::array::from_fn(|a| {
            3. * ((p[1][a] - p[0][a]) * (1. - t).powi(2)
                + 2. * (p[2][a] - p[1][a]) * (1. - t) * t
                + (p[3][a] - p[2][a]) * t * t)
        });
        if norm(tangent) < 1e-5 {
            // Repeated endpoint handles still define a direction. Interior
            // cusps have no unique forward tangent and cannot drive a car.
            if t == 0. {
                tangent = p[1..]
                    .iter()
                    .map(|v| std::array::from_fn(|a| v[a] - p[0][a]))
                    .find(|v| norm(*v) >= 1e-5)?;
            } else if t == 1. {
                tangent = p[..3]
                    .iter()
                    .rev()
                    .map(|v| std::array::from_fn(|a| p[3][a] - v[a]))
                    .find(|v| norm(*v) >= 1e-5)?;
            } else {
                return None;
            }
        }
        Some((self.position(t), tangent))
    }
    pub(super) fn validate_points(&self, points: &[[f32; 3]]) -> Result<(), Error> {
        if points.len() != self.times.len() + 1 {
            return Err(bad("curve point count"));
        }
        for (&t, p) in std::iter::once(&0.).chain(&self.times).zip(points) {
            if norm(std::array::from_fn(|a| p[a] - self.position(t)[a])) > 0.01 {
                return Err(bad("curve point mismatch"));
            }
        }
        Ok(())
    }
}
fn norm(v: [f32; 3]) -> f32 {
    v.iter().map(|v| v * v).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curve_tangents_change_continuously_across_distance_table_entries() {
        let c = Curve::new(
            [[0., 0., 0.], [0., 2., 4.], [4., 2., 4.], [4., 0., 0.]],
            vec![0.5, 1.],
            vec![4., 8.],
        )
        .unwrap();
        assert_eq!(c.sample(0.).unwrap().0, [0.; 3]);
        assert_eq!(c.sample(4.).unwrap().0, [2., 1.5, 3.]);
        assert_eq!(c.sample(8.).unwrap().0, [4., 0., 0.]);
        let a = c.sample(3.999).unwrap().1;
        let b = c.sample(4.001).unwrap().1;
        assert!(norm(std::array::from_fn(|i| a[i] - b[i])) < 0.01);
        assert!(c.sample(f32::NAN).is_none());
        assert!(c.sample(8.1).is_none());
        assert!(
            c.validate_points(&[[0.; 3], [2., 1.5, 3.], [4., 0., 0.]])
                .is_ok()
        );
        assert!(
            c.validate_points(&[[0.; 3], [2., 0., 2.], [4., 0., 0.]])
                .is_err()
        );
    }
    #[test]
    fn invalid_tables_and_degenerate_handles_are_bounded() {
        let p = [[0.; 3], [0.; 3], [0.; 3], [3., 0., 0.]];
        let c = Curve::new(p, vec![1.], vec![3.]).unwrap();
        assert_eq!(c.sample(0.).unwrap().1, [3., 0., 0.]);
        for (t, d) in [
            (vec![], vec![]),
            (vec![0.5, 1.], vec![1., 1.]),
            (vec![1., 0.5], vec![1., 2.]),
            (vec![1.], vec![f32::INFINITY]),
        ] {
            assert!(Curve::new(p, t, d).is_err());
        }
        assert!(
            Curve::new([[0.; 3]; 4], vec![1.], vec![1.])
                .unwrap()
                .sample(0.)
                .is_none()
        );
    }
}
