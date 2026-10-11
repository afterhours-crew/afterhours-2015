// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Departure from the current world: `leaveGameByGroup` (`4/22`) and the
//! disconnected mesh reports (`4/29`, status 0) that follow it.
//!
//! The leave is answered with an empty acknowledgement and a player-removed
//! notification (reason `PLAYER_LEFT`) for the current G2 and persona, the
//! order the official service used. A disconnected report for the current G2
//! (local to host, or to no connection) or for the current G1 self mesh is
//! acknowledged with an empty response. State changes only after the batch is
//! written; a failed write closes the departure. Departure ends this world
//! association; it does not create, re-enter or destroy any group.
use crate::{
    bootstrap::body_limits,
    world_readiness::{Binding, frame, wire},
};
use nfs_fire2::Fields;
use nfs_protocol::{gamemanager::*, users::ObjectId};

/// Mesh reports accepted per departure: two meshes, each possibly retried.
pub const MAX_MESH_REPORTS: usize = 8;
/// `PlayerNetConnectionStatus` of a disconnected mesh.
pub const DISCONNECTED: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a complete departure request for this world: not mine.
    Ineligible,
    Context,
    Bound,
    /// A batch is staged and not yet written, or a write failed.
    Write,
    Closed,
    Encode,
}

/// Whether `wire` is a request this service may own: any leave, or a mesh
/// report whose status is disconnected. Ownership still needs the binding.
pub fn owns(wire: &[u8]) -> bool {
    if frame(wire, LEAVE_GAME_BY_GROUP, 0).is_ok() {
        return true;
    }
    let Ok(f) = frame(wire, UPDATE_MESH_CONNECTION, 0) else {
        return false;
    };
    UpdateMeshConnectionRequest::decode(f.body, body_limits())
        .is_ok_and(|q| q.player_net_connection_status == Some(DISCONNECTED))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Staged {
    Leave,
    Mesh,
}

pub struct Departure {
    binding: Binding,
    left: bool,
    reports: usize,
    staged: Option<Staged>,
    closed: bool,
}
impl Departure {
    /// `binding` is the committed current world association.
    pub fn new(binding: Binding) -> Result<Self, Error> {
        binding.validate(true).map_err(|_| Error::Context)?;
        Ok(Self {
            binding,
            left: false,
            reports: 0,
            staged: None,
            closed: false,
        })
    }
    /// The current G2 has been left (the leave batch was written).
    pub fn left(&self) -> bool {
        self.left
    }
    /// Answer one departure request. `Ineligible` is "not mine": the caller
    /// must not fall back to another reply.
    pub fn reply(&mut self, request: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.staged.is_some() {
            self.abort_write();
            return Err(Error::Write);
        }
        if let Ok(f) = frame(request, LEAVE_GAME_BY_GROUP, 0) {
            return self.leave(f);
        }
        let f = frame(request, UPDATE_MESH_CONNECTION, 0).map_err(|_| Error::Ineligible)?;
        self.mesh(f)
    }
    fn leave(&mut self, f: nfs_fire2::Frame<'_>) -> Result<Vec<Vec<u8>>, Error> {
        let q = LeaveGameByGroupRequest::decode(f.body, body_limits())
            .map_err(|_| Error::Ineligible)?;
        let b = self.binding;
        if f.fields.correlation == 0
            || q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
            || q.blaze_object_type_and_id != Some(ObjectId(30722, 2, b.local_connection as i64))
            || q.player_removed_title_context != Some(0)
            || q.game_id != Some(b.game)
            || q.player_id != Some(b.persona)
            || q.player_removed_reason != Some(GROUP_LEFT)
            || q.title_context_string != Some(b"")
        {
            return Err(Error::Ineligible);
        }
        let ack = wire(
            &[],
            Fields {
                category: 1,
                ..f.fields
            },
        )
        .map_err(|_| Error::Encode)?;
        // A repeated leave after the removal was published is acknowledged
        // without a second notification.
        if self.left {
            self.staged = Some(Staged::Mesh);
            return Ok(vec![ack]);
        }
        let removed = NotifyPlayerRemoved {
            player_removed_title_context: Some(0),
            game_id: Some(b.game),
            is_lockable_for_preferred_joins: Some(false),
            player_id: Some(b.persona),
            player_removed_reason: Some(PLAYER_LEFT),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        let removed = wire(
            &removed,
            Fields {
                routing_a: COMPONENT,
                routing_b: NOTIFY_PLAYER_REMOVED,
                category: 2,
                ..Default::default()
            },
        )
        .map_err(|_| Error::Encode)?;
        self.staged = Some(Staged::Leave);
        Ok(vec![ack, removed])
    }
    fn mesh(&mut self, f: nfs_fire2::Frame<'_>) -> Result<Vec<Vec<u8>>, Error> {
        let q = UpdateMeshConnectionRequest::decode(f.body, body_limits())
            .map_err(|_| Error::Ineligible)?;
        let b = self.binding;
        let local = ObjectId(30722, 2, b.local_connection as i64);
        let current = match q.game_id {
            Some(g) if g == b.game => {
                q.target_group_id == Some(ObjectId(30722, 2, b.host_connection as i64))
                    || q.target_group_id == Some(ObjectId(30722, 2, 0))
            }
            Some(g) if g == b.group => q.target_group_id == Some(local),
            _ => false,
        };
        if f.fields.correlation == 0
            || q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
            || q.player_net_connection_status != Some(DISCONNECTED)
            || q.player_net_connection_flags.is_none()
            || q.source_group_id != Some(local)
            || !current
            || q.qos_info
                .as_ref()
                .is_none_or(|v| v.unknown_field_count() != 0)
        {
            return Err(Error::Ineligible);
        }
        if self.reports >= MAX_MESH_REPORTS {
            return Err(Error::Bound);
        }
        self.reports += 1;
        self.staged = Some(Staged::Mesh);
        Ok(vec![
            wire(
                &[],
                Fields {
                    category: 1,
                    ..f.fields
                },
            )
            .map_err(|_| Error::Encode)?,
        ])
    }
    /// The staged batch was completely written.
    pub fn committed(&mut self) -> Result<(), Error> {
        match self.staged.take() {
            Some(Staged::Leave) => self.left = true,
            Some(Staged::Mesh) => {}
            None => return Err(Error::Write),
        }
        Ok(())
    }
    pub fn pending(&self) -> bool {
        self.staged.is_some()
    }
    /// A failed or partial write: nothing staged is committed.
    pub fn abort_write(&mut self) {
        self.staged = None;
        self.closed = true;
    }
}

#[cfg(test)]
mod tests;
