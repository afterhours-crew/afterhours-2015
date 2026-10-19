// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ambient road sampling, focus geometry and bounded population selection.
use super::{Car, Content, Error, distance_squared, horizontal_distance_squared, road_basis};
use std::collections::{BTreeMap, BTreeSet};

const GRID: usize = 33;
const MAX_CANDIDATES: usize = 131_072;
const INNER2: f32 = 300. * 300.;
const OUTER2: f32 = 450. * 450.;

#[derive(Clone, Copy, Debug)]
pub struct Focus {
    pub id: u16,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub forward: [f32; 3],
}

impl Focus {
    pub(super) fn validate(&self) -> Result<(), Error> {
        if self
            .position
            .iter()
            .chain(&self.velocity)
            .chain(&self.forward)
            .any(|v| !v.is_finite())
            || self.position.iter().any(|v| v.abs() > 100_000.)
            || !dot(self.velocity, self.velocity).is_finite()
            || (dot(self.forward, self.forward) - 1.).abs() > 0.01
        {
            return Err(Error::Bound);
        }
        Ok(())
    }

    fn predicted(&self) -> [f32; 3] {
        let speed = dot(self.velocity, self.velocity).sqrt();
        let scale = if speed > 0.1 {
            (speed * 3.).min(300.) / speed
        } else {
            0.
        };
        std::array::from_fn(|i| self.position[i] + self.velocity[i] * scale)
    }

    pub(super) fn retains(&self, position: [f32; 3]) -> bool {
        let predicted = self.predicted();
        let middle = std::array::from_fn(|i| (self.position[i] + predicted[i]) * 0.5);
        horizontal_distance_squared(position, self.position) < OUTER2
            || horizontal_distance_squared(position, predicted) < INNER2
            || horizontal_distance_squared(position, middle) < INNER2
    }

    fn eligible(&self, candidate: &Candidate) -> bool {
        let offset = subtract(candidate.position, self.predicted());
        let length = dot(offset, offset).sqrt();
        if length == 0. {
            return false;
        }
        let alignment = dot(offset.map(|v| v / length), self.forward);
        let factor = 1. / (alignment * alignment * 0.75 + 0.25);
        let shaped = length * factor;
        shaped * shaped > INNER2 && shaped * shaped < OUTER2 + 20_000.
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Candidate {
    pub road: usize,
    pub position: [f32; 3],
    pub basis: [[f32; 3]; 3],
}

#[derive(Clone, Debug)]
struct Road {
    quota: u32,
    spacing: f32,
    personas: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct Policy {
    cells: Vec<Vec<Candidate>>,
    roads: Vec<Road>,
    // The selector indexes this segment table by population-road ordinal.
    // Retain that indexing, including segments with no sampled candidates.
    speeds: Vec<f32>,
}

pub(super) struct Query<'a> {
    candidates: Vec<&'a Candidate>,
    pub target: usize,
    /// Player-chosen traffic amount (percent of the original), applied to the
    /// target and to each road's quota.
    percent: u16,
}

impl Policy {
    pub fn new(content: &Content) -> Result<Self, Error> {
        if content.splines.len() > nfs_content::traffic::MAX_SPLINES
            || content.personas.len() > nfs_content::traffic::MAX_PERSONAS
        {
            return Err(Error::Bound);
        }
        let mut policy = Self {
            cells: vec![Vec::new(); GRID * GRID],
            roads: Vec::new(),
            speeds: Vec::new(),
        };
        let mut count = 0;
        let mut segments = 0;
        for (road, spline) in content.splines.iter().enumerate() {
            let mut length = 0.;
            let mut carry = 0.;
            for segment in &spline.segments {
                segments += 1;
                if segments > nfs_content::traffic::MAX_SEGMENTS
                    || !segment.speed.is_finite()
                    || segment.speed < 0.
                {
                    return Err(Error::Bound);
                }
                policy.speeds.push((segment.speed * 0.447_04).powi(2));
                let segment_length = segment.curve.length();
                length += segment_length;
                let mut step = 0;
                while carry + step as f32 * 25. < segment_length {
                    count += 1;
                    if count > MAX_CANDIDATES {
                        return Err(Error::Bound);
                    }
                    let (position, tangent) = segment
                        .curve
                        .sample(carry + step as f32 * 25.)
                        .ok_or(Error::Shape)?;
                    let basis = road_basis([0.; 3], tangent).ok_or(Error::Shape)?;
                    let x = cell(position[0]);
                    let z = cell(position[2]);
                    if x >= GRID || z >= GRID {
                        return Err(Error::Bound);
                    }
                    policy.cells[x * GRID + z].push(Candidate {
                        road,
                        position,
                        basis,
                    });
                    step += 1;
                }
                carry = (step as f32 * 25. - segment_length) + carry + 0.1;
            }
            if !length.is_finite() {
                return Err(Error::Shape);
            }
            policy.roads.push(Road {
                quota: if spline.density == 0 {
                    0
                } else {
                    (length / f32::from(spline.density)) as u32
                },
                spacing: if spline.density == 0 {
                    1000.
                } else {
                    f32::from(spline.density)
                },
                personas: content
                    .personas
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.tags.iter().any(|t| spline.tags.contains(t)))
                    .map(|(i, _)| i)
                    .collect(),
            });
        }
        if policy.speeds.iter().any(|v| !v.is_finite())
            || policy
                .cells
                .iter()
                .flatten()
                .any(|c| c.road >= policy.speeds.len())
        {
            return Err(Error::Bound);
        }
        Ok(policy)
    }

    pub fn query(&self, focus: &Focus, percent: u16) -> Query<'_> {
        let predicted = focus.predicted();
        let x = cell(predicted[0] - 225.);
        let z = cell(predicted[2] - 225.);
        let mut candidates = Vec::new();
        let mut roads = BTreeSet::new();
        let mut factor = 1f32;
        for x in x..(x + 2).min(GRID) {
            for z in z..(z + 2).min(GRID) {
                for candidate in &self.cells[x * GRID + z] {
                    let distance = horizontal_distance_squared(candidate.position, predicted);
                    if distance > OUTER2 * 0.1 && distance < OUTER2 {
                        candidates.push(candidate);
                        roads.insert(candidate.road);
                        factor = factor.max(1. + (100. - self.roads[candidate.road].spacing) * 0.1);
                    }
                }
            }
        }
        let score = (roads.len() as f32 * factor) as u32;
        let target = ((10. * (score as f32 / 16.).clamp(0., 1.)) as u32).clamp(2, 10);
        Query {
            candidates,
            target: crate::ai_settings::scale(target, percent) as usize,
            percent,
        }
    }

