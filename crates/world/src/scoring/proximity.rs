// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned participant-proximity count for `VehicleProximitySetFilter`
//! (E916, client-verified in E1130). The filter reads one participant set
//! through its `Participants` link and counts the members whose car is
//! within `Radius` metres (3D, inclusive) of the owning participant's car.
//! The owning participant and members without a car are never counted; the
//! count is published only when it changes.
//!
//! The installed filters read channel sets, not every car in the world: the
//! drift train (30 m) reads `DriftParticipantSet` and the speed match (20 m)
//! `HighSpeedParticipantSet`. Each set holds the participants whose drifting
//! or high-speed participant property is set by their own scoring logic
//! (players and AI racers); traffic and cops never run that logic.
use super::{Error, draft::Vehicle, finite};

/// `Radius` (hash of the name).
pub const RADIUS: u32 = 3_298_407_133;
/// `Participants`: link to the participant set the filter reads.
pub const PARTICIPANTS: u32 = 304_393_993;
/// Integer output: participants inside the radius (name not recovered).
pub const COUNT: u32 = 2_795_662_463;
/// `DriftParticipantSet` link of `DriftParticipantChannel` (drift train).
pub const DRIFT_PARTICIPANTS: u32 = 2_549_112_565;
/// `HighSpeedParticipantSet` link of `HighSpeedParticipantChannel` (speed match).
pub const HIGH_SPEED_PARTICIPANTS: u32 = 3_038_050_257;
pub const MAX_VEHICLES: usize = 256;

/// Another participant's car in one participant set, keyed by the set's
/// channel link (for example [`DRIFT_PARTICIPANTS`]). A participant in two
/// sets appears once per set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    pub set: u32,
    pub vehicle: Vehicle,
}

/// Members of `set` within `radius` metres (3D) of the own position,
/// excluding the own identity. A non-positive radius counts nothing.
pub fn count(own: Vehicle, members: &[Member], set: u32, radius: f32) -> Result<i32, Error> {
    finite(radius)?;
    if members.len() > MAX_VEHICLES {
        return Err(Error::Bound);
    }
    if radius <= 0.0 {
        return Ok(0);
    }
    let limit = radius * radius;
    let mut total = 0i32;
    for member in members {
        let other = member.vehicle;
        if member.set != set || other.identity == own.identity {
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
    fn drifting(identity: u32, x: f32) -> Member {
        Member {
            set: DRIFT_PARTICIPANTS,
            vehicle: car(identity, x),
        }
    }
    #[test]
    fn counts_set_members_inside_the_radius_and_never_the_owner() {
        let own = car(1, 0.0);
        let members = [
            drifting(1, 0.0),
            drifting(2, 10.0),
            drifting(3, 29.9),
            drifting(4, 30.1),
        ];
        assert_eq!(count(own, &members, DRIFT_PARTICIPANTS, 30.0).unwrap(), 2);
        assert_eq!(count(own, &members, DRIFT_PARTICIPANTS, 0.0).unwrap(), 0);
        assert_eq!(count(own, &members, DRIFT_PARTICIPANTS, -5.0).unwrap(), 0);
        assert!(count(own, &members, DRIFT_PARTICIPANTS, f32::NAN).is_err());
        let many = vec![drifting(2, 1.0); MAX_VEHICLES + 1];
        assert!(count(own, &many, DRIFT_PARTICIPANTS, 30.0).is_err());
    }
    #[test]
    fn counts_only_the_filtered_set() {
        let own = car(1, 0.0);
        let fast = Member {
            set: HIGH_SPEED_PARTICIPANTS,
            vehicle: car(2, 5.0),
        };
        let members = [fast, drifting(3, 50.0)];
        assert_eq!(count(own, &members, DRIFT_PARTICIPANTS, 30.0).unwrap(), 0);
        assert_eq!(
            count(own, &members, HIGH_SPEED_PARTICIPANTS, 20.0).unwrap(),
            1
        );
        assert_eq!(count(own, &[], DRIFT_PARTICIPANTS, 30.0).unwrap(), 0);
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
