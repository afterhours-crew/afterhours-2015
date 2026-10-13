// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The level's SpawnPoints scene (`levels/Genesis01/SpawnPoints`). Its game
//! data holds integer participant properties with client notifications, two
//! teleport locations by the garage, the garage and club map markers and a
//! race box trigger at the garage entrance.
//!
//! Field 1 is the garage's VolumeParticipantSetFilter subscription: method 0
//! carries a u32 token and an initial false membership bit, method 1 removes
//! it. Client methods 2 and 3 echo the token after an f64 clock. Field 2 is
//! the associated SetClientNotification Boolean, which triggers the entrance
//! prompt. Track reported occupancy separately from presentation readiness so
//! a rollout crossing need not expose the prompt before the driving context
//! exists. These endpoints are distinct from the scene's integer properties.
use crate::{
    garage::presence::Notification as Flag,
    participants::{Call, Endpoint, Notification},
    replication::{Error, Initial, Record, sublevel},
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PARTICIPANTS: usize = 128;
const ASSIGN_FIELD: usize = 1;
const OCCUPIED_FIELD: usize = 2;
/// Client notification arguments: f64 clock, u32 value, byte padding (all
/// nine official notifications).
const REQUEST_BITS: usize = 103;
const ID_OFFSET: usize = 64;
/// Client methods on the assignment endpoint.
pub const REQUEST: u32 = 2;
pub const RELEASE: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bindings {
    pub assign: Endpoint,
    pub occupied: Endpoint,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpawnPoints {
    bindings: Option<Bindings>,
    assigned: BTreeMap<u16, u32>,
    occupants: BTreeSet<u16>,
    presentation_blocked: BTreeSet<u16>,
}

impl SpawnPoints {
    /// Bind to the SpawnPoints scene created with `key`. `Ok(false)` when the
    /// scene is absent; assignments then stay unsupported.
    pub fn bind(&mut self, records: &[Record], key: u32) -> Result<bool, Error> {
        if self.bindings.is_some() {
            return Err(Error::DuplicateObject);
        }
        let mut found = records.iter().filter_map(|record| match &record.initial {
            Some(Initial::SubLevel { prefix, fields }) if prefix.content_key == key => {
                Some((record.id, fields))
            }
            _ => None,
        });
        let Some((scene, fields)) = found.next() else {
            return Ok(false);
        };
        if found.next().is_some() {
            return Err(Error::DuplicateObject);
        }
        let (Some(sublevel::Initial::Rpc(assign)), Some(sublevel::Initial::RpcBool { rpc, .. })) =
            (fields.get(ASSIGN_FIELD), fields.get(OCCUPIED_FIELD))
        else {
            return Err(Error::TypeMismatch);
        };
        self.bindings = Some(Bindings {
            assign: Endpoint {
                scene,
                selector: assign.selector,
                serial: assign.serial,
            },
            occupied: Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            },
        });
        Ok(true)
    }
    pub fn bindings(&self) -> Option<Bindings> {
        self.bindings
    }
    pub fn assigned(&self, participant: u16) -> Option<u32> {
        self.assigned.get(&participant).copied()
    }
    fn presented(&self) -> bool {
        self.occupants
            .iter()
            .any(|p| !self.presentation_blocked.contains(p))
    }
    /// Gate this participant's contribution to the scene notification while
    /// retaining current zone reports. The caller publishes returned changes
    /// in order with its lifecycle transition. Readiness alone never invents
    /// an occupied zone, and another ready occupant keeps the scene flag on.
    pub fn set_presentation_ready(
        &mut self,
        participant: u16,
        ready: bool,
    ) -> Result<Option<Flag>, Error> {
        let b = self.bindings.ok_or(Error::Unsupported)?;
        if !self.assigned.contains_key(&participant) {
            return Err(Error::UnknownObject);
        }
        let was = self.presented();
        if ready {
            self.presentation_blocked.remove(&participant);
        } else {
            // Assigned participants already obey MAX_PARTICIPANTS.
            self.presentation_blocked.insert(participant);
        }
        let now = self.presented();
        Ok((now != was).then_some(Flag {
            endpoint: b.occupied,
            enabled: now,
        }))
    }
    /// Official hosts number a participant's world spawns 1, 2, 3, ... within
    /// a session: E101's first garage exit assigned 1, and E742's garage exit
    /// assigned 3 after the prelude (1) and starter choice (2).
    pub fn next_value(&self, participant: u16) -> u32 {
        self.assigned
            .get(&participant)
            .map_or(1, |v| v.saturating_add(1))
    }
    /// Subscribe `participant` with `value` as the current crossing token.
    /// The existing zero-aligned assignment codec also supplies the initial
    /// false membership bit, followed by four alignment bits.
    pub fn assign(&mut self, participant: u16, value: u32) -> Result<Notification, Error> {
        let b = self.bindings.ok_or(Error::Unsupported)?;
        if !self.assigned.contains_key(&participant) && self.assigned.len() == MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        let notification = Notification {
            endpoint: b.assign,
            participant,
            call: Call::Assign(value),
        };
        notification.encode()?;
        self.assigned.insert(participant, value);
        Ok(notification)
    }
    /// Clear the subscription when the car enters the garage, allowing the
    /// next lifetime to subscribe with a fresh token. A plain drive-away only
    /// changes occupancy. The participant leaves the occupants here while
    /// its session counter continues.
    /// `Ok(None)` for a participant without an assignment.
    pub fn clear(
        &mut self,
        participant: u16,
    ) -> Result<Option<(Notification, Option<Flag>)>, Error> {
        let b = self.bindings.ok_or(Error::Unsupported)?;
        if !self.assigned.contains_key(&participant) {
            return Ok(None);
        }
        let notification = Notification {
            endpoint: b.assign,
            participant,
            call: Call::Leave,
        };
        notification.encode()?;
        let was = self.presented();
        self.occupants.remove(&participant);
        self.presentation_blocked.remove(&participant);
        let now = self.presented();
        let flag = (now != was).then_some(Flag {
            endpoint: b.occupied,
            enabled: now,
        });
        Ok(Some((notification, flag)))
    }
    /// A client request or release on the assignment endpoint. `Ok(None)` is
    /// "not this endpoint"; `Ok(Some(None))` an accepted call that leaves the
    /// occupied flag unchanged.
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Option<Flag>>, Error> {
        Ok(self.report(body, owns)?.map(|report| report.flag))
    }
    /// `receive` with the accepted call's participant and kind, for callers
    /// that follow the zone (the garage prompt ends when the car leaves it).
    pub fn report(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Report>, Error> {
        let Some(b) = self.bindings else {
            return Ok(None);
        };
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if envelope.references().first() != Some(&b.assign.scene)
            || route.selector() != b.assign.selector
            || !matches!(route.method_index(), REQUEST | RELEASE)
        {
            return Ok(None);
        }
        let &[_, participant] = envelope.references() else {
            return Err(Error::Shape);
        };
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != REQUEST_BITS
        {
            return Err(Error::Shape);
        }
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        let id = route
            .arguments()
            .read_u32(ID_OFFSET, 32)
            .map_err(|_| Error::Shape)?;
        let current = self.assigned(participant).ok_or(Error::Shape)?;
        if id == 0 || id > current {
            return Err(Error::Shape);
        }
        let release = route.method_index() == RELEASE;
        // The next assignment is reserved before it is published. In-flight
        // callbacks still carry the previous spawn id; acknowledge them without
        // applying old occupancy to the current spawn or blocking reliable RPCs.
        if id < current {
            return Ok(Some(Report {
                participant,
                release,
                current: false,
                flag: None,
            }));
        }
        let was = self.presented();
        if release {
            self.occupants.remove(&participant);
        } else {
            self.occupants.insert(participant);
        }
        let now = self.presented();
        Ok(Some(Report {
            participant,
            release,
            current: true,
            flag: (now != was).then_some(Flag {
                endpoint: b.occupied,
                enabled: now,
            }),
        }))
    }
}
/// An accepted client call on the assignment endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    pub participant: u16,
    /// A release (method 3) rather than a request (method 2).
    pub release: bool,
    /// The call carries the current spawn id; a stale id is acknowledged only.
    pub current: bool,
    /// The occupied-flag change to publish, if any.
    pub flag: Option<Flag>,
}

#[cfg(test)]
mod tests;
