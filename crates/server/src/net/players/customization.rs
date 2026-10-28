// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::customization_timer::Owner;
use nfs_world::{logic::Message, replication::entity};

impl PlayerListener {
    pub(super) fn livery_cache_update(
        roles: Option<&crate::scene_roles::SceneRoles>,
        players: &Players,
        persona: u64,
        body: nfs_protocol::world::BitSpan<'_>,
    ) -> Result<Option<nfs_world::garage::livery::Invalidation>, replication::Error> {
        use nfs_world::{
            garage::livery,
            participants::Endpoint,
            replication::{Initial, sublevel},
        };
        let Some(roles) = roles else { return Ok(None) };
        let Some(scene) = players.objects().scene(roles.garage) else {
            return Ok(None);
        };
        let Some(Initial::SubLevel { fields, .. }) =
            players.objects().get(scene).map(|o| &o.initial)
        else {
            return Ok(None);
        };
        let Some(sublevel::Initial::Rpc(rpc)) = fields.get(livery::COMPONENT) else {
            return Ok(None);
        };
        livery::receive(
            Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            },
            body,
            |claimed| {
                claimed == persona
                    && players
                        .player_participants()
                        .any(|p| players.owns_participant(HOST_SELECTOR as u8, persona, p))
            },
        )
    }

    pub(super) fn item_builder_begin(
        roles: Option<&crate::scene_roles::SceneRoles>,
        players: &Players,
        persona: u64,
        body: nfs_protocol::world::BitSpan<'_>,
    ) -> Result<bool, replication::Error> {
        use nfs_world::replication::{Initial, sublevel};
        let Some(roles) = roles else { return Ok(false) };
        let Some(scene) = players.objects().scene(roles.garage) else {
            return Ok(false);
        };
        let Some(Initial::SubLevel { fields, .. }) =
            players.objects().get(scene).map(|o| &o.initial)
        else {
            return Ok(false);
        };
        let Some(sublevel::Initial::Rpc(rpc)) = fields.get(3) else {
            return Ok(false);
        };
        let binding = nfs_services::item_builder::Binding::new(scene, rpc.selector)
            .map_err(|_| replication::Error::Shape)?;
        binding
            .begin_update(body, |player| {
                players.owns(HOST_SELECTOR as u8, persona, player)
            })
            .map_err(|error| match error {
                nfs_services::item_builder::Error::Ownership => replication::Error::UnknownObject,
                _ => replication::Error::Shape,
            })
    }
    /// Whether a lookup error only means the message names no object this
    /// connection owns: logged and consumed, never a refused frame (a refused
    /// reliable message is resent forever and jams the channel, E1252).
    pub(super) fn is_unowned(error: replication::Error) -> bool {
        matches!(
            error,
            replication::Error::UnknownObject
                | replication::Error::TypeMismatch
                | replication::Error::DuplicateObject
        )
    }
    pub(super) fn unowned_logic(message: &Message, error: replication::Error) {
        if let Message::Reached { event, target, .. } = message {
            tracing::warn!(
                ghost = target.ghost,
                entity = target.entity,
                event,
                ?error,
                "client logic message names no owned object; consumed"
            );
        }
    }
    pub(super) fn customization_owner(
        roles: Option<&crate::scene_roles::SceneRoles>,
        players: &Players,
        ghosts: &BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        message: &Message,
    ) -> Result<Owner, replication::Error> {
        let Message::Reached { target, .. } = message else {
            return Err(replication::Error::Shape);
        };
        let mut candidates = ghosts
            .iter()
            .filter(|(_, map)| map.values().any(|g| *g == target.ghost));
        let participant = match (candidates.next(), candidates.next()) {
            (Some((&participant, _)), None) => participant,
            (Some(_), Some(_)) => return Err(replication::Error::DuplicateObject),
            // A timer the garage logic did not create (the intro's final
            // garage presentation, E1252) is owned by its root's participant;
            // the type and ownership checks below still apply.
            (None, _) => match &players
                .objects()
                .get(target.ghost)
                .ok_or(replication::Error::UnknownObject)?
                .initial
            {
                replication::Initial::Entity { fields, .. } => match fields.first() {
                    Some(entity::Initial::Root { reference, .. }) => *reference,
                    _ => return Err(replication::Error::TypeMismatch),
                },
                _ => return Err(replication::Error::TypeMismatch),
            },
        };
        let local_slot = players
            .local_slot(HOST_SELECTOR as u8, persona, participant)
            .ok_or(replication::Error::UnknownObject)?;
        if !matches!(&players.objects().get(target.ghost).ok_or(replication::Error::UnknownObject)?.initial,
            replication::Initial::Entity { prefix, fields } if prefix.asset == roles.ok_or(replication::Error::Unsupported)?.customization_timer
                && matches!(fields.first(), Some(entity::Initial::Root { reference, .. }) if *reference == participant))
        {
            return Err(replication::Error::TypeMismatch);
        }
        Ok(Owner {
            participant,
            ghost: target.ghost,
            local_slot,
        })
    }
}
