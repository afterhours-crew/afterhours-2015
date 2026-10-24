// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Race-owned checkpoint progress and finishing order. The caller authenticates
//! the reporting car and supplies its participant and checkpoint ordinal.
//! Measurements come from the distance reporter; no client position or rank is
//! accepted as a result. Times are on the shared host/client replication clock.
use crate::{race_pacing::MAX_WAYPOINTS, replication::Error};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PARTICIPANTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Participant {
    pub id: u16,
    pub next: usize,
    /// One-based lap, incremented past the final lap at completion.
    pub lap: u8,
    pub passed: u16,
    pub finished: Option<u16>,
    /// Removed from racing without a finish (for example a DNF deadline).
    pub withdrawn: bool,
    last: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    pub passed: usize,
    pub next: usize,
    pub lap: u8,
    pub seconds: f32,
    /// One-based finishing position, set only by the final crossing.
    pub finish: Option<u16>,
    /// The waypoint-channel goal's index and separate crossing counter.
    /// At the finish its owning command completes instead of publishing a pair.
    pub goal_progress: Option<[u8; 2]>,
}

impl Crossing {
    /// Result analysis multiplies the f32 seconds by 1000, then PropertyCast
    /// truncates to an integer. Its Precision field only affects string output.
    pub fn milliseconds(self) -> Result<i32, Error> {
        let value = self.seconds * 1000.;
        if !value.is_finite() || !(0.0..2_147_483_648.0).contains(&value) {
            return Err(Error::Bound);
        }
        Ok(value as i32)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Race {
    checkpoints: usize,
    laps: u8,
    start: f64,
    participants: Vec<Participant>,
    finished: u16,
}

impl Race {
    pub fn new(checkpoints: usize, laps: u8, members: &[u16], start: f64) -> Result<Self, Error> {
        if !(1..=MAX_WAYPOINTS).contains(&checkpoints)
            || !(1..=15).contains(&laps)
            || members.is_empty()
            || members.len() > MAX_PARTICIPANTS
            || !start.is_finite()
            || start < 0.
            || members.iter().any(|&id| id == 0 || id >= 8192)
            || members.iter().collect::<BTreeSet<_>>().len() != members.len()
        {
            return Err(Error::Bound);
        }
        Ok(Self {
            checkpoints,
            laps,
            start,
            participants: members
                .iter()
                .map(|&id| Participant {
                    id,
                    next: 0,
                    lap: 1,
                    passed: 0,
                    finished: None,
                    withdrawn: false,
                    last: start,
                })
                .collect(),
            finished: 0,
        })
    }

    pub fn participants(&self) -> &[Participant] {
        &self.participants
    }

    pub fn participant(&self, id: u16) -> Option<&Participant> {
        self.participants.iter().find(|p| p.id == id)
    }

    pub fn crossing_seconds(&self, id: u16) -> Option<f32> {
        self.participant(id).map(|p| (p.last - self.start) as f32)
    }

    /// A withdrawal never increments the finish counter or assigns a result.
    pub fn withdraw(&mut self, id: u16) -> bool {
        let Some(p) = self.participants.iter_mut().find(|p| p.id == id) else {
            return false;
        };
        if p.finished.is_some() || p.withdrawn {
            return false;
        }
        p.withdrawn = true;
        true
    }

    /// Unknown participants, repeats, out-of-order checkpoints and stale reports
    /// are inert. Malformed time is an error without changing any state.
    /// Finish positions follow accepted crossing order, as the result counter does.
    pub fn cross(
        &mut self,
        participant: u16,
        checkpoint: usize,
        time: f64,
    ) -> Result<Option<Crossing>, Error> {
        if !time.is_finite() || time < 0. || !(time - self.start).abs().is_finite() {
            return Err(Error::Shape);
        }
        let Some(p) = self.participants.iter_mut().find(|p| p.id == participant) else {
            return Ok(None);
        };
        if p.finished.is_some() || p.withdrawn || checkpoint != p.next || time < p.last {
            return Ok(None);
        }
        let seconds = (time - self.start) as f32;
        if !seconds.is_finite() {
            return Err(Error::Bound);
        }
        let next = (p.next + 1) % self.checkpoints;
        let lap = p.lap + u8::from(next == 0);
        let passed = p.passed + 1;
        let finish = (lap > self.laps).then_some(self.finished + 1);
        let crossing = Crossing {
            passed: checkpoint,
            next,
            lap,
            seconds,
            finish,
            goal_progress: finish.is_none().then_some([next as u8, passed as u8]),
        };
        p.next = next;
        p.lap = lap;
        p.passed = passed;
        p.last = time;
        p.finished = finish;
        if finish.is_some() {
            self.finished += 1;
        }
        Ok(Some(crossing))
    }

    /// Ordinal rank of active participants by ascending remaining millimetres,
    /// offset by the finished count. Finished positions never change.
    /// Measurements must cover precisely the active set so missing cars cannot
    /// silently become leaders. Equal distances retain the supplied member order
    /// (host policy: native pointer order is not portable).
    pub fn positions(&self, distances: &[(u16, i32)]) -> Result<BTreeMap<u16, u16>, Error> {
        let mut values = BTreeMap::new();
        for &(id, distance) in distances {
            if distance < 0
                || self
                    .participant(id)
                    .is_none_or(|p| p.finished.is_some() || p.withdrawn)
                || values.insert(id, distance).is_some()
            {
                return Err(Error::Shape);
            }
        }
        let mut active = Vec::new();
        let mut positions = BTreeMap::new();
        for p in &self.participants {
            if p.withdrawn {
                continue;
            }
            if let Some(position) = p.finished {
                positions.insert(p.id, position);
            } else {
                active.push((p.id, *values.get(&p.id).ok_or(Error::Shape)?));
            }
        }
        active.sort_by_key(|(_, distance)| *distance);
        positions.extend(
            active
                .into_iter()
                .enumerate()
                .map(|(i, (id, _))| (id, self.finished + i as u16 + 1)),
        );
        Ok(positions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdrawn_participants_cannot_finish_or_change_the_finish_counter() {
        let mut race = Race::new(1, 1, &[7, 8, 9], 10.).unwrap();
        assert!(race.withdraw(7));
        assert!(!race.withdraw(7));
        assert!(!race.withdraw(100));
        assert_eq!(race.cross(7, 0, 11.).unwrap(), None);
        assert_eq!(race.cross(8, 0, 12.).unwrap().unwrap().finish, Some(1));
        assert!(!race.withdraw(8));
        assert!(race.positions(&[(7, 0), (9, 50)]).is_err());
        assert_eq!(
            race.positions(&[(9, 50)]).unwrap(),
            BTreeMap::from([(8, 1), (9, 2)])
        );
    }

    #[test]
    fn interleaved_racers_finish_once_in_accepted_order() {
        let mut race = Race::new(2, 2, &[20, 10, 30], 5.).unwrap();
        for (time, id, checkpoint) in [
            (6., 20, 0),
            (7., 10, 0),
            (8., 20, 1),
            (9., 30, 0),
            (10., 10, 1),
            (11., 10, 0),
            (12., 20, 0),
        ] {
            let crossed = race.cross(id, checkpoint, time).unwrap().unwrap();
            assert_eq!(crossed.finish, None);
            assert_eq!(
                crossed.goal_progress.unwrap()[1],
                race.participant(id).unwrap().passed as u8
            );
        }
        let first = race.cross(10, 1, 13.).unwrap().unwrap();
        assert_eq!((first.finish, first.lap, first.next), (Some(1), 3, 0));
        assert_eq!(first.goal_progress, None);
        assert_eq!(first.seconds, 8.);
        let before = race.clone();
        assert_eq!(race.cross(10, 1, 14.).unwrap(), None);
        assert_eq!(race.cross(10, 0, 15.).unwrap(), None);
        assert_eq!(race, before);
        assert_eq!(race.cross(20, 1, 14.).unwrap().unwrap().finish, Some(2));
        assert_eq!(
            race.positions(&[(30, 50)]).unwrap(),
            BTreeMap::from([(10, 1), (20, 2), (30, 3)])
        );
    }

    #[test]
    fn invalid_and_stale_crossings_cannot_advance_a_race_or_another_instance() {
        let mut race = Race::new(3, 1, &[7], 10.).unwrap();
        let independent = race.clone();
        for (id, checkpoint, time) in [(8, 0, 11.), (7, 1, 11.), (7, 0, 9.)] {
            assert_eq!(race.cross(id, checkpoint, time).unwrap(), None);
            assert_eq!(race, independent);
        }
        assert!(race.cross(7, 0, f64::NAN).is_err());
        assert!(race.cross(7, 0, f64::INFINITY).is_err());
        assert!(race.cross(7, 0, f64::MAX).is_err());
        assert_eq!(race, independent);
        assert_eq!(
            race.cross(7, 0, 12.).unwrap().unwrap().goal_progress,
            Some([1, 1])
        );
        let progressed = race.clone();
        assert_eq!(race.cross(7, 1, 11.).unwrap(), None);
        assert_eq!(race.cross(7, 0, 13.).unwrap(), None);
        assert_eq!(race, progressed);
        assert_eq!(independent.participant(7).unwrap().next, 0);
    }

    #[test]
    fn standings_use_distance_and_keep_finished_positions() {
        let mut race = Race::new(1, 1, &[30, 10, 20], 0.).unwrap();
        assert_eq!(
            race.positions(&[(20, 5), (30, 50), (10, 5)]).unwrap(),
            BTreeMap::from([(10, 1), (20, 2), (30, 3)])
        );
        race.cross(30, 0, 1.).unwrap();
        assert_eq!(
            race.positions(&[(20, 2), (10, 3)]).unwrap(),
            BTreeMap::from([(30, 1), (20, 2), (10, 3)])
        );
        for values in [
            vec![(20, 2)],
            vec![(20, 2), (10, -1)],
            vec![(20, 2), (10, 3), (30, 0)],
            vec![(20, 2), (10, 3), (20, 2)],
        ] {
            assert!(race.positions(&values).is_err());
        }
        race.cross(20, 0, 2.).unwrap();
        race.cross(10, 0, 3.).unwrap();
        assert_eq!(
            race.positions(&[]).unwrap(),
            BTreeMap::from([(30, 1), (20, 2), (10, 3)])
        );
    }

    #[test]
    fn results_truncate_the_f32_millisecond_product() {
        let mut race = Race::new(1, 1, &[1], 0.).unwrap();
        let crossing = race.cross(1, 0, 12.3458).unwrap().unwrap();
        assert_eq!(crossing.milliseconds().unwrap(), 12_345);
        for seconds in [f32::NAN, f32::INFINITY, -1., 3_000_000.] {
            assert!(
                Crossing {
                    seconds,
                    ..crossing
                }
                .milliseconds()
                .is_err()
            );
        }
    }

    #[test]
    fn course_and_membership_bounds_and_long_lap_counters() {
        for (count, laps, ids, time) in [
            (0, 1, vec![1], 0.),
            (256, 1, vec![1], 0.),
            (2, 0, vec![1], 0.),
            (2, 16, vec![1], 0.),
            (2, 1, vec![], 0.),
            (2, 1, vec![1, 1], 0.),
            (2, 1, vec![0], 0.),
            (2, 1, vec![8192], 0.),
            (2, 1, (1..=129).collect(), 0.),
            (2, 1, vec![1], f64::NAN),
        ] {
            assert!(Race::new(count, laps, &ids, time).is_err());
        }
        let mut race = Race::new(255, 15, &[1], 0.).unwrap();
        for index in 0..3825 {
            let crossed = race.cross(1, index % 255, index as f64).unwrap().unwrap();
            if index == 3824 {
                assert_eq!(crossed.finish, Some(1));
            } else {
                assert_eq!(
                    crossed.goal_progress,
                    Some([((index + 1) % 255) as u8, (index + 1) as u8])
                );
            }
        }
    }
}
