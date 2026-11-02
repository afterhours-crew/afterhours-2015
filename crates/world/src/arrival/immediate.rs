// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An authored root-only arrival that starts stopped. Client completion is
//! still mandatory; its initial stop flag is not evidence that it has rendered.
use crate::{
    replication::{
        Error, Record,
        entity::{
            self,
            creation::{Asset, Content},
        },
        players::Players,
    },
    sequences::{
        Component, Owner,
        authored::{Blueprint, Instance, live_sequences},
    },
};

#[derive(Clone, Debug)]
pub struct Immediate {
    manager: Component,
    blueprint: Blueprint,
    content: Content,
}
impl Immediate {
    pub fn new(
        asset: Asset,
        manager: Component,
        embedded_bus: u32,
        bus_count: u16,
        content: Content,
    ) -> Result<Self, Error> {
        let blueprint = Blueprint::new(
            asset,
            embedded_bus,
            bus_count,
            &[entity::Kind::SequenceRoot],
        )?;
        if manager.scene_key == 0
            || manager.index >= crate::replication::sublevel::MAX_SERIALIZERS
            || content.profile(asset)? != blueprint.profile()
        {
            return Err(Error::Shape);
        }
        Ok(Self {
            manager,
            blueprint,
            content,
        })
    }
    pub(super) fn start(&self, players: &mut Players, owner: Owner) -> Result<Record, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        let world = players.objects();
        let manager = world
            .scene_endpoint(self.manager.scene_key, self.manager.index)
            .ok_or(Error::UnknownObject)?;
        let creation = self.blueprint.creation_with_stop(
            Instance {
                manager,
                participant: owner.participant,
                number: live_sequences(world, manager),
            },
            true,
            |_| Err(Error::Unsupported),
        )?;
        Ok(players
            .spawn_entities(vec![creation], &self.content)?
            .remove(0))
    }
}
