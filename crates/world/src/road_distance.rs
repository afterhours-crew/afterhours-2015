// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Direction-independent road distance for race progress (E1002). Unlike the
//! driving graph, this query uses authored spline distance tables and permits
//! travel in both directions. It does not add the distance off the road.
//!
//! Native pointer ordering is replaced by explicit segment order for exact
//! nearest-road ties. Candidate bounds and height-weighted nearest selection
//! follow the normal, heading-independent query. Route queries use precomputed
//! endpoint distances, so the socket worker
//! never runs a graph search.
use crate::road_graph::{Error, MAX_LANE_POINTS, MAX_LANES, MAX_NODES};
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap};

pub const GRID_SIZE: usize = 200;
pub const MAX_SEGMENTS: usize = 32_768;
const MAX_CELL_SEGMENTS: usize = 4_096;
const MAX_GRID_REFERENCES: usize = 1 << 20;
const MAX_LANE_SEGMENTS: usize = 512;

/// One DistanceMappedCubicBezierData, without any game-specific identifiers.
#[derive(Clone, Debug)]
pub struct Spline {
    pub controls: [[f32; 3]; 4],
    /// Coefficients in ascending power order: P(t) = c0 + c1*t + c2*t² + c3*t³.
    pub coefficients: [[f32; 3]; 4],
    pub points: Vec<[f32; 3]>,
    /// One entry per interval between successive points; neither table begins
    /// with an implicit zero entry.
    pub end_times: Vec<f32>,
    pub end_distances: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Lane {
    pub start: usize,
    pub end: usize,
    /// Authored order is retained, including lanes whose geometry is reversed.
    pub segments: Vec<usize>,
    pub layer: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Location {
    segment: usize,
    offset: f32,
    point: [f32; 3],
}
impl Location {
    pub fn segment(&self) -> usize {
        self.segment
    }
    pub fn offset(&self) -> f32 {
        self.offset
    }
    pub fn point(&self) -> [f32; 3] {
        self.point
    }
}

#[derive(Clone, Debug)]
struct Segment {
    spline: Spline,
    lane: usize,
    /// Distances before and after this segment, in native accumulation order.
    before: f32,
    after: f32,
}

#[derive(Clone, Debug)]
pub struct Network {
    lanes: Vec<Lane>,
    segments: Vec<Segment>,
    adjacent: Vec<Vec<(usize, f32)>>,
    grid: Vec<Vec<Vec<usize>>>,
}

/// Prepared for a fixed checkpoint. Build off the socket worker.
#[derive(Clone, Debug)]
pub struct Target {
    location: Location,
    nodes: Vec<f32>,
}

fn finite(v: f32) -> Result<f32, Error> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(Error::NonFinite)
    }
}
fn point(p: [f32; 3]) -> Result<(), Error> {
    for v in p {
        if finite(v)?.abs() > 1_000_000. {
            return Err(Error::Bound);
        }
    }
    Ok(())
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}

impl Spline {
    fn validate(&self) -> Result<(), Error> {
        if !(2..=MAX_LANE_POINTS).contains(&self.points.len())
            || self.end_times.len() + 1 != self.points.len()
            || self.end_distances.len() != self.end_times.len()
        {
            return Err(Error::Bound);
        }
        for p in self
            .controls
            .iter()
            .chain(&self.coefficients)
            .chain(&self.points)
        {
            point(*p)?;
        }
        let (mut last_t, mut last_d) = (0., 0.);
        for (&t, &d) in self.end_times.iter().zip(&self.end_distances) {
            if finite(t)? < last_t || t > 1. || finite(d)? < last_d || d > 1_000_000. {
                return Err(Error::Shape);
            }
            (last_t, last_d) = (t, d);
        }
        if last_t != 1. || last_d <= 0. {
            return Err(Error::Shape);
        }
        Ok(())
    }
    fn length(&self) -> f32 {
        *self.end_distances.last().expect("validated spline")
    }

    fn upper_bound(&self, p: [f32; 3]) -> f32 {
        self.controls
            .iter()
            .map(|&c| {
                let d = sub(p, c);
                dot(d, d)
            })
            .fold(0., f32::max)
            .sqrt()
            + 0.001
    }

