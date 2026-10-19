// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Server-side wandering on the road graph, ported from the native server's
//! wander unit (E987). While no client drives a rival, the host moves it
//! kinematically along the route the native planner would choose:
//!
//! * Target (`0464b310`): a road point 300 + U(0, 100) m ahead, accepted when
//!   more than 10 m away; otherwise the same behind the car; otherwise, for
//!   goals with the third flag, a point offset by ±U(100, 200) m in x and z,
//!   snapped to the road.
//! * Junctions (`0464bf90`, shared with client `03f884e0`): the exit most
//!   aligned with the arriving direction, or, unless the goal prefers going
//!   straight, with probability 25/101 a uniformly random exit whose direction
//!   dot product is above -0.5.
//! * Random numbers come from a port of the native generator `033ff940` /
//!   `03400140`; the host injects its seed.
//! * Speed is the native non-urgent wander speed, 22.352 m/s (`0464ae90`).
use crate::road_graph::{Edge, Error, Graph, Pose};
use std::collections::VecDeque;

pub const WANDER_SPEED: f32 = 22.352;
/// Accepted targets are farther than this from the car (native 10 m, squared 100).
pub const MIN_TARGET_DISTANCE: f32 = 10.0;
const MAX_ROUTE: usize = 512;
/// A single advance never moves farther than this many planned routes.
const MAX_PLANS_PER_ADVANCE: usize = 8;

/// Port of the native multiplicative generator (two Schrage-style steps
/// modulo 2^31 - 1 with multipliers 0x5e30 and 0x661f).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRandom {
    state: i32,
}

fn native_step(x: i32, multiplier: i32) -> i32 {
    let product_low = (x & 0xffff).wrapping_mul(multiplier);
    let middle = (x >> 16)
        .wrapping_mul(multiplier)
        .wrapping_add(product_low >> 16);
    let mut value = ((middle & 0x7fff) << 16).wrapping_add(middle >> 15);
    value = value
        .wrapping_add(0x8000_0001u32 as i32)
        .wrapping_add(product_low & 0xffff);
    if value < 0 {
        value.wrapping_add(0x7fff_ffff)
    } else {
        value
    }
}

impl NativeRandom {
    pub fn new(seed: u32) -> Self {
        Self { state: seed as i32 }
    }

    fn next(&mut self) -> i32 {
        let x = if self.state == 0 { 1 } else { self.state };
        self.state = native_step(native_step(x, 0x5e30), 0x661f);
        self.state
    }

    /// Integer in `low..=high` (native `033ff940`).
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        let span = high.wrapping_sub(low).wrapping_add(1);
        let value = self.next();
        if span == 0 {
            return low;
        }
        low.wrapping_add(value.wrapping_rem(span))
    }

    /// Value in (0, 1] (native `03400140`).
    pub fn unit(&mut self) -> f32 {
        let value = ((self.next() | 0x80) >> 7).wrapping_add(1);
        value as f32 * 5.960_464_5e-8
    }

    /// Value between `low` and `high` (native `034000f0`).
    pub fn uniform(&mut self, low: f32, high: f32) -> f32 {
        self.unit() * (high - low) + low
    }
}

/// The three native WanderGoal flags (urgent, prefer straight, random offset).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flags {
    pub prefer_straight: bool,
    pub random_offset: bool,
}

