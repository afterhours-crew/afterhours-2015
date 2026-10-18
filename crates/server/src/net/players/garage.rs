// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_services::garage::{Binding, Operation};
use nfs_world::{garage::Slots, participants::HostRpc};
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::net) enum GarageOperation {
    Select {
        item: u64,
        previous_primary: u64,
    },
    Place {
        item: u64,
        secondary: u8,
    },
    Sold {
        secondary: u8,
    },
    MoveToWarehouse {
        item: u64,
        secondary: u8,
    },
    TakeFromWarehouse {
        item: u64,
        secondary: u8,
        previous: Option<u64>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::net) struct GarageRequest {
    pub participant: u16,
    pub operation: GarageOperation,
}
impl PlayerListener {
    pub(in crate::net) fn garage_offer_notifications(
        &self,
        after: &nfs_world::items::Collection,
        catalog: &nfs_world::items::Catalog,
    ) -> Result<Vec<Vec<u8>>, replication::Error> {
        let before = &self
            .inventory
            .as_ref()
            .ok_or(replication::Error::Shape)?
            .items;
        let added = after
            .items
            .iter()
            .filter(|(id, item)| {
                !before.items.contains_key(id)
                    && item.state == nfs_world::items::PURCHASABLE
                    && item.owner == 0
            })
            .map(|(&id, _)| id)
            .collect::<Vec<_>>();
        if added.is_empty() {
            return Ok(Vec::new());
        }
        let index = self
            .players
            .account_runtime_index(HOST_SELECTOR as u8, self.persona)
            .ok_or(replication::Error::UnknownObject)?;
        Ok(vec![
            nfs_world::items::ChangeNotification {
                player_index: u32::from(index),
                added: after
                    .for_roots(added)
                    .map_err(|_| replication::Error::Shape)?,
                removed: Default::default(),
            }
            .encode(catalog)
            .map_err(|_| replication::Error::Shape)?,
        ])
    }
    pub(in crate::net) fn garage_ready(&self) -> bool {
        let participants = self.participants.in_customization();
        self.purchase_ready()
            && !participants.is_empty()
            && participants.iter().all(|&p| {
                self.players
                    .owns_participant(HOST_SELECTOR as u8, self.persona, p)
                    && self
                        .population
                        .as_ref()
                        .is_some_and(|population| population.all_loaded(p))
                    && !self.world_cars.values().any(|(owner, _)| *owner == p)
            })
    }
    pub(in crate::net) fn take_garage(&mut self) -> Option<GarageRequest> {
        if !self.garage_ready() {
            return None;
        }
        self.garage_commands.pop_front()
    }
    pub(super) fn stage_garage(
        &self,
        players: &Players,
        body: nfs_protocol::world::BitSpan<'_>,
        pending: &mut VecDeque<GarageRequest>,
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
        let Some(index) = self
            .vehicles
            .as_ref()
            .and_then(|content| content.layout.garage_component_index)
        else {
            return Ok(false);
        };
        let Some(rpc) = fields
            .get(index)
            .and_then(replication::sublevel::Initial::rpc)
        else {
            return Ok(false);
        };
        let Some(call) = Binding::new(scene, rpc.selector)
            .map_err(|_| replication::Error::Shape)?
            .read(body, |participant| {
                players.owns_participant(HOST_SELECTOR as u8, self.persona, participant)
            })
            .map_err(|_| replication::Error::Shape)?
        else {
            return Ok(false);
        };
        let participant = call.participant;
        if !self.participants.in_customization().contains(&participant) {
            return Err(replication::Error::Shape);
        }
        let slots = self
            .inventory
            .as_ref()
            .ok_or(replication::Error::Shape)?
            .slots
            .values();
        let operation = match call.operation {
            Operation::Select { secondary } => GarageOperation::Select {
                item: slots[usize::from(secondary) + 1].ok_or(replication::Error::UnknownObject)?,
                previous_primary: slots[0].ok_or(replication::Error::UnknownObject)?,
            },
            Operation::Place { secondary, item } => GarageOperation::Place { secondary, item },
            Operation::Sold { secondary } => GarageOperation::Sold { secondary },
            Operation::MoveToWarehouse { secondary } => {
                let Some(item) = slots[usize::from(secondary) + 1] else {
                    // A repeated clear of an empty slot has no effect.
                    return Ok(true);
                };
                GarageOperation::MoveToWarehouse { item, secondary }
            }
            Operation::TakeFromWarehouse { secondary, item } => {
                GarageOperation::TakeFromWarehouse {
                    item,
                    secondary,
                    previous: slots[usize::from(secondary) + 1],
                }
            }
        };
        let request = GarageRequest {
            participant,
            operation,
        };
        if !pending.contains(&request) {
            if pending.len() >= 16 {
                return Err(replication::Error::Bound);
            }
            pending.push_back(request);
        }
        Ok(true)
    }

    /// The worker supplies the exact pre-transaction inventory. This keeps a
    /// late result from replacing a more recent local selection or edit.
    pub(in crate::net) fn garage_inventory(
        &mut self,
        previous: &Inventory,
        garage: [Option<u64>; 5],
        loaded: Arc<crate::persistent::Loaded>,
        items: Arc<nfs_world::items::Collection>,
    ) -> Result<(), replication::Error> {
        let old = self.inventory.as_ref().ok_or(replication::Error::Shape)?;
        let before = self.persistent.as_ref().ok_or(replication::Error::Shape)?;
        let slots = Slots::new(garage)?;
        if old.slots != previous.slots
            || old.items != previous.items
            || loaded.garage() != garage
            || loaded.generation() < before.generation()
            || loaded
                .tables()
                .iter()
                .filter(|(key, _)| **key != nfs_services::persistent::key("GarageItemsTable"))
                .ne(before
                    .tables()
                    .iter()
                    .filter(|(key, _)| **key != nfs_services::persistent::key("GarageItemsTable")))
            || loaded.stores() != before.stores()
        {
            return Err(replication::Error::Shape);
        }
        let inventory = Inventory { slots, items };
        let mut players = self.players.clone();
        let mut participants = self.participants.clone();
        let mut population = self.population.clone().ok_or(replication::Error::Shape)?;
        let mut output = crate::vehicle_content::population::Output::default();
        let mut rpcs = Vec::new();
        if slots != old.slots {
            let content = self.vehicles.as_ref().ok_or(replication::Error::Shape)?;
            for participant in participants.in_customization() {
                if !players.owns_participant(HOST_SELECTOR as u8, self.persona, participant) {
                    continue;
                }
                let changed = population.reconcile(
                    &mut players,
                    crate::vehicle_content::population::Owner {
                        connection: HOST_SELECTOR as u8,
                        persona: self.persona,
                        participant,
                    },
                    old,
                    inventory.clone(),
                    &content.vehicles,
                    &content.layout,
                )?;
                output.deleted.extend(changed.deleted);
                output.records.extend(changed.records);
                output.messages.extend(changed.messages);
                output.bindings.extend(changed.bindings);
            }
            rpcs.extend(
                participants
                    .replace_garage(old.slots, slots, |p| {
                        players.owns_participant(HOST_SELECTOR as u8, self.persona, p)
                    })?
                    .into_iter()
                    .map(HostRpc::Garage),
            );
            rpcs.extend(output.bindings.into_iter().map(HostRpc::Vehicle));
        } else {
            population.refresh_inventory(
                &players,
                HOST_SELECTOR as u8,
                self.persona,
                old,
                &inventory,
            )?;
        }
        if let Some(bank) = self.bank_notification(loaded.balance().0)? {
            rpcs.push(bank);
        }
        let has_section = !output.records.is_empty() || !output.deleted.is_empty();
        if (has_section && self.pending.len() >= MAX_PENDING)
            || output.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || rpcs.len()
                > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
        {
            return Err(replication::Error::Bound);
        }
        if old.slots != slots {
            self.equipment.discard_previews();
        }
        self.players = players;
        self.participants = participants;
        self.population = Some(population);
        self.inventory = Some(inventory);
        self.persistent = Some(loaded);
        self.pending_content.extend(output.messages);
        self.pending_rpcs.extend(rpcs);
        if has_section {
            self.pending.push_back(Section {
                float_bits: None,
                flag: false,
                deleted: output.deleted,
                setup: (!output.records.is_empty()).then_some(Setup::Packed {
                    tag: None,
                    width: None,
                    axes: [None; 3],
                }),
                records: output.records,
            });
        }
        Ok(())
    }
}
