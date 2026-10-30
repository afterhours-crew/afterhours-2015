// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned road measurements for the road-driving query, taken from the
//! installed road network (E916: `RoadNetworkEntityData` with 8,652 lane
//! segments, each a sampled spline with an integer speed limit in mph, a
//! lane width and a lane direction). The caller converts installed content
//! into [`Segment`]s; [`Network::sample`] projects an owned car onto the
//! nearest segment and answers the measurements the query contract (E904)
//! needs: the signed lateral distance to the segment, the segment's speed
//! limit and an owned wrong-direction classification.
//!
//! Classification (inferred, calibrated against the official E914 field
//! timeline): a lane with an authored direction is driven the wrong way when
//! the normalized velocity points against the lane tangent beyond the native
//! -0.4 cosine threshold; lanes authored as `Any` direction are never wrong
//! way. The native query reads these from its road manager; here they are
//! server measurements, never a client assertion.
use super::{Error, finite, road::Sample};

pub const MAX_SEGMENTS: usize = 65_536;
pub const MAX_POINTS: usize = 64;
/// Segments farther than this (3D distance to the projected point) are not
/// considered the car's road; the query's own `MaxDistToSegment` then
/// decides on-road state from the lateral distance.
pub const SEARCH_RADIUS: f32 = 40.0;
/// Uniform spatial grid cell edge in metres (segments are at most ~60 m).
const CELL: f32 = 64.0;
/// Native wrong-direction cosine threshold (E904: constant -0.4f).
pub const WRONG_WAY_COSINE: f32 = -0.4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Any,
    Forward,
    Backward,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// Installed integer speed limit in mph.
    pub speed_limit_mph: u16,
    pub width: f32,
    pub direction: Direction,
    /// Sampled spline points in order (at least two, finite).
    pub points: Vec<[f32; 3]>,
}

impl Segment {
    fn validate(&self) -> Result<(), Error> {
        if self.points.len() < 2 || self.points.len() > MAX_POINTS {
            return Err(Error::Bound);
        }
        for point in &self.points {
            for c in point {
                finite(*c)?;
            }
        }
        if !(self.width.is_finite() && self.width > 0.0) {
            return Err(Error::NonFinite);
        }
        Ok(())
    }
}

/// The nearest segment's measurements for one position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub segment: usize,
    /// Closest point on the segment polyline.
    pub point: [f32; 3],
    /// Unit tangent of the polyline piece holding the closest point (XZ).
    pub tangent: [f32; 2],
    /// Signed lateral distance in the XZ plane: positive to the right of the
    /// tangent (Y up, right-handed), negative to the left.
    pub lateral: f32,
    /// 3D distance from the position to the closest point.
    pub distance: f32,
}

#[derive(Clone, Debug)]
pub struct Network {
    segments: Vec<Segment>,
    origin: [f32; 2],
    columns: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
}

fn cell_of(
    origin: [f32; 2],
    columns: usize,
    rows: usize,
    x: f32,
    z: f32,
) -> Option<(usize, usize)> {
    let cx = ((x - origin[0]) / CELL).floor();
    let cz = ((z - origin[1]) / CELL).floor();
    if cx < 0.0 || cz < 0.0 || cx >= columns as f32 || cz >= rows as f32 {
        return None;
    }
    Some((cx as usize, cz as usize))
}

