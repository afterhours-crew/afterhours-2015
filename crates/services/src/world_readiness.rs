// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Current-world host readiness gated by independent reliable synchronization.
//! The edge supplies a binding only after the G2 setup and self-validation batch
//! committed. Reply tickets belong to one session; abort/partial writes revoke it.
use crate::bootstrap::{body_limits, frame_limits};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::{gamemanager::*, users::ObjectId};
use std::sync::Arc;
pub const MAX_HOST_REQUESTS: usize = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Source,
    Context,
    Request,
    Prerequisite,
    Clock,
    Bound,
    Write,
    Closed,
    AdmissionBusy,
    NoPending,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub group: u64,
    pub matchmaking: u64,
    pub game: u64,
    pub persona: i64,
    pub user_session: u64,
    pub local_connection: u64,
    pub host_connection: u64,
    pub baseline_join_micros: i64,
}
impl Binding {
    pub fn validate(self, current: bool) -> Result<(), Error> {
        if [
            self.group,
            self.matchmaking,
            self.game,
            self.user_session,
            self.local_connection,
            self.host_connection,
        ]
        .iter()
        .any(|v| *v == 0 || *v > i64::MAX as u64)
            || self.persona <= 0
            || self.group == self.matchmaking
            || self.group == self.game
            || self.matchmaking == self.game
            || self.local_connection == self.host_connection
            || self.baseline_join_micros < 0
            || (current && self.baseline_join_micros == 0)
        {
            return Err(Error::Context);
        }
        Ok(())
    }
}
/// The edge publishes only after this binding's peer sync and complete host ACK
/// have committed. A challenge response alone is not a synchronization proof.
#[derive(Debug, Clone, Copy)]
pub enum InitialReliableSyncPrerequisite {
    Pending,
    #[cfg(test)]
    ChallengeAcknowledgedOnly,
    Published {
        binding: Binding,
    },
}
pub fn frame(w: &[u8], opcode: u16, category: u8) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(w, frame_limits())
        .map_err(|_| Error::Request)?
        .ok_or(Error::Request)?;
    let f = d.frame;
    if d.consumed != w.len()
        || (f.fields.routing_a, f.fields.routing_b, f.fields.category) != (4, opcode, category)
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
        || (category == 2 && f.fields.correlation != 0)
        || nfs_fire2::encode(f, frame_limits()).map_err(|_| Error::Request)? != w
    {
        return Err(Error::Request);
    }
    Ok(f)
}
pub fn wire(body: &[u8], fields: Fields) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields,
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Request)
}
fn normalized_mesh(body: &[u8], binding: Binding) -> Result<(), Error> {
    let q = UpdateMeshConnectionRequest::decode(body, body_limits()).map_err(|_| Error::Request)?;
    if q.unknown_field_count() != 0
        || q.encode(body_limits()).map_err(|_| Error::Request)? != body
        || q.game_id != Some(binding.game)
        || q.source_group_id != Some(ObjectId(30722, 2, binding.local_connection as i64))
        || q.target_group_id != Some(ObjectId(30722, 2, binding.host_connection as i64))
        || q.player_net_connection_status != Some(2)
        || q.player_net_connection_flags != Some(0)
        || q.qos_info.as_ref().is_none_or(|v| {
            v.unknown_field_count() != 0
                || v.latency_ms != Some(0)
                || v.packet_loss != Some(PacketLossBits(0))
        })
    {
        return Err(Error::Request);
    }
    Ok(())
}
/// Opaque policy association, not protocol data or a captured reply. Matching
/// associations constrain consumption of the move-only continuation permit.
#[derive(Clone)]
pub struct Config {
    policy_id: [u8; 32],
}
impl Config {
    pub fn new(policy_id: [u8; 32]) -> Self {
        Self { policy_id }
    }
    pub fn policy_id(&self) -> [u8; 32] {
        self.policy_id
    }
}
#[derive(Debug)]
pub struct PreparedBatch {
    frames: Vec<Vec<u8>>,
    owner: Arc<()>,
    binding: Binding,
    join: i64,
}
impl PreparedBatch {
    pub fn frames(&self) -> &[Vec<u8>] {
        &self.frames
    }
}
#[derive(Debug)]
pub enum Decision {
    PendingPrerequisite,
    PendingWrite,
    Prepared(PreparedBatch),
    Ack(Vec<u8>),
}
/// Move-only authorization, minted once after the entire readiness batch commits.
/// Private fields prevent manufacturing it from copied current IDs or a boolean.
pub struct ContinuationPermit {
    binding: Binding,
    policy_id: [u8; 32],
    _owner: Arc<()>,
}
impl ContinuationPermit {
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn consume(self, expected_policy: [u8; 32]) -> Result<Binding, Error> {
        if self.policy_id != expected_policy || expected_policy == [0; 32] {
            return Err(Error::Source);
        }
        self.binding.validate(true)?;
        Ok(self.binding)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Prepared,
    Emitted,
    Poisoned,
}
pub struct Session {
    config: Config,
    binding: Binding,
    owner: Arc<()>,
    phase: Phase,
    body: Option<Vec<u8>>,
    join: Option<i64>,
    count: usize,
    pending: Option<Vec<u8>>,
    continuation_taken: bool,
}
impl Session {
    /// Only the write-owned integration boundary may supply this current
    /// binding after complete G2 and self11/ACK publication. The allocated Arc
    /// seals each non-Clone batch ticket to its owning session.
    pub fn new(config: &Config, binding: Binding) -> Result<Self, Error> {
        binding.validate(true)?;
        Ok(Self {
            config: config.clone(),
            binding,
            owner: Arc::new(()),
            phase: Phase::Waiting,
            body: None,
            join: None,
            count: 0,
            pending: None,
            continuation_taken: false,
        })
    }
    /// Classify only a canonical current self identity. Status, flags and QoS
    /// remain for the existing self-only handler to validate; no success or
    /// readiness is granted here. Any nonself report receives full validation
    /// through request(), including foreign or malformed targets.
    pub fn is_self_mesh(&self, wire: &[u8]) -> bool {
        let Ok(f) = frame(wire, 29, 0) else {
            return false;
        };
        if f.fields.correlation == 0 {
            return false;
        }
        let Ok(q) = UpdateMeshConnectionRequest::decode(f.body, body_limits()) else {
            return false;
        };
        q.unknown_field_count() == 0
            && q.encode(body_limits()).is_ok_and(|body| body == f.body)
            && q.game_id == Some(self.binding.game)
            && q.source_group_id == Some(ObjectId(30722, 2, self.binding.local_connection as i64))
            && q.target_group_id == q.source_group_id
    }
    pub fn request(
        &mut self,
        q: &[u8],
        gate: InitialReliableSyncPrerequisite,
        chosen_epoch_micros: Option<i64>,
    ) -> Result<Decision, Error> {
        if self.phase == Phase::Poisoned {
            return Err(Error::Closed);
        }
        if self.phase == Phase::Prepared && Arc::strong_count(&self.owner) == 1 {
            self.abort_write();
            return Err(Error::Write);
        }
        if self.phase == Phase::Waiting && self.pending.is_some() {
            return Err(Error::AdmissionBusy);
        }
        let result = self.request_inner(q, gate, chosen_epoch_micros, true);
        if result.is_err() {
            self.phase = Phase::Poisoned;
            self.pending = None;
        }
        result
    }
    /// Re-evaluate the one owned pending request after gate publication. Internal
    /// events/polls do not count as traffic and never change its correlation.
    pub fn resume(
        &mut self,
        gate: InitialReliableSyncPrerequisite,
        time: Option<i64>,
    ) -> Result<Decision, Error> {
        if self.phase == Phase::Poisoned {
            return Err(Error::Closed);
        }
        let q = self.pending.clone().ok_or(Error::NoPending)?;
        let result = self.request_inner(&q, gate, time, false);
        if result.is_err() {
            self.phase = Phase::Poisoned;
            self.pending = None;
        }
        result
    }
    /// The committed current world association this session serves.
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn has_pending_request(&self) -> bool {
        self.pending.is_some()
    }
    pub fn admitted_requests(&self) -> usize {
        self.count
    }
    fn request_inner(
        &mut self,
        q: &[u8],
        gate: InitialReliableSyncPrerequisite,
        time: Option<i64>,
        admission: bool,
    ) -> Result<Decision, Error> {
        let f = frame(q, 29, 0)?;
        if f.fields.correlation == 0 {
            return Err(Error::Request);
        }
        normalized_mesh(f.body, self.binding)?;
        if self.body.as_ref().is_some_and(|b| b != f.body) {
            return Err(Error::Request);
        }
        if admission && self.count >= MAX_HOST_REQUESTS {
            return Err(Error::Bound);
        }
        if admission {
            self.count += 1;
        }
        let ack = wire(
            &[],
            Fields {
                category: 1,
                ..f.fields
            },
        )?;
        if self.phase == Phase::Emitted {
            return Ok(Decision::Ack(ack));
        }
        if self.phase == Phase::Prepared {
            return Ok(Decision::PendingWrite);
        }
        // Challenge-only evidence has no runtime proof representation. The
        // test variant demonstrates that it is indistinguishable from pending.
        let proof = match gate {
            InitialReliableSyncPrerequisite::Pending => None,
            #[cfg(test)]
            InitialReliableSyncPrerequisite::ChallengeAcknowledgedOnly => None,
            InitialReliableSyncPrerequisite::Published { binding } => Some(binding),
        };
        match proof {
            None => {
                if self.pending.is_none() {
                    self.pending = Some(q.to_vec());
                }
                return Ok(Decision::PendingPrerequisite);
            }
            Some(binding) if binding != self.binding => {
                return Err(Error::Prerequisite);
            }
            Some(_) => {}
        }
        let time = time.ok_or(Error::Clock)?;
        // Explicit construction policy: fresh positive chosen time after the
        // already-positive current setup value. No fixed delay is inferred.
        if time <= self.binding.baseline_join_micros {
            return Err(Error::Clock);
        }
        let state = NotifyGamePlayerStateChange {
            game_id: Some(self.binding.game),
            player_id: Some(self.binding.persona),
            player_state: Some(4),
            ..Default::default()
        };
        let state = wire(
            &state.encode(body_limits()).map_err(|_| Error::Source)?,
            Fields {
                routing_a: 4,
                routing_b: 116,
                category: 2,
                ..Default::default()
            },
        )?;
        let join = NotifyPlayerJoinCompleted {
            game_id: Some(self.binding.game),
            player_id: Some(self.binding.persona),
            joined_game_timestamp: Some(time),
            ..Default::default()
        };
        let join = wire(
            &join.encode(body_limits()).map_err(|_| Error::Source)?,
            Fields {
                routing_a: 4,
                routing_b: 30,
                category: 2,
                ..Default::default()
            },
        )?;
        self.body = Some(f.body.to_vec());
        self.join = Some(time);
        self.phase = Phase::Prepared;
        self.pending = None;
        Ok(Decision::Prepared(PreparedBatch {
            frames: vec![ack, state, join],
            owner: Arc::clone(&self.owner),
            binding: self.binding,
            join: time,
        }))
    }
    /// Consume only after all three writes succeed. The writer must retain the
    /// whole prepared object; failure/partial write calls abort_write instead.
    pub fn commit_written(&mut self, batch: PreparedBatch) -> Result<(), Error> {
        if self.phase != Phase::Prepared
            || !Arc::ptr_eq(&batch.owner, &self.owner)
            || batch.binding != self.binding
            || self.join != Some(batch.join)
        {
            self.phase = Phase::Poisoned;
            return Err(Error::Write);
        }
        self.phase = Phase::Emitted;
        Ok(())
    }
    pub fn abort_write(&mut self) {
        self.phase = Phase::Poisoned;
        self.pending = None;
    }
    pub fn emitted_join_time(&self) -> Option<i64> {
        if self.phase == Phase::Emitted {
            self.join
        } else {
            None
        }
    }
    pub fn take_continuation_permit(&mut self) -> Option<ContinuationPermit> {
        if self.phase != Phase::Emitted
            || self.continuation_taken
            || self.config.policy_id == [0; 32]
        {
            return None;
        }
        self.continuation_taken = true;
        Some(ContinuationPermit {
            binding: self.binding,
            policy_id: self.config.policy_id,
            _owner: self.owner.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