impl Flags {
    pub fn from_goal(flags: [bool; 3]) -> Self {
        Self {
            prefer_straight: flags[1],
            random_offset: flags[2],
        }
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Native junction choice for a car arriving at `node` heading `heading`.
pub fn choose_exit(
    graph: &Graph,
    node: usize,
    heading: [f32; 3],
    prefer_straight: bool,
    random: &mut NativeRandom,
) -> Result<Option<Edge>, Error> {
    let mut best: Option<(Edge, f32)> = None;
    let mut ahead = Vec::new();
    for (edge, tangent) in graph.exits(node)? {
        let alignment = dot(heading, tangent);
        if best.is_none_or(|(_, b)| alignment > b) {
            best = Some((edge, alignment));
        }
        if alignment > -0.5 {
            ahead.push(edge);
        }
    }
    let Some((straightest, _)) = best else {
        return Ok(None);
    };
    if !prefer_straight && random.range(0, 100) < 25 && !ahead.is_empty() {
        let pick = random.range(0, ahead.len() as i32 - 1);
        return Ok(Some(ahead[pick.clamp(0, ahead.len() as i32 - 1) as usize]));
    }
    Ok(Some(straightest))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wander {
    edge: Edge,
    travelled: f32,
    /// Edges after the current one; the route ends `stop` metres into the
    /// last edge (or into the current edge when no edges follow).
    route: VecDeque<Edge>,
    stop: f32,
    flags: Flags,
    random: NativeRandom,
}

impl Wander {
    /// Start on the road nearest to `position`, travelling the allowed lane
    /// direction closest to `heading`. None when no road is near.
    pub fn start(
        graph: &Graph,
        position: [f32; 3],
        heading: [f32; 3],
        flags: Flags,
        random: NativeRandom,
    ) -> Result<Option<Self>, Error> {
        let Some(location) = graph.nearest(position)? else {
            return Ok(None);
        };
        let length = graph.length(location.lane)?;
        let mut choice: Option<(Edge, f32, f32)> = None;
        for forward in [true, false] {
            let edge = Edge {
                lane: location.lane,
                forward,
            };
            if !graph.allowed(edge)? {
                continue;
            }
            let travelled = if forward {
                location.offset
            } else {
                length - location.offset
            };
            let alignment = dot(heading, graph.pose(edge, travelled)?.tangent);
            if choice.is_none_or(|(_, _, a)| alignment > a) {
                choice = Some((edge, travelled, alignment));
            }
        }
        let Some((edge, travelled, _)) = choice else {
            return Ok(None);
        };
        let mut wander = Self {
            edge,
            travelled,
            route: VecDeque::new(),
            stop: travelled,
            flags,
            random,
        };
        wander.plan(graph)?;
        Ok(Some(wander))
    }

    pub fn pose(&self, graph: &Graph) -> Result<Pose, Error> {
        graph.pose(self.edge, self.travelled)
    }

    pub fn edge(&self) -> Edge {
        self.edge
    }

    /// Where the current route ends.
    pub fn target(&self, graph: &Graph) -> Result<Pose, Error> {
        graph.pose(*self.route.back().unwrap_or(&self.edge), self.stop)
    }

    /// Walk `distance` metres ahead of (edge, travelled) choosing native exits.
    fn walk(
        &mut self,
        graph: &Graph,
        edge: Edge,
        travelled: f32,
        distance: f32,
    ) -> Result<(VecDeque<Edge>, f32), Error> {
        let mut route = VecDeque::new();
        let mut current = edge;
        let mut remaining = distance;
        let mut available = graph.length(current.lane)? - travelled;
        let mut at = travelled;
        loop {
            if available >= remaining {
                return Ok((route, at + remaining));
            }
            remaining -= available.max(0.0);
            let node = graph.head(current)?;
            let heading = graph.pose(current, graph.length(current.lane)?)?.tangent;
            let Some(next) = choose_exit(
                graph,
                node,
                heading,
                self.flags.prefer_straight,
                &mut self.random,
            )?
            else {
                return Ok((route, graph.length(current.lane)?));
            };
            if route.len() >= MAX_ROUTE {
                return Ok((route, graph.length(current.lane)?));
            }
            route.push_back(next);
            current = next;
            at = 0.0;
            available = graph.length(current.lane)?;
        }
    }

    /// Whether a route from the car's position ends more than the native minimum away.
    fn far_enough(
        &self,
        graph: &Graph,
        base: Edge,
        route: &VecDeque<Edge>,
        stop: f32,
    ) -> Result<bool, Error> {
        let here = graph.pose(self.edge, self.travelled)?.position;
        let there = graph.pose(*route.back().unwrap_or(&base), stop)?.position;
        let d = [there[0] - here[0], there[1] - here[1], there[2] - here[2]];
        Ok(dot(d, d) > MIN_TARGET_DISTANCE * MIN_TARGET_DISTANCE)
    }

    /// Choose the next route from the current position (native `0464b310`).
    fn plan(&mut self, graph: &Graph) -> Result<bool, Error> {
        let distance = self.random.uniform(0.0, 100.0) + 300.0;
        let (route, stop) = self.walk(graph, self.edge, self.travelled, distance)?;
        if self.far_enough(graph, self.edge, &route, stop)? {
            self.route = route;
            self.stop = stop;
            return Ok(true);
        }
        let reverse = Edge {
            lane: self.edge.lane,
            forward: !self.edge.forward,
        };
        if graph.allowed(reverse)? {
            let travelled = graph.length(self.edge.lane)? - self.travelled;
            let distance = self.random.uniform(0.0, 100.0) + 300.0;
            let (route, stop) = self.walk(graph, reverse, travelled, distance)?;
            if self.far_enough(graph, reverse, &route, stop)? {
                self.edge = reverse;
                self.travelled = travelled;
                self.route = route;
                self.stop = stop;
                return Ok(true);
            }
        }
        if !self.flags.random_offset {
            return Ok(false);
        }
        let here = graph.pose(self.edge, self.travelled)?.position;
        let x_sign = if self.random.range(0, 1) == 0 {
            -1.0
        } else {
            1.0
        };
        let x = here[0] + self.random.uniform(100.0, 200.0) * x_sign;
        let z_sign = if self.random.range(0, 1) == 0 {
            -1.0
        } else {
            1.0
        };
        let z = here[2] + self.random.uniform(100.0, 200.0) * z_sign;
        let Some(location) = graph.nearest([x, here[1], z])? else {
            return Ok(false);
        };
        // Drive to the target lane's nearer allowed end, then along it.
        let from = graph.head(self.edge)?;
        let length = graph.length(location.lane)?;
        for forward in [true, false] {
            let edge = Edge {
                lane: location.lane,
                forward,
            };
            if !graph.allowed(edge)? {
                continue;
            }
            let Some(mut route) = graph.shortest_path(from, graph.tail(edge)?)? else {
                continue;
            };
            if route.len() >= MAX_ROUTE {
                continue;
            }
            route.push(edge);
            let stop = if forward {
                location.offset
            } else {
                length - location.offset
            };
            let route: VecDeque<_> = route.into_iter().collect();
            if self.far_enough(graph, self.edge, &route, stop)? {
                self.route = route;
                self.stop = stop;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Move `seconds` along the route at the wander speed, planning the next
    /// route at each target. A car with no reachable target stays put.
    pub fn advance(&mut self, graph: &Graph, seconds: f32) -> Result<(), Error> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(Error::NonFinite);
        }
        let mut distance = WANDER_SPEED * seconds;
        let mut plans = 0;
        while distance > 0.0 {
            let end = if self.route.is_empty() {
                self.stop
            } else {
                graph.length(self.edge.lane)?
            };
            let left = (end - self.travelled).max(0.0);
            if distance < left {
                self.travelled += distance;
                return Ok(());
            }
            distance -= left;
            self.travelled = end;
            if let Some(next) = self.route.pop_front() {
                self.edge = next;
                self.travelled = 0.0;
                continue;
            }
            if plans >= MAX_PLANS_PER_ADVANCE || !self.plan(graph)? {
                return Ok(());
            }
            plans += 1;
        }
        Ok(())
    }

    /// Continue from a pose a client reported for the car (handover back to
    /// the host). Keeps the flags and generator; None when no road is near.
    pub fn relocate(
        &self,
        graph: &Graph,
        position: [f32; 3],
        heading: [f32; 3],
    ) -> Result<Option<Self>, Error> {
        Self::start(graph, position, heading, self.flags, self.random)
    }
}

#[cfg(test)]
mod tests;
