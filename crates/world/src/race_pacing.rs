// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored race pacing calculations. The caller owns participants, ordering,
//! reference-player assignment and progress measurement; none are wire values.
use crate::scoring::{Error, curve::Curve};

pub const MAX_OPPONENTS: usize = 11;
pub const MAX_WAYPOINTS: usize = 255;

/// Native suffix-distance accounting, independent of the navigation service.
/// Callers supply measured segment lengths and distance to the current waypoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    suffix: Vec<f32>,
    lap_distance: f32,
    total: f32,
    laps: u8,
}

impl Route {
    /// `segments[i]` is the distance from waypoint i to i+1. `start` measures
    /// the optional start location to the first waypoint; `closing` measures the
    /// last waypoint to the first. No closing segment is added after the final lap.
    pub fn new(segments: &[f32], start: f32, closing: f32, laps: u8) -> Result<Self, Error> {
        if segments.len() >= MAX_WAYPOINTS || !(1..=15).contains(&laps) {
            return Err(Error::Bound);
        }
        for &distance in segments.iter().chain([&start, &closing]) {
            if finite(distance)? < 0. {
                return Err(Error::Bound);
            }
        }
        let mut suffix = vec![0.; segments.len() + 1];
        for i in (0..segments.len()).rev() {
            suffix[i] = finite(suffix[i + 1] + segments[i])?;
        }
        let lap_distance = finite(suffix[0] + closing)?;
        let total = finite(
            finite(finite(f32::from(laps) * suffix[0])? + start)?
                + finite(f32::from(laps - 1) * closing)?,
        )?;
        if total <= 0. {
            return Err(Error::Bound);
        }
        Ok(Self {
            suffix,
            lap_distance,
            total,
            laps,
        })
    }

    pub fn total_distance(&self) -> f32 {
        self.total
    }

    /// `lap` is one-based; `next` is the zero-based target waypoint. Progress can
    /// be negative before the start and is not clamped to the current segment.
    pub fn progress(&self, next: usize, lap: u8, distance_to_next: f32) -> Result<f32, Error> {
        finite(self.total - self.remaining_distance(next, lap, distance_to_next)?)
    }

    /// Remaining distance for the native race-position reporter. Unlike pacing's
    /// total distance, this contains no start segment. Callers supply navigation
    /// distances; the ordinary checkpoint locator is used, not its continuation.
    pub fn remaining_distance(
        &self,
        next: usize,
        lap: u8,
        distance_to_next: f32,
    ) -> Result<f32, Error> {
        if !(1..=self.laps).contains(&lap) || finite(distance_to_next)? < 0. {
            return Err(Error::Bound);
        }
        let suffix = *self.suffix.get(next).ok_or(Error::Bound)?;
        let remaining = finite(distance_to_next + suffix)?;
        let remaining_laps = finite(f32::from(self.laps - lap) * self.lap_distance)?;
        finite(remaining_laps + remaining)
    }