    pub fn nearby(&self, focus: &Focus, cars: &BTreeMap<u64, Car>) -> usize {
        cars.values()
            .filter(|c| horizontal_distance_squared(c.anchor, focus.position) < OUTER2)
            .count()
    }

    pub fn select(
        &self,
        query: &Query<'_>,
        focus: &Focus,
        all: &[Focus],
        cars: &BTreeMap<u64, Car>,
        counts: &BTreeMap<usize, u32>,
        random: [u32; 2],
    ) -> Option<Candidate> {
        if query.candidates.is_empty() {
            return None;
        }
        let near: Vec<_> = cars
            .values()
            .filter(|c| distance_squared(c.anchor, focus.position) <= 100. * 100.)
            .collect();
        if near.len() >= 3
            && near
                .iter()
                .map(|c| {
                    let speed2 = dot(c.velocity, c.velocity);
                    if speed2 <= 0.000_001 {
                        0.
                    } else {
                        speed2.sqrt()
                    }
                })
                .sum::<f32>()
                / (near.len() as f32)
                < 5.
        {
            return None;
        }
        let start = random[0] as usize % query.candidates.len();
        let eligible: Vec<_> = query.candidates.iter().cycle().skip(start).take(query.candidates.len()).copied().filter(|candidate| {
            focus.eligible(candidate)
                && !all.iter().any(|other| other.id != focus.id && horizontal_distance_squared(other.position, candidate.position) < INNER2)
                && !cars.values().any(|car| dot(car.forward, candidate.basis[2]) > 0. && horizontal_distance_squared(car.anchor, candidate.position) <= 50. * 50.)
                // An unseen road is permitted once; the count entry survives retirement.
                && counts.get(&candidate.road).is_none_or(|&count| count < crate::ai_settings::scale(self.roads[candidate.road].quota, query.percent))
                && self.approaching(focus, candidate)
        }).take(20).collect();
        (!eligible.is_empty()).then(|| *eligible[random[1] as usize % eligible.len()])
    }

    fn approaching(&self, focus: &Focus, candidate: &Candidate) -> bool {
        if dot(focus.velocity, focus.velocity) >= self.speeds[candidate.road] {
            return true;
        }
        let ahead = dot(subtract(candidate.position, focus.position), focus.forward);
        let heading = dot(candidate.basis[2], focus.forward);
        if ahead < 0. {
            heading >= 0.
        } else {
            heading < 0.
        }
    }

    pub fn persona(&self, road: usize, cursor: usize) -> Option<(usize, usize)> {
        let entries = &self.roads[road].personas;
        if entries.is_empty() {
            return None;
        }
        Some((
            entries[if cursor < entries.len() { cursor } else { 0 }],
            (cursor + 1) % entries.len(),
        ))
    }
}

fn cell(value: f32) -> usize {
    ((value as i32).wrapping_add(8192) as u32 >> 9) as usize
}
fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}

#[cfg(test)]
mod tests;
