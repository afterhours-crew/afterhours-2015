// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_world::{
    race_pacing::{self, Config, Route},
    race_progress,
    replication::Error,
    road_distance::{Network, Target},
};
use std::{collections::BTreeMap, sync::Arc};

/// Shared fixed-checkpoint queries, prepared before the networking loop.
#[derive(Clone, Debug)]
pub struct Navigation {
    network: Arc<Network>,
    targets: Vec<Target>,
    segments: Vec<f32>,
    closing: f32,
    laps: u8,
    config: Config,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub participant: u16,
    pub position: [f32; 3],
    pub base_skill: f32,
}

pub struct Measured {
    /// Same order as the supplied active command members.
    pub skills: Vec<f32>,
    pub remaining: Vec<(u16, i32)>,
}

impl Navigation {
    pub fn new(
        network: Arc<Network>,
        checkpoints: &[[f32; 3]],
        laps: u8,
        config: Config,
    ) -> Result<Self, Error> {
        if checkpoints.is_empty()
            || checkpoints.len() > race_pacing::MAX_WAYPOINTS
            || !(1..=15).contains(&laps)
        {
            return Err(Error::Bound);
        }
        let locations = checkpoints
            .iter()
            .map(|&p| {
                network
                    .locate(p)
                    .map_err(|_| Error::Shape)?
                    .ok_or(Error::Unsupported)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let targets = locations
            .iter()
            .map(|&p| network.prepare_target(p).map_err(|_| Error::Shape))
            .collect::<Result<Vec<_>, _>>()?;
        let mut segments = Vec::new();
        for (i, &location) in locations[..locations.len() - 1].iter().enumerate() {
            segments.push(
                network
                    .distance(location, &targets[i + 1])
                    .map_err(|_| Error::Shape)?
                    .ok_or(Error::Unsupported)?,
            );
        }
        let closing = if laps > 1 {
            network
                .distance(*locations.last().ok_or(Error::Shape)?, &targets[0])
                .map_err(|_| Error::Shape)?
                .ok_or(Error::Unsupported)?
        } else {
            0.
        };
        Ok(Self {
            network,
            targets,
            segments,
            closing,
            laps,
            config,
        })
    }

    /// The unbound pacing command takes its first participant's current car
    /// position when it creates the shared group (E1002), not the intro locator.
    pub fn start(&self, position: [f32; 3]) -> Result<Route, Error> {
        let distance = self.distance(position, 0)?.ok_or(Error::Unsupported)?;
        Route::new(&self.segments, distance, self.closing, self.laps).map_err(|_| Error::Shape)
    }

    fn distance(&self, position: [f32; 3], next: usize) -> Result<Option<f32>, Error> {
        let target = self.targets.get(next).ok_or(Error::Bound)?;
        let Some(location) = self.network.locate(position).map_err(|_| Error::Shape)? else {
            return Ok(None);
        };
        self.network
            .distance(location, target)
            .map_err(|_| Error::Shape)
    }

    /// Measure every remaining command independently. The native query creates
    /// a zero-distance result before resolving the two road locations; an
    /// unresolved location returns without writing it (RVA 0x4116c80).
    /// A route search with fewer than two nodes also leaves distance zero.
    /// This zero replaces only distance-to-next, not the remaining route/laps.
    pub fn measure(
        &self,
        route: &Route,
        progress: &race_progress::Race,
        samples: &[Sample],
        reference: Option<u16>,
    ) -> Result<Measured, Error> {
        if samples.len() > race_pacing::MAX_OPPONENTS + 1 {
            return Err(Error::Bound);
        }
        let mut by_id = BTreeMap::new();
        for sample in samples {
            if by_id.insert(sample.participant, sample).is_some() || !sample.base_skill.is_finite()
            {
                return Err(Error::Shape);
            }
        }
        let active = progress
            .participants()
            .iter()
            .filter(|p| p.finished.is_none() && !p.withdrawn);
        if active.clone().count() != samples.len()
            || active.clone().any(|p| !by_id.contains_key(&p.id))
        {
            return Err(Error::Shape);
        }
        let mut members = Vec::with_capacity(samples.len());
        let mut remaining = Vec::with_capacity(samples.len());
        for sample in samples {
            let p = progress
                .participant(sample.participant)
                .ok_or(Error::UnknownObject)?;
            let distance = self.distance(sample.position, p.next)?.unwrap_or(0.);
            // Use the final lap to omit later laps from the comparator's value.
            let remaining_lap = route
                .remaining_distance(p.next, self.laps, distance)
                .map_err(|_| Error::Shape)?;
            members.push(race_pacing::Member {
                participant: p.id,
                lap: p.lap,
                passed: p.passed,
                crossing_seconds: progress.crossing_seconds(p.id).ok_or(Error::Shape)?,
                remaining_lap,
                distance: route
                    .progress(p.next, p.lap, distance)
                    .map_err(|_| Error::Shape)?,
                base_skill: sample.base_skill,
            });
            remaining.push((
                p.id,
                route
                    .remaining_millimetres(p.next, p.lap, distance)
                    .map_err(|_| Error::Shape)?,
            ));
        }
        let skills = self
            .config
            .evaluate_members(&members, reference, route.total_distance())
            .map_err(|_| Error::Shape)?;
        Ok(Measured { skills, remaining })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::{
        race_pacing::{AiOnlyMode, Schedule},
        road_distance::{GRID_SIZE, Lane, Spline},
    };

    fn navigation() -> Navigation {
        let mut grid = vec![vec![Vec::new(); GRID_SIZE]; GRID_SIZE];
        grid[100][100] = vec![0];
        grid[101][100] = vec![0];
        let network = Network::new(
            2,
            vec![Lane {
                start: 0,
                end: 1,
                segments: vec![0],
                layer: 1,
            }],
            vec![Spline {
                controls: [[0.; 3], [0.; 3], [200., 0., 0.], [200., 0., 0.]],
                coefficients: [[0.; 3], [200., 0., 0.], [0.; 3], [0.; 3]],
                points: vec![[0.; 3], [200., 0., 0.]],
                end_times: vec![1.],
                end_distances: vec![200.],
            }],
            grid,
        )
        .unwrap();
        let config = Config::new(
            (1..=11)
                .map(|count| {
                    vec![
                        Schedule {
                            target: None,
                            skill: None
                        };
                        count
                    ]
                })
                .collect(),
            0.,
            1.,
            AiOnlyMode::Average,
        )
        .unwrap();
        Navigation::new(
            Arc::new(network),
            &[[50., 0., 0.], [100., 0., 0.]],
            1,
            config,
        )
        .unwrap()
    }

    #[test]
    fn measures_active_members_and_keeps_finishing_order_after_human_release() {
        let nav = navigation();
        let route = nav.start([10., 0., 0.]).unwrap();
        assert!((route.total_distance() - 90.).abs() < 0.000_1);
        let mut progress = race_progress::Race::new(2, 1, &[1, 2], 10.).unwrap();
        let sample = |id, x| Sample {
            participant: id,
            position: [x, 0., 0.],
            base_skill: 0.5,
        };
        let measured = nav
            .measure(
                &route,
                &progress,
                &[sample(1, 30.), sample(2, 20.)],
                Some(1),
            )
            .unwrap();
        assert_eq!(measured.skills, [0.5, 0.5]);
        let positions = progress.positions(&measured.remaining).unwrap();
        assert_eq!(positions, BTreeMap::from([(1, 1), (2, 2)]));
        progress.cross(1, 0, 11.).unwrap();
        progress.cross(1, 1, 12.).unwrap();
        let measured = nav
            .measure(&route, &progress, &[sample(2, 25.)], None)
            .unwrap();
        assert_eq!(
            progress.positions(&measured.remaining).unwrap(),
            BTreeMap::from([(1, 1), (2, 2)])
        );
        assert!(
            nav.measure(&route, &progress, &[sample(1, 30.), sample(2, 20.)], None)
                .is_err()
        );
        assert!(nav.measure(&route, &progress, &[], None).is_err());
        let unavailable = nav
            .measure(&route, &progress, &[sample(2, 5000.)], None)
            .unwrap();
        // No next-road location: the suffix still contributes 50 metres.
        assert_eq!(unavailable.remaining, [(2, 50_000)]);
    }
}
