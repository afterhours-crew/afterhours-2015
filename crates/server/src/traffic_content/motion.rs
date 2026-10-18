// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Deterministic host traffic policy over installed road geometry. This is a
//! deliberately bounded road follower, not a recreation of the game's physics.
use nfs_content::traffic::Spline;
use nfs_world::replication::{Error, vehicle};

#[derive(Clone, Debug)]
pub(super) struct Route {
    pub spline: usize,
    pub segment: usize,
    pub point: usize,
    pub offset: f32,
}

impl Route {
    /// Advance through connected edges only. Gaps/endpoints retire the car;
    /// zero-length data cannot loop forever or teleport to an unrelated road.
    pub fn travel(
        &mut self,
        spline: &Spline,
        mut distance: f32,
    ) -> Option<([f32; 3], [[f32; 3]; 3])> {
        for _ in 0..512 {
            let segment = spline.segments.get(self.segment)?;
            if self.point + 1 >= segment.points.len() {
                let end = *segment.points.last()?;
                let next = spline.segments.get(self.segment + 1)?;
                if super::population::distance_squared(end, *next.points.first()?) > 1. {
                    return None;
                }
                self.segment += 1;
                self.point = 0;
                self.offset = 0.;
                continue;
            }
            let a = segment.points[self.point];
            let b = segment.points[self.point + 1];
            let length = super::population::distance_squared(a, b).sqrt();
            if length > 100. || !length.is_finite() {
                return None;
            }
            if length < 0.001 || self.offset + distance >= length {
                distance = (distance - (length - self.offset)).max(0.);
                self.offset = 0.;
                self.point += 1;
                continue;
            }
            self.offset += distance;
            let fraction = self.offset / length;
            let position = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * fraction);
            return Some((position, super::population::road_basis(a, b)?));
        }
        None
    }
}

#[derive(Clone, Debug)]
pub(super) struct Clock {
    seconds: f64,
    revision: u32,
    observed_ms: u64,
}
impl Clock {
    pub fn observe(
        anchor: &mut Option<Self>,
        now: u64,
        physics: &vehicle::Physics,
    ) -> Result<(), Error> {
        let [high, low, revision] = physics.identity_words;
        let seconds = f64::from_bits((u64::from(high) << 32) | u64::from(low));
        if !seconds.is_finite() || !(0. ..1e9).contains(&seconds) {
            return Err(Error::Bound);
        }
        if seconds == 0. || anchor.as_ref().is_some_and(|a| seconds <= a.seconds) {
            return Ok(());
        }
        *anchor = Some(Self {
            seconds,
            revision,
            observed_ms: now,
        });
        Ok(())
    }
    pub fn stamp(&self, now: u64) -> Result<(u64, u32), Error> {
        let elapsed = now.checked_sub(self.observed_ms).ok_or(Error::Bound)?;
        // The world runs at 30 simulation ticks/second. Clock words carry an
        // f64 time and a tick, as in ordinary accepted chassis reports.
        Ok((
            (self.seconds + elapsed as f64 / 1000.).to_bits(),
            self.revision
                .wrapping_add((elapsed.saturating_mul(30) / 1000) as u32),
        ))
    }
}

/// Stop for vehicles on the same floor inside the forward corridor. Both other
/// host traffic and the player are obstacles; a long scheduling gap cannot move
/// a car through one. This policy intentionally does not claim rigid-body physics.
pub(super) fn clearance(position: [f32; 3], forward: [f32; 3], obstacles: &[[f32; 3]]) -> f32 {
    obstacles
        .iter()
        .filter_map(|other| {
            let delta: [f32; 3] = std::array::from_fn(|i| other[i] - position[i]);
            let ahead = delta[0] * forward[0] + delta[2] * forward[2];
            let side = delta[0] * forward[2] - delta[2] * forward[0];
            (delta[1].abs() < 3. && ahead > 0. && side.abs() < 2.8).then_some((ahead - 9.).max(0.))
        })
        .fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_content::traffic::Segment;

    #[test]
    fn connected_roads_progress_without_skipping_gaps_or_looping_on_empty_edges() {
        let mut spline = Spline {
            index: 0,
            density: 1,
            tags: vec![],
            segments: vec![
                Segment {
                    speed: 35.,
                    points: vec![[0.; 3], [0.; 3], [10., 0., 0.]],
                },
                Segment {
                    speed: 35.,
                    points: vec![[10., 0., 0.], [10., 0., 10.]],
                },
            ],
        };
        let route = Route {
            spline: 0,
            segment: 0,
            point: 0,
            offset: 0.,
        };
        let mut r = route.clone();
        assert_eq!(r.travel(&spline, 12.).unwrap().0, [10., 0., 2.]);
        assert!(r.travel(&spline, 9.).is_none());
        spline.segments[1].points[0] = [50., 0., 0.];
        assert!(route.clone().travel(&spline, 12.).is_none());
        spline.segments[0].points = vec![[0.; 3]; 600];
        assert!(route.clone().travel(&spline, 0.).is_none());
    }

    #[test]
    fn obstacle_corridor_does_not_mix_floors_or_opposite_side_lanes() {
        assert_eq!(clearance([0.; 3], [1., 0., 0.], &[[15., 0., 0.]]), 6.);
        assert_eq!(clearance([0.; 3], [1., 0., 0.], &[[3., 0., 0.]]), 0.);
        assert!(
            clearance(
                [0.; 3],
                [1., 0., 0.],
                &[[15., 8., 0.], [15., 0., 4.], [-1., 0., 0.]]
            )
            .is_infinite()
        );
    }
}
