// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Durable seen-marker state; event availability is a separate progression rule.
use super::*;
use crate::net::items::story::{Action, Request};
use nfs_world::participants::{Endpoint, HostRpc};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct State {
    binding: Option<(u16, Endpoint)>,
    cleared: bool,
    published: BTreeSet<u32>,
    acknowledgements: BTreeSet<u32>,
}

impl State {
    fn bind(&mut self, participant: u16, endpoint: Endpoint) {
        // World IDs are never reused within a connection. A new scene or
        // participant therefore starts a new publication lifetime.
        if self.binding != Some((participant, endpoint)) {
            *self = Self {
                binding: Some((participant, endpoint)),
                ..Default::default()
            };
        }
    }

    fn publish(&mut self, stored: &BTreeSet<u32>, capacity: usize) -> Vec<HostRpc> {
        let Some((_, endpoint)) = self.binding else {
            return vec![];
        };
        if !self.published.is_subset(stored) {
            self.cleared = false;
            self.published.clear();
        }
        self.acknowledgements.retain(|id| stored.contains(id));
        let mut calls = vec![];
        if !self.cleared && capacity > 0 {
            calls.push(HostRpc::GameMode(nfs_world::map_points::publication(
                endpoint, None,
            )));
            self.cleared = true;
        }
        // Repeated requests still get an acknowledgement. When restoration
        // and an acknowledgement overlap, publish that ID only once.
        let ids: BTreeSet<_> = self
            .acknowledgements
            .iter()
            .copied()
            .chain(stored.difference(&self.published).copied())
            .collect();
        for id in ids.into_iter().take(capacity.saturating_sub(calls.len())) {
            calls.push(HostRpc::GameMode(nfs_world::map_points::publication(
                endpoint,
                Some(id),
            )));
            self.published.insert(id);
            self.acknowledgements.remove(&id);
        }
        calls
    }
}

fn endpoint(
    players: &Players,
    role: crate::scene_roles::SceneRpc,
) -> Result<Option<Endpoint>, replication::Error> {
    let Some(scene) = players.objects().scene(role.scene) else {
        return Ok(None);
    };
    let Some(replication::Initial::SubLevel { fields, .. }) =
        players.objects().get(scene).map(|o| &o.initial)
    else {
        return Err(replication::Error::TypeMismatch);
    };
    let rpc = fields
        .get(role.serializer)
        .and_then(replication::sublevel::Initial::rpc)
        .ok_or(replication::Error::TypeMismatch)?;
    Ok(Some(Endpoint {
        scene,
        selector: rpc.selector,
        serial: rpc.serial,
    }))
}

fn stored(loaded: &crate::persistent::Loaded) -> Result<BTreeSet<u32>, replication::Error> {
    let table = crate::persistent::key(nfs_services::map_points::TABLE);
    nfs_services::map_points::seen(loaded.tables().get(&table))
        .map_err(|_| replication::Error::Shape)
}

impl PlayerListener {
    pub(super) fn map_point_request(
        &self,
        players: &Players,
        body: nfs_protocol::world::BitSpan<'_>,
    ) -> Result<Option<Request>, replication::Error> {
        let Some(role) = self.roles.as_ref().and_then(|r| r.seen_points) else {
            return Ok(None);
        };
        let Some(endpoint) = endpoint(players, role)? else {
            return Ok(None);
        };
        let Some(seen) = nfs_world::map_points::receive(endpoint, body, |id| {
            players.owns_participant(HOST_SELECTOR as u8, self.persona, id)
        })?
        else {
            return Ok(None);
        };
        let progression = self
            .progression
            .clone()
            .ok_or(replication::Error::Unsupported)?;
        let player_index = players
            .runtime_index(HOST_SELECTOR as u8, self.persona, seen.participant)
            .ok_or(replication::Error::UnknownObject)?;
        Ok(Some(Request {
            participant: seen.participant,
            player_index: u32::from(player_index),
            progression,
            action: Action::PoiSeen {
                endpoint,
                id: seen.id,
            },
        }))
    }

    pub(super) fn committed_map_points(
        &self,
        request: &Request,
        loaded: &crate::persistent::Loaded,
    ) -> Result<State, replication::Error> {
        let mut next = self.seen_points.clone();
        let Action::PoiSeen {
            endpoint: requested,
            id,
        } = request.action
        else {
            return Ok(next);
        };
        let ids = stored(loaded)?;
        if !ids.contains(&id) {
            return Err(replication::Error::Shape);
        }
        let current = self
            .roles
            .as_ref()
            .and_then(|r| r.seen_points)
            .map(|r| endpoint(&self.players, r))
            .transpose()?
            .flatten();
        if current == Some(requested) {
            next.bind(request.participant, requested);
            next.acknowledgements.insert(id);
        }
        // A replaced scene restores from the committed table, never from an
        // acknowledgement naming the retired scene.
        Ok(next)
    }

    pub(super) fn advance_map_points(&mut self) -> Result<(), replication::Error> {
        let (Some(role), Some(loaded)) = (
            self.roles.as_ref().and_then(|r| r.seen_points),
            self.persistent.as_deref(),
        ) else {
            return Ok(());
        };
        let Some(participant) = self.players.player_participants().find(|&p| {
            self.players
                .owns_participant(HOST_SELECTOR as u8, self.persona, p)
                && self.entered(p)
        }) else {
            return Ok(());
        };
        let Some(endpoint) = endpoint(&self.players, role)? else {
            return Ok(());
        };
        // Scene creations and ordered handoffs must precede these calls.
        if !self.pending_entries.is_empty()
            || !self.pending.is_empty()
            || self.session_backlog >= MAX_PENDING
        {
            return Ok(());
        }
        let ids = stored(loaded)?;
        let mut next = self.seen_points.clone();
        next.bind(participant, endpoint);
        let capacity = nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len());
        let calls = next.publish(&ids, capacity);
        for call in &calls {
            call.encode()?;
        }
        self.pending_rpcs.extend(calls);
        self.seen_points = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
