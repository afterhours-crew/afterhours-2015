// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored free-roam rival activation geometry. This is independent of ambient
//! traffic's road sampling and distance policy.
use crate::Error;

pub(crate) mod extract;
mod json;
pub mod preset;
pub use extract::extract;
pub(crate) use json::build;

pub const MAX_RACERS: usize = 128;
pub const MAX_REGIONS: usize = 64;
pub const MAX_PERSONALITIES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Links {
    pub racers: u32,
    pub provider: u32,
    pub personas: u32,
    pub regions: u32,
    pub secondary_regions: u32,
    pub spawn: u32,
    pub participants: u32,
    pub actors: u32,
    pub personality_library: u32,
    pub personalities: u32,
    pub personality: u32,
}
impl Links {
    pub(crate) fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "racers": self.racers, "provider": self.provider, "personas": self.personas,
            "regions": self.regions, "secondary_regions": self.secondary_regions,
            "spawn": self.spawn, "participants": self.participants, "actors": self.actors,
            "personality_library": self.personality_library,
            "personalities": self.personalities, "personality": self.personality,
        })
    }
    pub(crate) fn from_json(v: &serde_json::Value) -> Result<Self, Error> {
        let names = [
            "racers",
            "provider",
            "personas",
            "regions",
            "secondary_regions",
            "spawn",
            "participants",
            "actors",
            "personality_library",
            "personalities",
            "personality",
        ];
        let o = v
            .as_object()
            .filter(|o| o.len() == names.len())
            .ok_or_else(|| bad("link fields"))?;
        let mut ports = Vec::new();
        for name in names {
            let port = o
                .get(name)
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n != 0)
                .ok_or_else(|| bad("link port"))?;
            if ports.contains(&port) {
                return Err(bad("duplicate link port"));
            }
            ports.push(port);
        }
        Ok(Self {
            racers: ports[0],
            provider: ports[1],
            personas: ports[2],
            regions: ports[3],
            secondary_regions: ports[4],
            spawn: ports[5],
            participants: ports[6],
            actors: ports[7],
            personality_library: ports[8],
            personalities: ports[9],
            personality: ports[10],
        })
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Persona {
    pub name: String,
    /// Position in the linked PersonaSet, before a specific provider filters it.
    pub provider_index: u16,
    /// Native personality library ordinal; all-ones means no linked personality.
    pub personality_index: u16,
    pub preset: preset::Preset,
    pub driving: nfs_world::replication::vehicle::customization::SpawnOverrides,
}

#[derive(Clone, Debug)]
pub struct Racer {
    pub guid: [u8; 16],
    pub event_type: u32,
    pub personas: Vec<usize>,
    pub regions: Vec<Region>,
    pub secondary_regions: Vec<Region>,
    pub position: [f32; 3],
    pub basis: [[f32; 3]; 3],
}

#[derive(Clone, Debug)]
pub struct Content {
    pub managers: nfs_world::replication::players::ai::Managers,
    pub personality_count: u16,
    pub personas: Vec<Persona>,
    pub racers: Vec<Racer>,
}

fn bad(message: &str) -> Error {
    Error::Content(format!("rivals: {message}"))
}

pub const MAX_REGION_POINTS: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    points: Vec<[f32; 3]>,
    height: f32,
    base_y: f32,
}

impl Region {
    pub fn new(points: Vec<[f32; 3]>, height: f32) -> Result<Self, Error> {
        if !(3..=MAX_REGION_POINTS).contains(&points.len())
            || !height.is_finite()
            || height.abs() > 1_000_000.
            || points
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
        {
            return Err(Error::Content("rival region geometry bound".into()));
        }
        // The volume uses the arithmetic mean of authored points, including Y;
        // it does not use the first vertex or the lower bounding-box plane.
        let base_y = points.iter().fold(0., |sum, p| sum + p[1]) / points.len() as f32;
        Ok(Self {
            points,
            height,
            base_y,
        })
    }

    pub fn points(&self) -> &[[f32; 3]] {
        &self.points
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    /// Evaluate the native volume test, preserving its strict lower Y plane,
    /// inclusive upper plane and horizontal crossing comparisons. A nonpositive
    /// height disables the vertical test. The polygon closes implicitly.
    pub fn contains(&self, position: [f32; 3]) -> bool {
        if position
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
            || self.height > 0.
                && (position[1] <= self.base_y || position[1] > self.base_y + self.height)
        {
            return false;
        }
        let mut previous = self.points[self.points.len() - 1];
        let mut previous_high = previous[2] >= position[2];
        let mut inside = false;
        for &current in &self.points {
            let high = current[2] >= position[2];
            if previous_high != high {
                let a = (current[2] - position[2]) * (previous[0] - current[0]);
                let b = (current[0] - position[0]) * (previous[2] - current[2]);
                if (a >= b) == high {
                    inside = !inside;
                }
            }
            previous = current;
            previous_high = high;
        }
        inside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(height: f32) -> Region {
        Region::new(
            vec![[0., 2., 0.], [10., 2., 0.], [10., 2., 10.], [0., 2., 10.]],
            height,
        )
        .unwrap()
    }

    #[test]
    fn volume_planes_are_asymmetric_and_disabled_by_nonpositive_height() {
        let region = rectangle(4.);
        for (y, expected) in [
            (1., false),
            (2., false),
            (2.001, true),
            (6., true),
            (6.001, false),
        ] {
            assert_eq!(region.contains([5., y, 5.]), expected, "y={y}");
        }
        for height in [0., -1.] {
            let region = rectangle(height);
            assert!(region.contains([5., -500., 5.]));
            assert!(region.contains([5., 500., 5.]));
        }
    }

    #[test]
    fn concavity_and_horizontal_boundary_comparisons_are_preserved() {
        let region = rectangle(4.);
        for (x, z, expected) in [
            (5., 5., true),
            (0., 5., true),
            (10., 5., true),
            (5., 0., false),
            (5., 10., true),
            (-0.01, 5., false),
            (10.01, 5., false),
        ] {
            assert_eq!(region.contains([x, 3., z]), expected, "({x}, {z})");
        }
        let concave = Region::new(
            vec![
                [0., 0., 0.],
                [8., 0., 0.],
                [8., 0., 3.],
                [3., 0., 3.],
                [3., 0., 8.],
                [0., 0., 8.],
            ],
            10.,
        )
        .unwrap();
        assert!(concave.contains([6., 1., 1.]));
        assert!(concave.contains([1., 1., 6.]));
        assert!(!concave.contains([6., 1., 6.]));
        assert!(!concave.contains([9., 1., 1.]));
    }

    #[test]
    fn vertical_origin_is_the_mean_and_invalid_geometry_is_rejected() {
        let region = Region::new(
            vec![[0., 0., 0.], [10., 4., 0.], [10., 4., 10.], [0., 8., 10.]],
            2.,
        )
        .unwrap();
        assert!(!region.contains([5., 3., 5.]));
        assert!(!region.contains([5., 4., 5.]));
        assert!(region.contains([5., 5., 5.]));
        assert!(!region.contains([5., 7., 5.]));
        assert!(Region::new(vec![[0.; 3]; 2], 1.).is_err());
        assert!(Region::new(vec![[0.; 3]; MAX_REGION_POINTS + 1], 1.).is_err());
        assert!(Region::new(vec![[f32::NAN; 3]; 3], 1.).is_err());
        assert!(Region::new(vec![[0.; 3]; 3], f32::INFINITY).is_err());
        assert!(!region.contains([f32::NAN, 5., 5.]));
    }
}