    /// Native broad-phase candidate bound. It first compares the controls'
    /// horizontal angles, then the two edges adjacent to the nearest control
    /// point. The returned distance is three-dimensional and unweighted.
    fn lower_bound(&self, p: [f32; 3]) -> f32 {
        let delta = self.controls.map(|c| sub(c, p));
        let horizontal = delta.map(|d| (d[0] * d[0] + d[2] * d[2]).sqrt());
        if horizontal.iter().any(|&length| length < 0.000_001) {
            return 0.;
        }
        let inverse = 1. / horizontal[0];
        let base = [delta[0][0] * inverse, delta[0][2] * inverse];
        let mut angles = [0.; 4];
        for i in 1..4 {
            let inverse = 1. / horizontal[i];
            let cosine = ((delta[i][0] * inverse) * base[0] + (delta[i][2] * inverse) * base[1])
                .clamp(-1., 1.);
            let angle = cosine.acos();
            angles[i] = if delta[i][2] < 0. {
                std::f32::consts::TAU - angle
            } else {
                angle
            };
        }
        let min = angles.into_iter().fold(f32::INFINITY, f32::min);
        let max = angles.into_iter().fold(f32::NEG_INFINITY, f32::max);
        if max - min >= std::f32::consts::PI {
            return 0.;
        }
        let distances = delta.map(|d| dot(d, d));
        let first = if distances[0] < distances[1] { 0 } else { 1 };
        let second = if distances[2] < distances[3] { 2 } else { 3 };
        let center = if distances[first] < distances[second] {
            first
        } else {
            second
        };
        let edge_distance = |a: [f32; 3], b: [f32; 3]| {
            let difference = sub(b, a);
            let length = dot(difference, difference).sqrt();
            let t = if length < 0.000_001 {
                0.
            } else {
                let unit = difference.map(|v| v * (1. / length));
                (dot(unit, sub(p, a)) / length).clamp(0., 1.)
            };
            let q = std::array::from_fn(|axis| (1. - t) * a[axis] + t * b[axis]);
            let d = sub(q, p);
            dot(d, d)
        };
        edge_distance(self.controls[(center + 3) % 4], self.controls[center])
            .min(edge_distance(
                self.controls[center],
                self.controls[(center + 1) % 4],
            ))
            .sqrt()
            - 0.001
    }

    fn parameter(&self, distance: f32) -> f32 {
        let first = self.end_distances[0];
        if distance <= first {
            return if first < 0.000_001 {
                self.end_times[0]
            } else {
                (distance / first * self.end_times[0]).clamp(0., 1.)
            };
        }
        let i = (1..self.end_distances.len())
            .find(|&i| distance <= self.end_distances[i] + 0.001)
            .unwrap_or(self.end_distances.len() - 1);
        // With only one interval, a projected offset cannot exceed its length.
        if i == 0 {
            return self.end_times[0];
        }
        let width = self.end_distances[i] - self.end_distances[i - 1];
        if width < 0.000_001 {
            return self.end_times[i - 1];
        }
        ((distance - self.end_distances[i - 1]) / width
            * (self.end_times[i] - self.end_times[i - 1])
            + self.end_times[i - 1])
            .clamp(0., 1.)
    }

    fn at(&self, distance: f32) -> [f32; 3] {
        let t = self.parameter(distance);
        let t2 = t * t;
        let t3 = t2 * t;
        std::array::from_fn(|axis| {
            ((self.coefficients[1][axis] * t + self.coefficients[0][axis])
                + self.coefficients[2][axis] * t2)
                + self.coefficients[3][axis] * t3
        })
    }

    /// Native projection divides by the authored interval length twice. This
    /// intentionally differs from projection divided by the chord length².
    fn project(&self, p: [f32; 3]) -> f32 {
        let (mut best, mut offset, mut start) = (f32::MAX, 0., 0.);
        for (i, pair) in self.points.windows(2).enumerate() {
            let length = (self.end_distances[i] - start).max(0.001);
            let along = (dot(sub(pair[1], pair[0]), sub(p, pair[0])) / length).clamp(0., length);
            let t = along / length;
            let q = std::array::from_fn(|axis| (1. - t) * pair[0][axis] + t * pair[1][axis]);
            let d = sub(q, p);
            let squared = dot(d, d);
            if squared < best {
                best = squared;
                offset = along + start;
            }
            start = self.end_distances[i];
        }
        // Degenerate intervals use an epsilon during projection; keep the
        // exposed location within the actual authored segment.
        offset.min(self.length())
    }
}

#[derive(Clone, Copy, Debug)]
struct Visit(usize, f32);
impl PartialEq for Visit {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1.to_bits() == other.1.to_bits()
    }
}
impl Eq for Visit {}
impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Visit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .1
            .total_cmp(&self.1)
            .then_with(|| other.0.cmp(&self.0))
    }
}

