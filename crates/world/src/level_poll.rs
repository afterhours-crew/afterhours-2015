// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The level root's periodic poll. Throughout the official session (garage
//! and world) the host calls the level root's field 51 (an RPC field, wire
//! selector 49) with method 0 about every 1.1 s, scene-scoped. Each client answers on
//! the same field, participant-scoped, with method 1 and a one-bit value
//! (always set in the official session). The official host's spawn
//! assignments go out on this tick, and every client spawn request follows a
//! poll answer. Observed in E742/E747; modeled for E756.
use crate::{
    garage::presence::Notification as Flag,
    participants::Endpoint,
    replication::{Error, Initial, Record, sublevel},
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeMap;

const FIELD: usize = 51;
const MAX_PARTICIPANTS: usize = 128;
/// Median official interval between polls.
pub const INTERVAL_MS: u64 = 1_100;
/// Client answer method and argument size (one value bit and byte padding).
pub const ANSWER: u32 = 1;
const ANSWER_BITS: usize = 7;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LevelPoll {
    endpoint: Option<Endpoint>,
    next_ms: Option<u64>,
    answers: BTreeMap<u16, bool>,
}

impl LevelPoll {
    /// Bind to field 51 of the level root scene created with `key`.
    /// `Ok(false)` when the scene is absent.
    pub fn bind(&mut self, records: &[Record], key: u32) -> Result<bool, Error> {
        if self.endpoint.is_some() {
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
        let Some(sublevel::Initial::Rpc(rpc)) = fields.get(FIELD) else {
            return Err(Error::TypeMismatch);
        };
        self.endpoint = Some(Endpoint {
            scene,
            selector: rpc.selector,
            serial: rpc.serial,
        });
        Ok(true)
    }
    pub fn endpoint(&self) -> Option<Endpoint> {
        self.endpoint
    }
    /// The poll due at `now_ms`, if any. The first poll is due immediately;
    /// later ones follow at the interval without bursts after a stall.
    pub fn due(&mut self, now_ms: u64) -> Result<Option<Flag>, Error> {
        let Some(endpoint) = self.endpoint else {
            return Ok(None);
        };
        if self.next_ms.is_some_and(|next| now_ms < next) {
            return Ok(None);
        }
        let next = now_ms.checked_add(INTERVAL_MS).ok_or(Error::Bound)?;
        let poll = Flag {
            endpoint,
            enabled: true,
        };
        poll.encode()?;
        self.next_ms = Some(next);
        Ok(Some(poll))
    }
    /// The last answer from `participant`.
    pub fn answer(&self, participant: u16) -> Option<bool> {
        self.answers.get(&participant).copied()
    }
    /// A client answer. `Ok(None)` is "not this endpoint".
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<bool>, Error> {
        let Some(endpoint) = self.endpoint else {
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
        if envelope.references().first() != Some(&endpoint.scene)
            || route.selector() != endpoint.selector
            || route.method_index() != ANSWER
        {
            return Ok(None);
        }
        let &[_, participant] = envelope.references() else {
            return Err(Error::Shape);
        };
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != ANSWER_BITS
        {
            return Err(Error::Shape);
        }
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        if !self.answers.contains_key(&participant) && self.answers.len() == MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        let value = route.arguments().read_u32(0, 1).map_err(|_| Error::Shape)? == 1;
        self.answers.insert(participant, value);
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests;