    /// The distance reporter multiplies the f32 remaining metres by 1000 and
    /// truncates toward zero before sorting the integer participant property.
    pub fn remaining_millimetres(
        &self,
        next: usize,
        lap: u8,
        distance_to_next: f32,
    ) -> Result<i32, Error> {
        let value = finite(self.remaining_distance(next, lap, distance_to_next)? * 1000.)?;
        // i32::MAX rounds upward in f32; refuse that boundary before conversion.
        if value < 0. || value >= 2_147_483_648. {
            return Err(Error::Bound);
        }
        Ok(value as i32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiOnlyMode {
    First,
    Last,
    Average,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    /// Empty authored arrays evaluate to zero.
    pub target: Option<Curve>,
    pub skill: Option<Curve>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Index count-1 selects the group size; index rank-1 selects its opponent.
    /// The human reference is excluded from both count and rank.
    groups: Vec<Vec<Schedule>>,
    pub extra_target_offset: f32,
    pub target_offset_scale: f32,
    pub ai_only: AiOnlyMode,
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub distance: f32,
    pub total_distance: f32,
    pub reference_distance: f32,
    pub base_skill: f32,
}

/// One active command member. Finished racers are removed before evaluation.
#[derive(Clone, Copy, Debug)]
pub struct Member {
    pub participant: u16,
    pub lap: u8,
    pub passed: u16,
    /// Elapsed race seconds at the last accepted crossing.
    pub crossing_seconds: f32,
    /// Navigation distance to the end of this lap, including its suffix.
    pub remaining_lap: f32,
    /// Distance travelled, including completed laps and the start segment.
    pub distance: f32,
    pub base_skill: f32,
}

/// The native pacing comparator differs from the results-position reporter:
/// compare remaining distance on the same lap; otherwise crossings passed,
/// then the time of that crossing. Exact ties retain explicit member order.
fn precedes(a: &Member, b: &Member) -> bool {
    if a.lap == b.lap && a.remaining_lap != b.remaining_lap {
        a.remaining_lap < b.remaining_lap
    } else if a.passed != b.passed {
        a.passed > b.passed
    } else {
        a.crossing_seconds < b.crossing_seconds
    }
}

fn finite(v: f32) -> Result<f32, Error> {
    v.is_finite().then_some(v).ok_or(Error::NonFinite)
}

impl Config {
    pub fn new(
        groups: Vec<Vec<Schedule>>,
        extra_target_offset: f32,
        target_offset_scale: f32,
        ai_only: AiOnlyMode,
    ) -> Result<Self, Error> {
        if groups.is_empty()
            || groups.len() > MAX_OPPONENTS
            || groups
                .iter()
                .enumerate()
                .any(|(i, group)| group.len() != i + 1)
        {
            return Err(Error::Bound);
        }
        finite(extra_target_offset)?;
        finite(target_offset_scale)?;
        Ok(Self {
            groups,
            extra_target_offset,
            target_offset_scale,
            ai_only,
        })
    }

    pub fn opponent_counts(&self) -> usize {
        self.groups.len()
    }

    /// Evaluate one human's race, or its remaining opponents after the human
    /// releases the pacing command. Values remain in the input member order.
    /// Each event supplies its own reference participant; there is no global
    /// current player. Multiple human reference groups need explicit assignment.
    pub fn evaluate_members(
        &self,
        members: &[Member],
        reference: Option<u16>,
        total_distance: f32,
    ) -> Result<Vec<f32>, Error> {
        if members.len() > MAX_OPPONENTS + usize::from(reference.is_some())
            || finite(total_distance)? <= 0.
        {
            return Err(Error::Bound);
        }
        let mut ids = std::collections::BTreeSet::new();
        for m in members {
            if m.participant == 0
                || m.participant >= 8192
                || !ids.insert(m.participant)
                || !(1..=15).contains(&m.lap)
                || m.passed > 15 * MAX_WAYPOINTS as u16
                || finite(m.crossing_seconds)? < 0.
                || finite(m.remaining_lap)? < 0.
            {
                return Err(Error::Bound);
            }
            finite(m.distance)?;
            finite(m.base_skill)?;
        }
        if reference.is_some_and(|id| !ids.contains(&id)) {
            return Err(Error::Bound);
        }
        let mut order: Vec<_> = (0..members.len())
            .filter(|&i| Some(members[i].participant) != reference)
            .collect();
        // Match the bounded native insertion comparator, retaining input order
        // when all three measurements tie.
        for i in 1..order.len() {
            let mut at = i;
            while at > 0 && precedes(&members[order[at]], &members[order[at - 1]]) {
                order.swap(at, at - 1);
                at -= 1;
            }
        }
        let mut values: Vec<_> = members.iter().map(|m| m.base_skill).collect();
        if order.is_empty() {
            return Ok(values);
        }
        let reference_distance = match reference {
            Some(id) => {
                members
                    .iter()
                    .find(|m| m.participant == id)
                    .ok_or(Error::Bound)?
                    .distance
            }
            None => match self.ai_only {
                AiOnlyMode::First => members[order[0]].distance,
                AiOnlyMode::Last => members[*order.last().ok_or(Error::Bound)?].distance,
                AiOnlyMode::Average => {
                    let mut sum = 0.;
                    for &i in &order {
                        sum = finite(sum + members[i].distance)?;
                    }
                    finite(sum / order.len() as f32)?
                }
            },
        };
        for (rank, &i) in order.iter().enumerate() {
            let m = members[i];
            values[i] = self.evaluate(
                order.len(),
                rank + 1,
                Progress {
                    distance: m.distance,
                    total_distance,
                    reference_distance,
                    base_skill: m.base_skill,
                },
            )?;
        }
        Ok(values)
    }

    /// Evaluate one opponent after its group and rank have been determined.
    /// Preserve the native f32 operation order. The skill curve clamps its input;
    /// the final personality-plus-modifier result has no additional clamp.
    pub fn evaluate(&self, count: usize, rank: usize, p: Progress) -> Result<f32, Error> {
        let schedule = count
            .checked_sub(1)
            .and_then(|i| self.groups.get(i))
            .and_then(|group| rank.checked_sub(1).and_then(|i| group.get(i)))
            .ok_or(Error::Bound)?;
        finite(p.distance)?;
        finite(p.reference_distance)?;
        finite(p.base_skill)?;
        if finite(p.total_distance)? <= 0.0 {
            return Err(Error::Bound);
        }
        let normalized = finite(p.distance / p.total_distance)?;
        let target = schedule
            .target
            .as_ref()
            .map_or(Ok(0.), |c| c.evaluate(normalized))?;
        let offset = finite(finite(self.extra_target_offset + target)? * self.target_offset_scale)?;
        let target_distance = finite(offset + p.reference_distance)?;
        let difference = finite(p.distance - target_distance)?;
        let modifier = schedule
            .skill
            .as_ref()
            .map_or(Ok(0.), |c| c.evaluate(difference))?;
        finite(p.base_skill + modifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::curve::{Interpolation, Knot};

    #[test]
    fn route_progress_respects_laps_start_segment_and_invalid_distances() {
        let route = Route::new(&[5.], 5., 5., 2).unwrap();
        assert_eq!(route.total_distance(), 20.);
        assert_eq!(route.progress(0, 1, 5.).unwrap(), 0.);
        assert_eq!(route.progress(0, 1, 7.).unwrap(), -2.);
        assert_eq!(route.progress(1, 1, 5.).unwrap(), 5.);
        assert_eq!(route.progress(1, 1, 0.).unwrap(), 10.);
        assert_eq!(route.progress(0, 2, 5.).unwrap(), 10.);
        assert_eq!(route.progress(1, 2, 5.).unwrap(), 15.);
        assert_eq!(route.progress(1, 2, 0.).unwrap(), 20.);
        for (next, lap, distance) in [
            (2, 1, 0.),
            (0, 0, 0.),
            (0, 3, 0.),
            (0, 1, -1.),
            (0, 1, f32::NAN),
        ] {
            assert!(route.progress(next, lap, distance).is_err());
        }
        assert_eq!(Route::new(&[5.], 5., 99., 1).unwrap().total_distance(), 10.);
        assert!(Route::new(&[f32::NAN], 0., 0., 1).is_err());
        assert!(Route::new(&[-1.], 0., 0., 1).is_err());
        assert!(Route::new(&[0.], 0., 0., 1).is_err());
        assert!(Route::new(&[f32::MAX], 1., 0., 2).is_err());
        assert!(Route::new(&[1.; MAX_WAYPOINTS], 0., 0., 1).is_err());
    }

    #[test]
    fn standings_use_truncated_remaining_millimetres_without_the_start_segment() {
        let route = Route::new(&[5.], 700., 7., 2).unwrap();
        assert_eq!(route.remaining_distance(0, 1, 3.).unwrap(), 20.);
        assert_eq!(route.remaining_millimetres(0, 1, 3.).unwrap(), 20_000);
        assert_eq!(route.remaining_millimetres(1, 2, 0.0019).unwrap(), 1);
        assert_eq!(route.remaining_millimetres(1, 2, 0.0009).unwrap(), 0);
        assert!(route.remaining_millimetres(1, 2, f32::NAN).is_err());
        assert!(route.remaining_millimetres(1, 2, 3_000_000.).is_err());
        assert!(route.remaining_millimetres(1, 3, 0.).is_err());
    }

    fn curve(x: [f32; 2], y: [f32; 2], descending: bool) -> Curve {
        Curve::new(
            x,
            y,
            1.,
            1.,
            Interpolation::Linear,
            vec![
                Knot {
                    x: 0.,
                    y: if descending { 1. } else { 0. },
                    coefficient: 0.,
                },
                Knot {
                    x: 1.,
                    y: if descending { 0. } else { 1. },
                    coefficient: 0.,
                },
            ],
        )
        .unwrap()
    }

    fn config() -> Config {
        let first = Schedule {
            target: Some(curve([0., 1.], [0., 100.], false)),
            skill: Some(curve([-100., 100.], [-0.4, 0.4], true)),
        };
        let second = Schedule {
            target: Some(curve([0., 1.], [-100., 0.], true)),
            skill: first.skill.clone(),
        };
        Config::new(
            vec![vec![first.clone()], vec![first, second]],
            10.,
            2.,
            AiOnlyMode::Average,
        )
        .unwrap()
    }

    #[test]
    fn group_rank_target_offset_and_skill_use_distinct_inputs() {
        let c = config();
        let p = Progress {
            distance: 500.,
            total_distance: 1000.,
            reference_distance: 450.,
            base_skill: 0.5,
        };
        // First target = 450 + (10 + 50)*2 = 570; difference=-70.
        assert!((c.evaluate(2, 1, p).unwrap() - 0.78).abs() < 0.000001);
        // Second target = 450 + (10 - 50)*2 = 370; difference=130 clamps.
        assert!((c.evaluate(2, 2, p).unwrap() - 0.1).abs() < 0.000001);
        assert_eq!(c.evaluate(1, 1, p), c.evaluate(2, 1, p));
        assert!(c.evaluate(2, 0, p).is_err());
        assert!(c.evaluate(1, 2, p).is_err());
        assert!(c.evaluate(3, 1, p).is_err());
        assert!(c.evaluate(0, 1, p).is_err());
    }

    #[test]
    fn rejects_invalid_measurements_and_preserves_unclamped_skill() {
        let c = config();
        let p = Progress {
            distance: 0.,
            total_distance: 1000.,
            reference_distance: 1000.,
            base_skill: 0.9,
        };
        assert!((c.evaluate(1, 1, p).unwrap() - 1.3).abs() < 0.000001);
        for total_distance in [0., -1., f32::NAN, f32::INFINITY] {
            assert!(
                c.evaluate(
                    1,
                    1,
                    Progress {
                        total_distance,
                        ..p
                    }
                )
                .is_err()
            );
        }
        assert!(
            c.evaluate(
                1,
                1,
                Progress {
                    distance: f32::NAN,
                    ..p
                }
            )
            .is_err()
        );
        assert!(
            c.evaluate(
                1,
                1,
                Progress {
                    reference_distance: f32::INFINITY,
                    ..p
                }
            )
            .is_err()
        );
        assert!(
            c.evaluate(
                1,
                1,
                Progress {
                    base_skill: f32::NAN,
                    ..p
                }
            )
            .is_err()
        );
        let mut overflow = c.clone();
        overflow.target_offset_scale = f32::MAX;
        assert!(overflow.evaluate(1, 1, p).is_err());
        assert!(Config::new(vec![], 0., 1., AiOnlyMode::First).is_err());
        assert!(Config::new(vec![vec![]], 0., 1., AiOnlyMode::Last).is_err());
        let empty = Config::new(
            vec![vec![Schedule {
                target: None,
                skill: None,
            }]],
            0.,
            1.,
            AiOnlyMode::Average,
        )
        .unwrap();
        assert_eq!(empty.evaluate(1, 1, p).unwrap(), p.base_skill);
    }

    fn member(id: u16, distance: f32) -> Member {
        Member {
            participant: id,
            lap: 1,
            passed: 0,
            crossing_seconds: 0.,
            remaining_lap: 1000. - distance,
            distance,
            base_skill: 0.5,
        }
    }

    #[test]
    fn reference_is_excluded_from_rank_and_finishing_reselects_the_schedule() {
        let c = config();
        let members = [member(10, 450.), member(20, 400.), member(30, 500.)];
        let values = c.evaluate_members(&members, Some(10), 1000.).unwrap();
        assert_eq!(values[0], 0.5);
        assert!((values[2] - 0.78).abs() < 0.000001);
        // Rank two's descending target is -40: target=390, difference=10,
        // modifier=-0.04. The faster opponent uses rank one's target instead.
        assert!((values[1] - 0.46).abs() < 0.000001);
        // The leader finished: the other opponent now uses the one-AI schedule.
        let remaining = c.evaluate_members(&members[..2], Some(10), 1000.).unwrap();
        assert_eq!(remaining[0], 0.5);
        assert!((remaining[1] - 0.9).abs() < 0.000001);
        // The human finished: authored AI-only mode supplies the reference.
        for (mode, reference_distance) in [
            (AiOnlyMode::First, 500.),
            (AiOnlyMode::Last, 400.),
            (AiOnlyMode::Average, 450.),
        ] {
            let mut c = c.clone();
            c.ai_only = mode;
            let values = c.evaluate_members(&members[1..], None, 1000.).unwrap();
            for (i, rank) in [(0, 2), (1, 1)] {
                assert_eq!(
                    values[i],
                    c.evaluate(
                        2,
                        rank,
                        Progress {
                            distance: members[i + 1].distance,
                            total_distance: 1000.,
                            reference_distance,
                            base_skill: 0.5,
                        }
                    )
                    .unwrap()
                );
            }
        }
    }

    #[test]
    fn pacing_rank_uses_lap_distance_then_crossing_progress_and_time() {
        let mut a = member(1, 100.);
        let mut b = member(2, 200.);
        a.passed = 3;
        b.passed = 2;
        assert!(precedes(&b, &a)); // Same lap: distance has priority.
        a.lap = 2;
        assert!(precedes(&a, &b)); // Different laps: crossing count has priority.
        b = a;
        b.participant = 2;
        b.crossing_seconds = 2.;
        assert!(precedes(&a, &b));
        a.crossing_seconds = 2.;
        assert!(!precedes(&a, &b) && !precedes(&b, &a));
        let c = config();
        assert!(c.evaluate_members(&[a, a], None, 1000.).is_err());
        assert!(c.evaluate_members(&[a], Some(2), 1000.).is_err());
        a.distance = f32::NAN;
        assert!(c.evaluate_members(&[a], None, 1000.).is_err());
        assert_eq!(c.evaluate_members(&[], None, 1000.).unwrap(), vec![]);
    }
}
