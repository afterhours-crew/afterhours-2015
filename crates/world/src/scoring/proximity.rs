// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned participant-proximity count for `VehicleProximitySetFilter`
//! (E916, inferred from the installed wiring): the node's `Radius` input is
//! fed by a Float constant (30 m in the drift train, 20 m in the speed match)
//! and its integer output counts the participant set left inside that
//! radius; the count feeds a `CompareInt` against 1 and a synchronized
//! integer. The set is the other replicated cars of the session; the owned
//! car is never counted. Publication happens only when the count changes.
use super::{Error, draft::Vehicle, finite};

/// `Radius` (hash of the name).
pub const RADIUS: u32 = 3_298_407_133;
/// Integer output: participants inside the radius (name not recovered).
pub const COUNT: u32 = 2_795_662_463;
pub const MAX_VEHICLES: usize = 256;

/// Cars within `radius` metres (3D) of the own position, excluding the own
/// identity. A non-positive radius counts nothing.
pub fn count(own: Vehicle, others: &[Vehicle], radius: f32) -> Result<i32, Error> {
    finite(radius)?;
    if others.len() > MAX_VEHICLES {
        return Err(Error::Bound);
    }
    if radius <= 0.0 {
        return Ok(0);
    }
    let limit = radius * radius;
    let mut total = 0i32;
    for other in others {
        if other.identity == own.identity {
            continue;
        }
        let mut squared = 0.0f32;
        for axis in 0..3 {
            let d = finite(other.position[axis] - own.position[axis])?;
            squared = finite(squared + d * d)?;
        }
        if squared <= limit {
            total += 1;
        }
    }
    Ok(total)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Node {
    published: Option<i32>,
}
impl Node {
    /// The value to publish for this measurement, if it differs from the last
    /// published one.
    pub fn update(&mut self, value: i32) -> Option<i32> {
        if self.published == Some(value) {
            return None;
        }
        self.published = Some(value);
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn car(identity: u32, x: f32) -> Vehicle {
        Vehicle {
            identity,
            position: [x, 0.0, 0.0],
            velocity: [0.0; 3],
        }
    }
    #[test]
    fn counts_others_inside_the_radius_and_never_the_owner() {
        let own = car(1, 0.0);
        let others = [car(1, 0.0), car(2, 10.0), car(3, 29.9), car(4, 30.1)];
        assert_eq!(count(own, &others, 30.0).unwrap(), 2);
        assert_eq!(count(own, &others, 0.0).unwrap(), 0);
        assert_eq!(count(own, &others, -5.0).unwrap(), 0);
        assert!(count(own, &others, f32::NAN).is_err());
        let many = vec![car(2, 1.0); MAX_VEHICLES + 1];
        assert!(count(own, &many, 30.0).is_err());
    }
    #[test]
    fn node_publishes_only_changes() {
        let mut node = Node::default();
        assert_eq!(node.update(0), Some(0));
        assert_eq!(node.update(0), None);
        assert_eq!(node.update(2), Some(2));
        assert_eq!(node.update(2), None);
    }
}
