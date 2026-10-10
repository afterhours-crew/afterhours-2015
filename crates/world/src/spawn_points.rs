// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The level's SpawnPoints scene (`levels/Genesis01/SpawnPoints`). Its game
//! data holds integer participant properties with client notifications, two
//! teleport locations by the garage, the garage and club map markers and a
//! race box trigger at the garage entrance.
//!
//! The host sets the participant's integer property on field 1 (method 0,
//! u32 value). The official session sent 1, 2 and 3 at the prelude, the
//! starter car and the garage exit. The client notifies the same field with
//! method 2 and method 3, echoing the value after an f64 clock. The host
//! mirrors that on the scene's flag (field 2); the timing fits entering and
//! leaving the garage trigger box. Observed in E742/E747; the value meanings
//! are inferred from that timing.
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
/// Property value the official host set when the player left the garage.
pub const GARAGE_EXIT: u32 = 3;

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
    /// Set `participant`'s property to `value`.
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
    /// A client request or release on the assignment endpoint. `Ok(None)` is
    /// "not this endpoint"; `Ok(Some(None))` an accepted call that leaves the
    /// occupied flag unchanged.
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Option<Flag>>, Error> {
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
        if self.assigned(participant) != Some(id) {
            return Err(Error::Shape);
        }
        let was = !self.occupants.is_empty();
        if route.method_index() == REQUEST {
            self.occupants.insert(participant);
        } else {
            self.occupants.remove(&participant);
        }
        let now = !self.occupants.is_empty();
        Ok(Some((now != was).then_some(Flag {
            endpoint: b.occupied,
            enabled: now,
        })))
    }
}

#[cfg(test)]
mod tests;
