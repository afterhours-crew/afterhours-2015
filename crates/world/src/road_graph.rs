// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed road graph used for server-side AI movement (E987). The road
//! schematic's `RoadNode` junctions are joined by `Lane` edges; each edge is a
//! centre line made of its ordered `LaneSegment` samples, oriented from its
//! start node to its end node. Lanes authored `Forward` may only be driven
//! start to end, `Backward` only end to start and `Any` both ways.
//!
//! The graph answers what the native server's wander planner needs: the pose
//! at a distance along an edge, the exits at a junction with their leaving
//! tangents, the nearest road point and a shortest path between junctions.
pub use crate::scoring::road_network::Direction;

pub const MAX_NODES: usize = 8_192;
pub const MAX_LANES: usize = 16_384;
pub const MAX_LANE_POINTS: usize = 4_096;
pub const MAX_NODE_LANES: usize = 16;
/// Widest accepted road (metres); installed widths are 4.5–50 m.
pub const MAX_WIDTH: f32 = 500.0;
/// Nearest-road searches look no farther than this (metres, 3D).
pub const SEARCH_RADIUS: f32 = 128.0;
const CELL: f32 = 64.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Bound,
    NonFinite,
    Shape,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bound => "road graph bound exceeded",
            Self::NonFinite => "road graph value is not finite",
            Self::Shape => "road graph is inconsistent",
        })
    }
}
impl std::error::Error for Error {}

/// One installed lane edge before validation.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneInput {
    pub start: usize,
    pub end: usize,
    pub direction: Direction,
    /// Centre line from the start node towards the end node.
    pub points: Vec<[f32; 3]>,
    /// Road width (`LaneWidth`, the whole road; E988) of each interval between
    /// consecutive points.
    pub widths: Vec<f32>,
}

#[derive(Clone, Debug)]
struct Node {
    position: [f32; 3],
    lanes: Vec<u32>,
}

#[derive(Clone, Debug)]
struct Lane {
    start: u32,
    end: u32,
    direction: Direction,
    points: Vec<[f32; 3]>,
    widths: Vec<f32>,
    /// Cumulative 3D length at each point; the last entry is the lane length.
    cumulative: Vec<f32>,
}

/// A lane travelled in one direction: `forward` runs start node to end node.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Edge {
    pub lane: u32,
    pub forward: bool,
}

/// Centre-line position, unit 3D tangent in the direction of travel and the
/// road width there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    pub tangent: [f32; 3],
    pub width: f32,
}

/// Closest road point to a position: the lane, the distance from the lane's
/// start node and the 3D distance to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Location {
    pub lane: u32,
    pub offset: f32,
    pub point: [f32; 3],
    pub distance: f32,
}

#[derive(Clone, Debug)]
pub struct Graph {
    nodes: Vec<Node>,
    lanes: Vec<Lane>,
    origin: [f32; 2],
    columns: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
}

fn finite(v: f32) -> Result<f32, Error> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(Error::NonFinite)
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = length(v);
    (l > f32::EPSILON).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

