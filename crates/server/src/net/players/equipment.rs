// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_services::item_builder::{Binding, Operation};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(in crate::net) struct EquipmentRequest {
    pub player: u16,
    pub vehicle: u16,
    pub car: u64,
    pub item: Option<u64>,
    pub dyno: Option<nfs_world::items::DynoStats>,
    pub player_index: u32,
    pub generation: u64,
    pub items: Arc<nfs_world::items::Collection>,
}
#[derive(Clone, Debug, Default)]
pub(super) struct State {
    edits: BTreeMap<u16, EquipmentRequest>,
    committed: VecDeque<EquipmentRequest>,
}

impl PlayerListener {
    pub(in crate::net) fn take_equipment(&mut self) -> Option<EquipmentRequest> {
        self.equipment.committed.pop_front()
    }
    pub(super) fn stage_equipment(
        &self,
        players: &Players,
        body: nfs_protocol::world::BitSpan<'_>,
        state: &mut State,
    ) -> Result<bool, replication::Error> {
        let Some(roles) = &self.roles else {
            return Ok(false);
        };
        let Some(scene) = players.objects().scene(roles.garage) else {
            return Ok(false);
        };
        let Some(replication::Initial::SubLevel { fields, .. }) =
            players.objects().get(scene).map(|o| &o.initial)
        else {
            return Ok(false);
        };
        let Some(rpc) = fields.get(3).and_then(replication::sublevel::Initial::rpc) else {
            return Ok(false);
        };
        let call = Binding::new(scene, rpc.selector)
            .map_err(|_| replication::Error::Shape)?
            .read(body, |player| {
                players.owns(HOST_SELECTOR as u8, self.persona, player)
            })
            .map_err(|_| replication::Error::Shape)?;
        let Some(call) = call else { return Ok(false) };
        match call.operation {
            Operation::Begin => {
                state.edits.remove(&call.player);
            }
            Operation::Equip { vehicle, .. } | Operation::Dyno { vehicle, .. } => {
                let (_, car) = players
                    .player_vehicle_item(HOST_SELECTOR as u8, self.persona, call.player, vehicle)
                    .ok_or(replication::Error::UnknownObject)?;
                if self.world_cars.contains_key(&vehicle) {
                    return Err(replication::Error::Shape);
                }
                let inventory = self.inventory.as_ref().ok_or(replication::Error::Shape)?;
                let loaded = self.persistent.as_ref().ok_or(replication::Error::Shape)?;
                if !inventory.slots.values().contains(&Some(car)) {
                    return Err(replication::Error::UnknownObject);
                }
                let Some(replication::Initial::Player(player)) =
                    players.objects().get(call.player).map(|o| &o.initial)
                else {
                    return Err(replication::Error::UnknownObject);
                };
                if let Operation::Equip { item, .. } = call.operation
                    && !inventory
                        .items
                        .items
                        .get(&item)
                        .is_some_and(|p| p.owner == car)
                {
                    return Err(replication::Error::UnknownObject);
                }
                if !state.edits.contains_key(&call.player) && state.edits.len() >= 16 {
                    return Err(replication::Error::Bound);
                }
                let edit = state
                    .edits
                    .entry(call.player)
                    .or_insert_with(|| EquipmentRequest {
                        player: call.player,
                        vehicle,
                        car,
                        item: None,
                        dyno: None,
                        player_index: u32::from(player.runtime_index),
                        generation: loaded.generation(),
                        items: inventory.items.clone(),
                    });
                if edit.car != car
                    || edit.vehicle != vehicle
                    || edit.generation != loaded.generation()
                {
                    return Err(replication::Error::Shape);
                }
                match call.operation {
                    Operation::Equip { item, .. } => {
                        if edit.item != Some(item) {
                            edit.dyno = None;
                        }
                        edit.item = Some(item);
                    }
                    Operation::Dyno { stats, .. } => edit.dyno = Some(stats),
                    _ => unreachable!(),
                }
            }
            Operation::Commit => {
                if let Some(edit) = state.edits.remove(&call.player) {
                    if state.committed.len() >= 16 {
                        return Err(replication::Error::Bound);
                    }
                    state.committed.push_back(edit);
                }
            }
        }
        Ok(true)
    }

    pub(in crate::net) fn equipped_inventory(
        &mut self,
        request: &EquipmentRequest,
        loaded: Arc<crate::persistent::Loaded>,
        items: Arc<nfs_world::items::Collection>,
    ) -> Result<(), replication::Error> {
        let old = self.inventory.as_ref().ok_or(replication::Error::Shape)?;
        let before = self.persistent.as_ref().ok_or(replication::Error::Shape)?;
        if self
            .players
            .player_vehicle_item(
                HOST_SELECTOR as u8,
                self.persona,
                request.player,
                request.vehicle,
            )
            .is_none_or(|(_, car)| car != request.car)
            || self.world_cars.contains_key(&request.vehicle)
            || loaded.garage() != old.slots.values()
            || loaded.generation() < before.generation()
            || loaded.balance() != before.balance()
            || loaded.tables() != before.tables()
            || loaded.stores() != before.stores()
            || old.items.items.len() != items.items.len()
            || old
                .items
                .items
                .iter()
                .any(|(id, item)| *id != request.car && items.items.get(id) != Some(item))
        {
            return Err(replication::Error::Shape);
        }
        let content = self
            .vehicles
            .as_ref()
            .ok_or(replication::Error::Unsupported)?;
        let custom = content.vehicles.customization(&items, request.car)?;
        let mut players = self.players.clone();
        let record = players.set_vehicle_customization(
            HOST_SELECTOR as u8,
            self.persona,
            request.vehicle,
            custom,
        )?;
        if record.is_some() && self.pending.len() >= MAX_PENDING {
            return Err(replication::Error::Bound);
        }
        let inventory = Inventory {
            slots: old.slots,
            items,
        };
        let mut population = self.population.clone();
        if let Some(population) = &mut population {
            population.refresh_inventory(
                &players,
                HOST_SELECTOR as u8,
                self.persona,
                old,
                &inventory,
            )?;
        }
        self.players = players;
        self.population = population;
        self.inventory = Some(inventory);
        self.persistent = Some(loaded);
        if let Some(record) = record {
            self.pending.push_back(Section {
                float_bits: None,
                flag: false,
                deleted: vec![],
                setup: Some(Setup::Packed {
                    tag: None,
                    width: None,
                    axes: [None; 3],
                }),
                records: vec![record],
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