impl Network {
    pub fn new(segments: Vec<Segment>) -> Result<Self, Error> {
        if segments.is_empty() || segments.len() > MAX_SEGMENTS {
            return Err(Error::Bound);
        }
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for segment in &segments {
            segment.validate()?;
            for p in &segment.points {
                min[0] = min[0].min(p[0]);
                min[1] = min[1].min(p[2]);
                max[0] = max[0].max(p[0]);
                max[1] = max[1].max(p[2]);
            }
        }
        let origin = [min[0] - CELL, min[1] - CELL];
        let columns = (((max[0] - origin[0]) / CELL).floor() as usize)
            .checked_add(2)
            .ok_or(Error::Bound)?;
        let rows = (((max[1] - origin[1]) / CELL).floor() as usize)
            .checked_add(2)
            .ok_or(Error::Bound)?;
        if columns.saturating_mul(rows) > 1 << 22 {
            return Err(Error::Bound);
        }
        let mut cells = vec![Vec::new(); columns * rows];
        for (index, segment) in segments.iter().enumerate() {
            let mut seen = std::collections::BTreeSet::new();
            for pair in segment.points.windows(2) {
                // Every cell touched by the piece's bounding box (pieces are short).
                let (ax, az) = (pair[0][0].min(pair[1][0]), pair[0][2].min(pair[1][2]));
                let (bx, bz) = (pair[0][0].max(pair[1][0]), pair[0][2].max(pair[1][2]));
                let Some((x0, z0)) = cell_of(origin, columns, rows, ax, az) else {
                    return Err(Error::Bound);
                };
                let Some((x1, z1)) = cell_of(origin, columns, rows, bx, bz) else {
                    return Err(Error::Bound);
                };
                for x in x0..=x1 {
                    for z in z0..=z1 {
                        if seen.insert((x, z)) {
                            cells[x + z * columns].push(index as u32);
                        }
                    }
                }
            }
        }
        Ok(Self {
            segments,
            origin,
            columns,
            rows,
            cells,
        })
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Closest point on a segment to `position`, with the piece tangent.
    fn project(segment: &Segment, position: [f32; 3]) -> Option<Projection> {
        let mut best: Option<Projection> = None;
        for pair in segment.points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if len2 <= f32::EPSILON {
                continue;
            }
            let w = [position[0] - a[0], position[1] - a[1], position[2] - a[2]];
            let t = ((w[0] * d[0] + w[1] * d[1] + w[2] * d[2]) / len2).clamp(0.0, 1.0);
            let point = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
            let e = [
                position[0] - point[0],
                position[1] - point[1],
                position[2] - point[2],
            ];
            let distance = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
            if best.is_some_and(|b| b.distance <= distance) {
                continue;
            }
            let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
            if flat <= f32::EPSILON {
                continue;
            }
            let tangent = [d[0] / flat, d[2] / flat];
            // Right of the tangent in the XZ plane with Y up: cross(up, tangent).
            let lateral = e[0] * tangent[1] - e[2] * tangent[0];
            best = Some(Projection {
                segment: 0,
                point,
                tangent,
                lateral,
                distance,
            });
        }
        best
    }

    /// The nearest segment within [`SEARCH_RADIUS`] of the position.
    pub fn nearest(&self, position: [f32; 3]) -> Option<Projection> {
        let (cx, cz) = cell_of(
            self.origin,
            self.columns,
            self.rows,
            position[0],
            position[2],
        )?;
        let reach = (SEARCH_RADIUS / CELL).ceil() as usize;
        let mut best: Option<Projection> = None;
        for x in cx.saturating_sub(reach)..=(cx + reach).min(self.columns - 1) {
            for z in cz.saturating_sub(reach)..=(cz + reach).min(self.rows - 1) {
                for &index in &self.cells[x + z * self.columns] {
                    let segment = &self.segments[index as usize];
                    let Some(mut projection) = Self::project(segment, position) else {
                        continue;
                    };
                    projection.segment = index as usize;
                    if projection.distance <= SEARCH_RADIUS
                        && best.is_none_or(|b| projection.distance < b.distance)
                    {
                        best = Some(projection);
                    }
                }
            }
        }
        best
    }

    /// Road-driving query sample for an owned car, or None off the network.
    pub fn sample(&self, position: [f32; 3], velocity: [f32; 3]) -> Result<Option<Sample>, Error> {
        for c in position.iter().chain(&velocity) {
            finite(*c)?;
        }
        let Some(projection) = self.nearest(position) else {
            return Ok(None);
        };
        let segment = &self.segments[projection.segment];
        let speed = finite(
            (velocity[0] * velocity[0] + velocity[1] * velocity[1] + velocity[2] * velocity[2])
                .sqrt(),
        )?;
        let wrong_direction = match segment.direction {
            Direction::Any => false,
            Direction::Forward | Direction::Backward => {
                let flat = (velocity[0] * velocity[0] + velocity[2] * velocity[2]).sqrt();
                if flat <= f32::EPSILON {
                    false
                } else {
                    let along = (velocity[0] * projection.tangent[0]
                        + velocity[2] * projection.tangent[1])
                        / flat;
                    let along = if segment.direction == Direction::Backward {
                        -along
                    } else {
                        along
                    };
                    along < WRONG_WAY_COSINE
                }
            }
        };
        Ok(Some(Sample {
            speed_limit: f32::from(segment.speed_limit_mph),
            speed_limit_override: None,
            distance_to_segment: finite(projection.lateral)?,
            speed_metres_per_second: speed,
            wrong_direction,
        }))
    }
}

#[cfg(test)]
mod tests;