impl Graph {
    pub fn new(nodes: Vec<[f32; 3]>, lanes: Vec<LaneInput>) -> Result<Self, Error> {
        if nodes.is_empty() || nodes.len() > MAX_NODES || lanes.is_empty() {
            return Err(Error::Bound);
        }
        if lanes.len() > MAX_LANES {
            return Err(Error::Bound);
        }
        let mut graph_nodes = Vec::with_capacity(nodes.len());
        for position in nodes {
            for c in position {
                finite(c)?;
            }
            graph_nodes.push(Node {
                position,
                lanes: Vec::new(),
            });
        }
        let mut graph_lanes = Vec::with_capacity(lanes.len());
        for (index, lane) in lanes.into_iter().enumerate() {
            if lane.start >= graph_nodes.len() || lane.end >= graph_nodes.len() {
                return Err(Error::Shape);
            }
            if lane.points.len() < 2 || lane.points.len() > MAX_LANE_POINTS {
                return Err(Error::Bound);
            }
            if lane.widths.len() + 1 != lane.points.len() {
                return Err(Error::Shape);
            }
            for &width in &lane.widths {
                if !(0.0..=MAX_WIDTH).contains(&finite(width)?) {
                    return Err(Error::Bound);
                }
            }
            let mut cumulative = Vec::with_capacity(lane.points.len());
            let mut total = 0.0f32;
            for (i, p) in lane.points.iter().enumerate() {
                for c in p {
                    finite(*c)?;
                }
                if i > 0 {
                    total += length(sub(*p, lane.points[i - 1]));
                }
                cumulative.push(finite(total)?);
            }
            if total <= f32::EPSILON {
                return Err(Error::Shape);
            }
            for node in [lane.start, lane.end] {
                let list = &mut graph_nodes[node].lanes;
                if !list.contains(&(index as u32)) {
                    if list.len() >= MAX_NODE_LANES {
                        return Err(Error::Bound);
                    }
                    list.push(index as u32);
                }
            }
            graph_lanes.push(Lane {
                start: lane.start as u32,
                end: lane.end as u32,
                direction: lane.direction,
                points: lane.points,
                widths: lane.widths,
                cumulative,
            });
        }
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for lane in &graph_lanes {
            for p in &lane.points {
                min[0] = min[0].min(p[0]);
                min[1] = min[1].min(p[2]);
                max[0] = max[0].max(p[0]);
                max[1] = max[1].max(p[2]);
            }
        }
        let origin = [min[0] - CELL, min[1] - CELL];
        let columns = ((max[0] - origin[0]) / CELL).floor() as usize + 2;
        let rows = ((max[1] - origin[1]) / CELL).floor() as usize + 2;
        if columns.saturating_mul(rows) > 1 << 22 {
            return Err(Error::Bound);
        }
        let mut cells = vec![Vec::new(); columns * rows];
        for (index, lane) in graph_lanes.iter().enumerate() {
            let mut seen = std::collections::BTreeSet::new();
            for pair in lane.points.windows(2) {
                let x0 = ((pair[0][0].min(pair[1][0]) - origin[0]) / CELL).floor() as usize;
                let x1 = ((pair[0][0].max(pair[1][0]) - origin[0]) / CELL).floor() as usize;
                let z0 = ((pair[0][2].min(pair[1][2]) - origin[1]) / CELL).floor() as usize;
                let z1 = ((pair[0][2].max(pair[1][2]) - origin[1]) / CELL).floor() as usize;
                for x in x0..=x1.min(columns - 1) {
                    for z in z0..=z1.min(rows - 1) {
                        if seen.insert((x, z)) {
                            cells[x + z * columns].push(index as u32);
                        }
                    }
                }
            }
        }
        Ok(Self {
            nodes: graph_nodes,
            lanes: graph_lanes,
            origin,
            columns,
            rows,
            cells,
        })
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    fn lane(&self, lane: u32) -> Result<&Lane, Error> {
        self.lanes.get(lane as usize).ok_or(Error::Shape)
    }

    pub fn length(&self, lane: u32) -> Result<f32, Error> {
        Ok(*self.lane(lane)?.cumulative.last().ok_or(Error::Shape)?)
    }

    /// Whether the lane's authored direction allows this travel direction.
    pub fn allowed(&self, edge: Edge) -> Result<bool, Error> {
        Ok(match self.lane(edge.lane)?.direction {
            Direction::Any => true,
            Direction::Forward => edge.forward,
            Direction::Backward => !edge.forward,
        })
    }

    /// The node an edge arrives at.
    pub fn head(&self, edge: Edge) -> Result<usize, Error> {
        let lane = self.lane(edge.lane)?;
        Ok(if edge.forward { lane.end } else { lane.start } as usize)
    }

    /// The node an edge leaves from.
    pub fn tail(&self, edge: Edge) -> Result<usize, Error> {
        let lane = self.lane(edge.lane)?;
        Ok(if edge.forward { lane.start } else { lane.end } as usize)
    }

    pub fn node_position(&self, node: usize) -> Result<[f32; 3], Error> {
        Ok(self.nodes.get(node).ok_or(Error::Shape)?.position)
    }

    /// Pose at `travelled` metres from the edge's tail, clamped to the lane.
    pub fn pose(&self, edge: Edge, travelled: f32) -> Result<Pose, Error> {
        let lane = self.lane(edge.lane)?;
        let total = *lane.cumulative.last().ok_or(Error::Shape)?;
        let along = finite(travelled)?.clamp(0.0, total);
        let offset = if edge.forward { along } else { total - along };
        let i = match lane
            .cumulative
            .binary_search_by(|c| c.partial_cmp(&offset).unwrap_or(std::cmp::Ordering::Less))
        {
            Ok(i) => i.min(lane.points.len() - 2),
            Err(i) => i.saturating_sub(1).min(lane.points.len() - 2),
        };
        let (a, b) = (lane.points[i], lane.points[i + 1]);
        let span = lane.cumulative[i + 1] - lane.cumulative[i];
        let t = if span > f32::EPSILON {
            ((offset - lane.cumulative[i]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let position = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ];
        let mut tangent = unit(sub(b, a)).unwrap_or([0.0, 0.0, 1.0]);
        if !edge.forward {
            tangent = [-tangent[0], -tangent[1], -tangent[2]];
        }
        Ok(Pose {
            position,
            tangent,
            width: lane.widths[i],
        })
    }

    /// Allowed edges leaving `node`, each with its leaving tangent. A lane
    /// whose both ends are the node yields both directions.
    pub fn exits(&self, node: usize) -> Result<Vec<(Edge, [f32; 3])>, Error> {
        let mut out = Vec::new();
        for &lane in &self.nodes.get(node).ok_or(Error::Shape)?.lanes {
            let data = self.lane(lane)?;
            for forward in [true, false] {
                let tail = if forward { data.start } else { data.end } as usize;
                let edge = Edge { lane, forward };
                if tail == node && self.allowed(edge)? {
                    out.push((edge, self.pose(edge, 0.0)?.tangent));
                }
            }
        }
        Ok(out)
    }

    /// Nearest lane point within [`SEARCH_RADIUS`]. Distance is measured in
    /// three dimensions so a car under or on a bridge snaps to its own level:
    /// official rival cars appear 0 to 0.5 m above the nearest centre line by
    /// this measure, while the horizontal-only nearest point picks another
    /// level for two of 24 (E988).
    pub fn nearest(&self, position: [f32; 3]) -> Result<Option<Location>, Error> {
        for c in position {
            finite(c)?;
        }
        let cx = ((position[0] - self.origin[0]) / CELL).floor();
        let cz = ((position[2] - self.origin[1]) / CELL).floor();
        if cx < 0.0 || cz < 0.0 || cx >= self.columns as f32 || cz >= self.rows as f32 {
            return Ok(None);
        }
        let (cx, cz) = (cx as usize, cz as usize);
        let reach = (SEARCH_RADIUS / CELL).ceil() as usize;
        let mut best: Option<Location> = None;
        for x in cx.saturating_sub(reach)..=(cx + reach).min(self.columns - 1) {
            for z in cz.saturating_sub(reach)..=(cz + reach).min(self.rows - 1) {
                for &lane in &self.cells[x + z * self.columns] {
                    let data = &self.lanes[lane as usize];
                    for (i, pair) in data.points.windows(2).enumerate() {
                        let (a, b) = (pair[0], pair[1]);
                        let d = sub(b, a);
                        let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                        let t = if len2 > f32::EPSILON {
                            let along = sub(position, a);
                            ((along[0] * d[0] + along[1] * d[1] + along[2] * d[2]) / len2)
                                .clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let point = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
                        let distance = length(sub(position, point));
                        if distance <= SEARCH_RADIUS && best.is_none_or(|b| distance < b.distance) {
                            let offset = data.cumulative[i]
                                + (data.cumulative[i + 1] - data.cumulative[i]) * t;
                            best = Some(Location {
                                lane,
                                offset,
                                point,
                                distance,
                            });
                        }
                    }
                }
            }
        }
        Ok(best)
    }

    /// Shortest allowed edge sequence from node `from` to node `to`, by lane
    /// length. Empty when `from == to`; None when unreachable.
    pub fn shortest_path(&self, from: usize, to: usize) -> Result<Option<Vec<Edge>>, Error> {
        if from >= self.nodes.len() || to >= self.nodes.len() {
            return Err(Error::Shape);
        }
        use std::{cmp::Reverse, collections::BinaryHeap};
        let mut dist = vec![f32::INFINITY; self.nodes.len()];
        let mut via: Vec<Option<Edge>> = vec![None; self.nodes.len()];
        dist[from] = 0.0;
        // Non-negative finite f32 values order like their bit patterns.
        let mut queue = BinaryHeap::from([Reverse((0u32, from))]);
        let mut reached = false;
        while let Some(Reverse((bits, node))) = queue.pop() {
            if f32::from_bits(bits) > dist[node] {
                continue;
            }
            if node == to {
                reached = true;
                break;
            }
            for (edge, _) in self.exits(node)? {
                let head = self.head(edge)?;
                let candidate = dist[node] + self.length(edge.lane)?;
                if candidate < dist[head] {
                    dist[head] = candidate;
                    via[head] = Some(edge);
                    queue.push(Reverse((candidate.to_bits(), head)));
                }
            }
        }
        if !reached {
            return Ok(None);
        }
        let mut path = Vec::new();
        let mut node = to;
        while node != from {
            let edge = via[node].ok_or(Error::Shape)?;
            path.push(edge);
            node = self.tail(edge)?;
            if path.len() > self.lanes.len() * 2 {
                return Err(Error::Shape);
            }
        }
        path.reverse();
        Ok(Some(path))
    }
}

#[cfg(test)]
pub(crate) mod tests;
