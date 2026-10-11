// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::customization_timer::Owner;
use nfs_world::{logic::Message, replication::entity};

impl PlayerListener {
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
        let (&participant, _) = candidates.next().ok_or(replication::Error::UnknownObject)?;
        if candidates.next().is_some() {
            return Err(replication::Error::DuplicateObject);
        }
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
