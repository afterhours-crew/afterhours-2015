// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local self-mesh validation for an already committed world setup.
//! This establishes control-plane publication only; host reliable synchronization
//! remains a separate prerequisite handled by world_readiness.
use crate::{
    bootstrap::body_limits,
    world_readiness::{frame, wire},
};
use nfs_fire2::Fields;
use nfs_protocol::{gamemanager::*, users::ObjectId};
pub const MAX_SELF_REQUESTS: usize = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Context,
    Request,
    Bound,
    Write,
    Closed,
    Encode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Current {
    pub group_id: u64,
    pub session_id: u64,
    pub game_id: u64,
    pub persona_id: i64,
    pub user_session_id: u64,
    pub local_connection_group_id: u64,
    pub host_connection_group_id: u64,
    pub scenario_id: u64,
}
impl Current {
    pub fn validate(&self) -> Result<(), Error> {
        if [
            self.group_id,
            self.session_id,
            self.game_id,
            self.user_session_id,
            self.local_connection_group_id,
            self.host_connection_group_id,
        ]
        .iter()
        .any(|v| *v == 0 || *v > i64::MAX as u64)
            || self.persona_id <= 0
            || self.group_id == self.session_id
            || self.group_id == self.game_id
            || self.session_id == self.game_id
            || self.local_connection_group_id == self.host_connection_group_id
        {
            return Err(Error::Context);
        }
        Ok(())
    }
}
/// Explicit local self-connection policy: no external QoS validation, no avoid
/// lists, zero failed attempts/tier and network topology zero. Other policies
/// must be implemented separately, not inferred from arbitrary client fields.
#[derive(Clone, Default)]
pub struct Config;
pub struct Session {
    current: Current,
    published: bool,
    pending: bool,
    self_requests: usize,
    closed: bool,
}
impl Session {
    /// The edge supplies this binding only after the accepted world setup wrote.
    pub fn new(_config: &Config, current: Current) -> Result<Self, Error> {
        current.validate()?;
        Ok(Self {
            current,
            published: false,
            pending: false,
            self_requests: 0,
            closed: false,
        })
    }
    pub fn validation_emitted(&self) -> bool {
        self.published && !self.closed
    }
    pub fn self_request_count(&self) -> usize {
        self.self_requests
    }
    pub fn has_pending_write(&self) -> bool {
        self.pending
    }
    pub fn reply(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = self.responses(bytes);
        if result.is_err() {
            self.abort_write();
        }
        result
    }
    fn responses(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        if self.pending {
            return Err(Error::Write);
        }
        if self.self_requests >= MAX_SELF_REQUESTS {
            return Err(Error::Bound);
        }
        let f = frame(bytes, 29, 0).map_err(|_| Error::Request)?;
        let q = UpdateMeshConnectionRequest::decode(f.body, body_limits())
            .map_err(|_| Error::Request)?;
        if f.fields.correlation == 0
            || q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Request)? != f.body
            || q.game_id != Some(self.current.game_id)
            || q.source_group_id
                != Some(ObjectId(
                    30722,
                    2,
                    self.current.local_connection_group_id as i64,
                ))
            || q.target_group_id != q.source_group_id
            || q.player_net_connection_status != Some(2)
            || q.player_net_connection_flags != Some(0)
            || q.qos_info.as_ref().is_none_or(|v| {
                v.unknown_field_count() != 0
                    || v.packet_loss != Some(PacketLossBits(0))
                    || v.latency_ms != Some(0)
            })
        {
            return Err(Error::Request);
        }
        let ack = wire(
            &[],
            Fields {
                category: 1,
                ..f.fields
            },
        )
        .map_err(|_| Error::Encode)?;
        let mut out = Vec::with_capacity(2);
        if !self.published {
            let body = NotifyMatchmakingSessionConnectionValidated {
                connection_validated_results: Some(ConnectionValidationResults {
                    fail_count: Some(0),
                    network_topology: Some(0),
                    tier: Some(0),
                    ..Default::default()
                }),
                dispatch_session_finished: Some(true),
                game_id: Some(self.current.game_id),
                user_group_id: Some(ObjectId(0, 0, 0)),
                scenario_id: Some(self.current.scenario_id),
                session_id: Some(self.current.session_id),
                qos_validation_performed: Some(false),
                user_session_id: Some(self.current.user_session_id),
                ..Default::default()
            }
            .encode(body_limits())
            .map_err(|_| Error::Encode)?;
            out.push(
                wire(
                    &body,
                    Fields {
                        routing_a: 4,
                        routing_b: 11,
                        category: 2,
                        ..Default::default()
                    },
                )
                .map_err(|_| Error::Encode)?,
            );
            self.pending = true;
        }
        out.push(ack);
        self.self_requests += 1;
        Ok(out)
    }
    /// Call only when every frame in the returned batch wrote. Publication is
    /// session-owned; an unrelated session's commit cannot release this pending state.
    pub fn commit_after_write(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.pending {
            self.pending = false;
            self.published = true;
        }
        Ok(())
    }
    pub fn abort_write(&mut self) {
        self.closed = true;
        self.pending = false;
    }
}
#[cfg(test)]
mod tests;
