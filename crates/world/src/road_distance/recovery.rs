// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::crash::{Destination, route::Geometry};

#[derive(Clone, Copy)]
struct Piece {
    segment: usize,
    from: f32,
    to: f32,
    forward: bool,
}

impl Spline {
    fn tangent(&self, offset: f32, forward: bool) -> Option<[f32; 3]> {
        let t = self.parameter(offset);
        let v = std::array::from_fn(|i| {
            self.coefficients[1][i]
                + 2. * t * self.coefficients[2][i]
                + 3. * t * t * self.coefficients[3][i]
        });
        let length = dot(v, v).sqrt();
        if !length.is_finite() || length < 0.000_001 {
            return None;
        }
        let scale = if forward { 1. / length } else { -1. / length };
        Some(v.map(|n| n * scale))
    }
}

impl Network {
    fn lane_pieces(
        &self,
        pieces: &mut Vec<Piece>,
        lane: usize,
        from: Option<Location>,
        to: Option<Location>,
        forward: bool,
    ) -> Result<(), Error> {
        let ids = &self.lanes.get(lane).ok_or(Error::Shape)?.segments;
        let index = |p: Location| {
            ids.iter()
                .position(|&id| id == p.segment)
                .ok_or(Error::Shape)
        };
        let start = from
            .map(index)
            .transpose()?
            .unwrap_or(if forward { 0 } else { ids.len() - 1 });
        let end = to
            .map(index)
            .transpose()?
            .unwrap_or(if forward { ids.len() - 1 } else { 0 });
        if (forward && start > end) || (!forward && start < end) {
            return Err(Error::Shape);
        }
        for n in 0..=start.abs_diff(end) {
            if pieces.len() >= MAX_SEGMENTS {
                return Err(Error::Bound);
            }
            let i = if forward { start + n } else { start - n };
            let length = self.segments[ids[i]].spline.length();
            pieces.push(Piece {
                segment: ids[i],
                forward,
                from: if i == start {
                    from.map(|p| p.offset)
                } else {
                    None
                }
                .unwrap_or(if forward { 0. } else { length }),
                to: if i == end { to.map(|p| p.offset) } else { None }.unwrap_or(if forward {
                    length
                } else {
                    0.
                }),
            });
        }
        Ok(())
    }

    /// Project onto the route toward the next checkpoint, independently of the
    /// vehicle's heading. The path follows precomputed successors, with no
    /// per-movement graph search. All geometry remains bounded by the network.
    pub fn recovery(
        &self,
        from: Location,
        target: &Target,
        sharp_degrees: f32,
    ) -> Result<Option<Geometry>, Error> {
        if !(0.0..=180.).contains(&finite(sharp_degrees)?) {
            return Err(Error::Bound);
        }
        let Some(distance) = self.distance(from, target)? else {
            return Ok(None);
        };
        let a = &self.segments[from.segment];
        let b = &self.segments[target.location.segment];
        let endpoints = self.endpoints(from)?;
        let best = endpoints
            .into_iter()
            .min_by(|(a, da), (b, db)| (da + target.nodes[*a]).total_cmp(&(db + target.nodes[*b])))
            .ok_or(Error::Shape)?;
        let direct = a.lane == b.lane
            && (from.segment == target.location.segment
                || ((a.before + from.offset) - (b.before + target.location.offset)).abs()
                    <= best.1 + target.nodes[best.0]);
        let mut pieces = Vec::new();
        if direct {
            let forward = a.before + from.offset <= b.before + target.location.offset;
            self.lane_pieces(
                &mut pieces,
                a.lane,
                Some(from),
                Some(target.location),
                forward,
            )?;
        } else {
            let mut node = best.0;
            self.lane_pieces(
                &mut pieces,
                a.lane,
                Some(from),
                None,
                node == self.lanes[a.lane].end,
            )?;
            let mut count = 0;
            while let Some((lane, next)) = target.steps[node] {
                count += 1;
                if count > self.adjacent.len() || target.nodes[next] >= target.nodes[node] {
                    return Err(Error::Shape);
                }
                self.lane_pieces(
                    &mut pieces,
                    lane,
                    None,
                    None,
                    node == self.lanes[lane].start,
                )?;
                node = next;
            }
            let lane = &self.lanes[b.lane];
            if node != lane.start && node != lane.end {
                return Err(Error::Shape);
            }
            self.lane_pieces(
                &mut pieces,
                b.lane,
                None,
                Some(target.location),
                node == lane.start,
            )?;
        }
        let first = pieces.first().ok_or(Error::Shape)?;
        let spline = &self.segments[first.segment].spline;
        let Some(forward) = spline.tangent(first.from, first.forward) else {
            return Ok(None);
        };
        let right = [forward[2], 0., -forward[0]];
        if dot(right, right) < 0.000_001 {
            return Ok(None);
        }
        let threshold = (sharp_degrees * std::f32::consts::PI * (1. / 180.)).cos();
        let mut travelled = 0.;
        let mut corner = None;
        for pair in pieces.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            travelled += (a.to - a.from).abs();
            let Some(end) = self.segments[a.segment].spline.tangent(a.to, a.forward) else {
                return Ok(None);
            };
            let Some(start) = self.segments[b.segment].spline.tangent(b.from, b.forward) else {
                return Ok(None);
            };
            if dot(end, start) < threshold {
                corner = Some(travelled);
                break;
            }
        }
        Ok(Some(Geometry {
            destination: Destination {
                position: spline.at(first.from),
                basis: [right, [0., 1., 0.], forward],
            },
            distance,
            corner,
        }))
    }
}
