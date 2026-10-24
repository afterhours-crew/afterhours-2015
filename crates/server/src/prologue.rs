// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed story inputs shared by isolated player sessions. Account inventory
//! is never the storage for the temporary first-drive vehicle.
use crate::{
    sequence_content::authored::Authored,
    vehicle_content::{GarageContent, population::TemporaryVehicle},
};
use nfs_content::{
    assets::AssetIndex,
    prologue::{Start, sequences},
};
use nfs_world::items::InventoryCatalog;
use nfs_world::{
    items::Collection,
    replication::{Error, entity::creation::Asset},
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Content {
    pub start: Start,
    pub opening: Authored,
    items: Arc<Collection>,
}
impl Content {
    #[cfg(test)]
    pub(crate) fn for_test(
        items: Arc<Collection>,
        parent: Asset,
        manager: nfs_world::sequences::Component,
    ) -> Self {
        use nfs_world::{replication::entity::Kind, scoring::event_sync::Binding};
        let sequence = sequences::Sequence {
            blueprint: "test/opening".into(),
            asset: Asset {
                bundle: 30,
                type_id: 9,
                local_index: 0,
            },
            catalog: vec![(8, 1), (9, 1)],
            manager,
            embedded_bus: 2,
            bus_count: 1,
            fields: vec![Kind::SequenceRoot, Kind::RpcReference],
            channels: std::collections::BTreeMap::from([(
                1,
                nfs_content::participant_logic::Channel::Participant { serializer: 1 },
            )]),
        };
        Self {
            start: Start {
                objective: 42,
                vehicle_item: items.items[&1].definition,
                opening_blueprint: sequence.blueprint.clone(),
                opening_guid: [0; 16],
                teleport: nfs_content::prologue::Pose {
                    position: [10., 2., 30.],
                    basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                },
            },
            opening: Authored::new(
                sequence,
                parent,
                vec![Binding {
                    entity: 1,
                    event: 37,
                    node: 0,
                }],
            )
            .unwrap(),
            items,
        }
    }
    pub fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        items: &InventoryCatalog,
    ) -> Result<Self, nfs_content::Error> {
        let start = nfs_content::prologue::extract(index)?;
        let sequence = sequences::extract(index, &start.opening_blueprint)?;
        let stops = sequences::direct_stop_signals(index, &start.opening_blueprint)?;
        if stops.is_empty() {
            return Err(nfs_content::Error::Content(
                "opening has no authored completion signal".into(),
            ));
        }
        let opening = Authored::new(sequence, parent, stops)
            .map_err(|e| nfs_content::Error::Content(format!("opening construction: {e:?}")))?;
        let mut built = items
            .instantiate_vehicle(start.vehicle_item, 1)
            .map_err(|e| nfs_content::Error::Content(format!("intro vehicle: {e}")))?;
        // SubItems belong to the ephemeral vehicle; optional AdditionalItems
        // are not an account grant or equipped parts.
        built.collection.roots = built.collection.items.keys().copied().collect();
        Ok(Self {
            start,
            opening,
            items: Arc::new(built.collection),
        })
    }
    pub fn active(&self, restored: &crate::progression::Restored) -> bool {
        restored
            .branch(crate::progression::Branch::Intro)
            .objectives()
            .get(&self.start.objective)
            .is_some_and(|o| o.active && !o.completed)
    }
    pub fn temporary_vehicle(&self, content: &GarageContent) -> Result<TemporaryVehicle, Error> {
        let definition = content
            .vehicles
            .definition(&self.start.vehicle_item)
            .ok_or(Error::Unsupported)?;
        let (position, basis) = match content.layout.world_spawn() {
            Some(pose) => (pose.locator(), pose.basis()),
            None => {
                let slot = content.layout.slot(0).ok_or(Error::Shape)?;
                (
                    definition
                        .resting
                        .position_at(slot.locator())?
                        .map(|v| v.map_or(0., f32::from_bits)),
                    slot.basis(),
                )
            }
        };
        Ok(TemporaryVehicle {
            items: self.items.clone(),
            vehicle: 1,
            position,
            basis,
            blueprint_scene_key: content.layout.item_scene_key,
        })
    }
}
