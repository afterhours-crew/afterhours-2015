// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{bits::BitWriter, participants::Endpoint, replication::Error};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeSet;

pub const COMPONENT: usize = 24;
const MAX_PARTICIPANTS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Notification {
    pub endpoint: Endpoint,
    pub enabled: bool,
}
impl Notification {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.endpoint.scene == 0 || self.endpoint.scene > 8191 || self.endpoint.selector > 511 {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(u64::from(!self.enabled), 32)
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 150,
                max_references: 1,
                max_payload_bytes: 7,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = e
            .route(RouteProfile::ClientReceive)
            .map_err(|_| Error::Shape)?;
        if e.words() != [0, 0]
            || e.references().len() != 1
            || !e.remaining().is_empty()
            || route.method_index() > 1
            || route.arguments().len() != 5
        {
            return Err(Error::Shape);
        }
        let result = Self {
            endpoint: Endpoint {
                scene: e.references()[0],
                selector: route.selector(),
                serial: route.serial().ok_or(Error::Shape)?,
            },
            enabled: route.method_index() == 0,
        };
        result.encode()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Presence {
    endpoint: Option<Endpoint>,
    occupants: BTreeSet<u16>,
}
impl Presence {
    pub fn set(
        &mut self,
        endpoint: Endpoint,
        participant: u16,
        present: bool,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Notification>, Error> {
        Notification {
            endpoint,
            enabled: present,
        }
        .encode()?;
        if participant == 0 || participant > 8191 || !owns(participant) {
            return Err(Error::UnknownObject);
        }
        if self.endpoint.is_some_and(|current| current != endpoint) {
            return Err(Error::UnknownObject);
        }
        if present
            && !self.occupants.contains(&participant)
            && self.occupants.len() == MAX_PARTICIPANTS
        {
            return Err(Error::Bound);
        }
        let was_present = !self.occupants.is_empty();
        if present {
            self.occupants.insert(participant);
        } else {
            self.occupants.remove(&participant);
        }
        self.endpoint = Some(endpoint);
        let enabled = !self.occupants.is_empty();
        Ok((enabled != was_present).then_some(Notification { endpoint, enabled }))
    }
}

#[cfg(test)]
mod tests;
