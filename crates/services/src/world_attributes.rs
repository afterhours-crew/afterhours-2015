// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Transient group-to-world association after committed host readiness.
//! No captured source data, clock, sockets or durable progression is required.
use crate::{bootstrap::frame_limits, world_readiness};
use nfs_fire2::Fields;
use nfs_protocol::{gamemanager::*, util::ConfigEntries};
pub const MAX_ATTRIBUTE_REQUESTS: usize = 8; // Resource bound, not a protocol retry limit.
const MAX_ATTRIBUTE_FRAME_BYTES: usize = 128;
fn attribute_body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 112,
        max_depth: 1,
        max_values: 4,
        max_collection: 1,
        max_byte_string: 21,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Source,
    Permit,
    Request,
    Bound,
    Write,
    Closed,
    DuplicateEnable,
}

fn canonical_body(body: &[u8], group: u64, game: u64) -> Result<(), Error> {
    let q = SetGameAttributesRequest::decode(body, attribute_body_limits())
        .map_err(|_| Error::Request)?;
    let decimal = game.to_string();
    if group == 0
        || game == 0
        || group == game
        || group > i64::MAX as u64
        || game > i64::MAX as u64
        || q.unknown_field_count() != 0
        || q.game_id != Some(group)
        || q.game_attribs
            .as_ref()
            .is_none_or(|a| a.0.as_slice() != [(b"gameSessionId".as_slice(), decimal.as_bytes())])
        || q.encode(attribute_body_limits())
            .map_err(|_| Error::Request)?
            != body
    {
        return Err(Error::Request);
    }
    Ok(())
}

#[derive(Clone)]
pub struct Config {
    policy_id: [u8; 32],
}
impl Config {
    /// The association must match the readiness policy. Zero cannot authorize
    /// a session because readiness cannot mint a zero-policy permit.
    pub fn new(policy_id: [u8; 32]) -> Self {
        Self { policy_id }
    }
}
struct PendingWrite {
    binding: world_readiness::Binding,
}
pub struct Session {
    config: Config,
    binding: Option<world_readiness::Binding>,
    pending: Option<PendingWrite>,
    committed: bool,
    count: usize,
    closed: bool,
}
impl Session {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            binding: None,
            pending: None,
            committed: false,
            count: 0,
            closed: false,
        }
    }
    pub fn enable(&mut self, permit: world_readiness::ContinuationPermit) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.binding.is_some() {
            self.closed = true;
            return Err(Error::DuplicateEnable);
        }
        match permit.consume(self.config.policy_id) {
            Ok(binding) => {
                self.binding = Some(binding);
                Ok(())
            }
            Err(_) => {
                self.closed = true;
                Err(Error::Permit)
            }
        }
    }
    pub fn is_enabled(&self) -> bool {
        self.binding.is_some() && !self.closed
    }
    /// Published transient association, visible only after the entire pair wrote.
    pub fn association(&self) -> Option<(u64, u64)> {
        if self.closed || !self.committed {
            return None;
        }
        self.binding.map(|b| (b.group, b.game))
    }
    pub fn has_pending_write(&self) -> bool {
        self.pending.is_some()
    }
    pub fn response(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let Some(binding) = self.binding else {
            return Ok(None);
        };
        let result = self.response_inner(wire, binding);
        if result.is_err() {
            self.abort_write();
        }
        result
    }
    fn response_inner(
        &mut self,
        wire: &[u8],
        binding: world_readiness::Binding,
    ) -> Result<Option<Vec<Vec<u8>>>, Error> {
        if wire.len() > MAX_ATTRIBUTE_FRAME_BYTES {
            return Err(Error::Bound);
        }
        if self.pending.is_some() {
            return Err(Error::Write);
        }
        let decoded = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Request)?
            .ok_or(Error::Request)?;
        if decoded.consumed != wire.len() {
            return Err(Error::Request);
        }
        if (
            decoded.frame.fields.routing_a,
            decoded.frame.fields.routing_b,
        ) != (4, SET_GAME_ATTRIBUTES)
        {
            return Ok(None);
        }
        self.count = self.count.checked_add(1).ok_or(Error::Bound)?;
        if self.count > MAX_ATTRIBUTE_REQUESTS {
            return Err(Error::Bound);
        }
        let q = world_readiness::frame(wire, SET_GAME_ATTRIBUTES, 0).map_err(|_| Error::Request)?;
        if q.fields.correlation == 0 {
            return Err(Error::Request);
        }
        canonical_body(q.body, binding.group, binding.game)?;
        let ack = world_readiness::wire(
            &[],
            Fields {
                category: 1,
                ..q.fields
            },
        )
        .map_err(|_| Error::Request)?;
        if self.committed {
            return Ok(Some(vec![ack]));
        }
        let decimal = binding.game.to_string();
        let body = NotifyGameAttribChange {
            game_id: Some(binding.group),
            game_attribs: Some(ConfigEntries(vec![(b"gameSessionId", decimal.as_bytes())])),
            ..Default::default()
        }
        .encode(attribute_body_limits())
        .map_err(|_| Error::Request)?;
        let notice = world_readiness::wire(
            &body,
            Fields {
                routing_b: NOTIFY_GAME_ATTRIB_CHANGE,
                category: 2,
                correlation: 0,
                ..q.fields
            },
        )
        .map_err(|_| Error::Request)?;
        self.pending = Some(PendingWrite { binding });
        Ok(Some(vec![ack, notice]))
    }
    /// Adapter calls only after every frame in the returned pair actually wrote.
    /// Other complete supported batches have no pending attribute ticket: no-op.
    pub fn commit_after_write(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if let Some(pending) = self.pending.take() {
            if self.binding != Some(pending.binding) || self.committed {
                self.abort_write();
                return Err(Error::Write);
            }
            self.committed = true;
        }
        Ok(())
    }
    pub fn abort_write(&mut self) {
        self.closed = true;
        self.pending = None;
    }
}

#[cfg(test)]
mod tests;
