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
//! association; it does not create, re-enter or destroy any group. A leave of
//! the current group (official E1026 1311.173 s) is acknowledged with a
//! player-removed notification for the group (`LFPJ` 1); the group's
//! disconnected report to no connection follows (1311.367 s). The caller may
//! then rebind departure to the client's new group. The most recently retired
//! group remains eligible for disconnected reports if renewal commits first;
//! it cannot leave again or change the replacement group's state.
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
    if let Ok(f) = frame(wire, SET_GAME_ATTRIBUTES, 0) {
        return SetGameAttributesRequest::decode(f.body, body_limits()).is_ok_and(|q| {
            q.game_attribs
                .is_some_and(|a| a.0.as_slice() == [(b"gameSessionId".as_slice(), b"0".as_slice())])
        });
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
    GroupLeave,
    Mesh,
    ClearAssociation,
}

pub struct Departure {
    binding: Binding,
    // Renewal may finish before the retired group's disconnected report.
    // Keep one tombstone for bounded retries without rebinding the new group.
    retired_group: Option<u64>,
    left: bool,
    group_left: bool,
    association_cleared: bool,
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
            retired_group: None,
            left: false,
            group_left: false,
            association_cleared: false,
            reports: 0,
            staged: None,
            closed: false,
        })
    }
    /// The current G2 has been left (the leave batch was written).
    pub fn left(&self) -> bool {
        self.left
    }
    /// The current group has been left (the leave batch was written).
    pub fn group_left(&self) -> bool {
        self.group_left
    }
    /// The client's new group replaces the one it left.
    pub fn rebind_group(&mut self, group: u64) -> Result<(), Error> {
        if !self.group_left
            || self.staged.is_some()
            || group == 0
            || group > i64::MAX as u64
            || group == self.binding.group
            || group == self.binding.game
            || group == self.binding.matchmaking
        {
            return Err(Error::Context);
        }
        self.retired_group = Some(self.binding.group);
        self.binding.group = group;
        self.group_left = false;
        self.association_cleared = false;
        Ok(())
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
        if let Ok(f) = frame(request, SET_GAME_ATTRIBUTES, 0) {
            return self.clear_association(f);
        }
        let f = frame(request, UPDATE_MESH_CONNECTION, 0).map_err(|_| Error::Ineligible)?;
        self.mesh(f)
    }
    /// A departed world leaves the current local group alive, with no world
    /// association. Publish the changed attribute once, after an owned leave.
    fn clear_association(&mut self, f: nfs_fire2::Frame<'_>) -> Result<Vec<Vec<u8>>, Error> {
        use nfs_protocol::util::ConfigEntries;
        let q = SetGameAttributesRequest::decode(f.body, body_limits())
            .map_err(|_| Error::Ineligible)?;
        if !self.left
            || self.group_left
            || f.fields.correlation == 0
            || q.game_id != Some(self.binding.group)
            || q.unknown_field_count() != 0
            || q.game_attribs
                .as_ref()
                .is_none_or(|a| a.0.as_slice() != [(b"gameSessionId".as_slice(), b"0".as_slice())])
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
        {
            return Err(Error::Ineligible);
        }
        if self.reports >= MAX_MESH_REPORTS {
            return Err(Error::Bound);
        }
        let ack = wire(
            &[],
            Fields {
                category: 1,
                ..f.fields
            },
        )
        .map_err(|_| Error::Encode)?;
        let mut frames = vec![ack];
        if !self.association_cleared {
            let body = NotifyGameAttribChange {
                game_id: Some(self.binding.group),
                game_attribs: Some(ConfigEntries(vec![(b"gameSessionId", b"0")])),
                ..Default::default()
            }
            .encode(body_limits())
            .map_err(|_| Error::Encode)?;
            frames.push(
                wire(
                    &body,
                    Fields {
                        routing_b: NOTIFY_GAME_ATTRIB_CHANGE,
                        category: 2,
                        correlation: 0,
                        ..f.fields
                    },
                )
                .map_err(|_| Error::Encode)?,
            );
        }
        self.reports += 1;
        self.staged = Some(Staged::ClearAssociation);
        Ok(frames)
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
            || !(q.game_id == Some(b.game) || q.game_id == Some(b.group))
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
        let group = q.game_id == Some(b.group);
        // A repeated leave after the removal was published is acknowledged
        // without a second notification.
        if (group && self.group_left) || (!group && self.left) {
            self.staged = Some(Staged::Mesh);
            return Ok(vec![ack]);
        }
        let removed = NotifyPlayerRemoved {
            player_removed_title_context: Some(0),
            game_id: Some(if group { b.group } else { b.game }),
            is_lockable_for_preferred_joins: Some(group),
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
        self.staged = Some(if group {
            Staged::GroupLeave
        } else {
            Staged::Leave
        });
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
            Some(g) if g == b.group => {
                q.target_group_id == Some(local)
                    || (self.group_left && q.target_group_id == Some(ObjectId(30722, 2, 0)))
            }
            Some(g) if Some(g) == self.retired_group => {
                q.target_group_id == Some(local) || q.target_group_id == Some(ObjectId(30722, 2, 0))
            }
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
            Some(Staged::GroupLeave) => self.group_left = true,
            Some(Staged::ClearAssociation) => self.association_cleared = true,
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