impl Network {
    pub fn new(
        nodes: usize,
        lanes: Vec<Lane>,
        splines: Vec<Spline>,
        grid: Vec<Vec<Vec<usize>>>,
    ) -> Result<Self, Error> {
        if !(1..=MAX_NODES).contains(&nodes)
            || !(1..=MAX_LANES).contains(&lanes.len())
            || !(1..=MAX_SEGMENTS).contains(&splines.len())
            || grid.len() != GRID_SIZE
        {
            return Err(Error::Bound);
        }
        let mut refs = 0usize;
        for column in &grid {
            if column.len() != GRID_SIZE {
                return Err(Error::Shape);
            }
            for cell in column {
                refs += cell.len();
                if cell.len() > MAX_CELL_SEGMENTS || refs > MAX_GRID_REFERENCES {
                    return Err(Error::Bound);
                }
                if cell.iter().any(|&i| i >= splines.len()) {
                    return Err(Error::Shape);
                }
            }
        }
        for spline in &splines {
            spline.validate()?;
        }
        let mut segments: Vec<_> = splines
            .into_iter()
            .map(|spline| Segment {
                spline,
                lane: usize::MAX,
                before: 0.,
                after: 0.,
            })
            .collect();
        let mut adjacent = vec![Vec::new(); nodes];
        for (lane_index, lane) in lanes.iter().enumerate() {
            if lane.start >= nodes || lane.end >= nodes || lane.segments.is_empty() {
                return Err(Error::Shape);
            }
            if lane.segments.len() > MAX_LANE_SEGMENTS {
                return Err(Error::Bound);
            }
            let mut total = 0.;
            for &id in &lane.segments {
                let segment = segments.get_mut(id).ok_or(Error::Shape)?;
                if segment.lane != usize::MAX {
                    return Err(Error::Shape);
                }
                segment.lane = lane_index;
                total = finite(total + segment.spline.length())?;
            }
            for (i, &id) in lane.segments.iter().enumerate() {
                let mut before = 0.;
                for &previous in lane.segments[..i].iter().rev() {
                    before = finite(before + segments[previous].spline.length())?;
                }
                let mut after = 0.;
                for &next in &lane.segments[i + 1..] {
                    after = finite(after + segments[next].spline.length())?;
                }
                segments[id].before = before;
                segments[id].after = after;
            }
            if lane.layer != 0 {
                adjacent[lane.start].push((lane.end, total));
                adjacent[lane.end].push((lane.start, total));
            }
        }
        if segments.iter().any(|s| s.lane == usize::MAX) {
            return Err(Error::Shape);
        }
        Ok(Self {
            lanes,
            segments,
            adjacent,
            grid,
        })
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn locate(&self, position: [f32; 3]) -> Result<Option<Location>, Error> {
        point(position)?;
        let cell = |v: f32| ((v as i32 + 10_000) / 100).clamp(0, 199) as usize;
        let (x, z) = (cell(position[0]), cell(position[2]));
        let mut candidates = BTreeSet::new();
        for column in &self.grid[x.saturating_sub(1)..(x + 2).min(GRID_SIZE)] {
            for cell in &column[z.saturating_sub(1)..(z + 2).min(GRID_SIZE)] {
                candidates.extend(cell.iter().copied());
            }
        }
        let (mut best, mut location) = (f32::MAX, None);
        let upper = candidates
            .iter()
            .map(|&i| self.segments[i].spline.upper_bound(position))
            .fold(f32::MAX, f32::min);
        for index in candidates {
            let segment = &self.segments[index];
            if segment.spline.lower_bound(position) > upper || self.lanes[segment.lane].layer == 0 {
                continue;
            }
            let offset = segment.spline.project(position);
            let p = segment.spline.at(offset);
            let mut delta = sub(position, p);
            delta[1] *= 5.;
            let distance = dot(delta, delta).sqrt();
            if distance < best {
                best = distance;
                location = Some(Location {
                    segment: index,
                    offset,
                    point: p,
                });
            }
        }
        Ok(location)
    }

    fn endpoints(&self, location: Location) -> Result<[(usize, f32); 2], Error> {
        let segment = self.segments.get(location.segment).ok_or(Error::Shape)?;
        if !(0.0..=segment.spline.length()).contains(&finite(location.offset)?) {
            return Err(Error::Bound);
        }
        let lane = &self.lanes[segment.lane];
        Ok([
            (lane.start, finite(location.offset + segment.before)?),
            (
                lane.end,
                finite((segment.spline.length() - location.offset) + segment.after)?,
            ),
        ])
    }

    pub fn prepare_target(&self, location: Location) -> Result<Target, Error> {
        let mut nodes = vec![f32::INFINITY; self.adjacent.len()];
        let mut pending = BinaryHeap::new();
        for (node, distance) in self.endpoints(location)? {
            if distance < nodes[node] {
                nodes[node] = distance;
                pending.push(Visit(node, distance));
            }
        }
        while let Some(Visit(node, distance)) = pending.pop() {
            if distance != nodes[node] {
                continue;
            }
            for &(next, length) in &self.adjacent[node] {
                let candidate = finite(distance + length)?;
                if candidate < nodes[next] {
                    nodes[next] = candidate;
                    pending.push(Visit(next, candidate));
                }
            }
        }
        Ok(Target { location, nodes })
    }

    pub fn distance(&self, from: Location, target: &Target) -> Result<Option<f32>, Error> {
        let endpoints = self.endpoints(from)?;
        self.endpoints(target.location)?;
        if target.nodes.len() != self.adjacent.len() {
            return Err(Error::Shape);
        }
        if from.segment == target.location.segment {
            return Ok(Some((from.offset - target.location.offset).abs()));
        }
        let a = &self.segments[from.segment];
        let b = &self.segments[target.location.segment];
        let mut best = f32::INFINITY;
        if a.lane == b.lane {
            best = ((a.before + from.offset) - (b.before + target.location.offset)).abs();
        }
        for (node, distance) in endpoints {
            best = best.min(distance + target.nodes[node]);
        }
        Ok(best.is_finite().then_some(best))
    }
}

#[cfg(test)]
mod tests;
