// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Reason, apply, ownership::slots};
use crate::inventory;
use nfs_storage::{AccountId, InventoryRepository, ItemId, Op, Snapshot, Timestamp};
use nfs_world_core::items::{self, DefinitionClass, InventoryCatalog};

/// Local warehouse policy, also advertised in bootstrap configuration.
pub const WAREHOUSE_CAPACITY: usize = 100;

fn vehicle(snapshot: &Snapshot, catalog: &InventoryCatalog, item: u64) -> Result<ItemId, Error> {
    let id = ItemId::new(item).ok_or(Error::Rejected(Reason::Shape))?;
    let record = snapshot
        .items
        .get(&id)
        .ok_or(Error::Rejected(Reason::Owner))?;
    if record.owner.is_some()
        || record.state != items::OWNED
        || catalog
            .bindings()
            .class(&record.definition.0)
            .map_err(inventory::Error::from)?
            != DefinitionClass::RaceVehicleItemData
    {
        return Err(Error::Rejected(Reason::Owner));
    }
    Ok(id)
}

fn slot(secondary: u8) -> Result<usize, Error> {
    if secondary >= 4 {
        return Err(Error::Rejected(Reason::Shape));
    }
    Ok(usize::from(secondary) + 1)
}

/// Unbind a parked car without changing any inventory record or money. An owned
/// car outside the five garage bindings is in the warehouse. Resolve its identity
/// when staging the command so an old request cannot clear a reused slot.
pub fn move_to_warehouse(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    item: u64,
    secondary: u8,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    let destination = slot(secondary)?;
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        let mut garage = slots(&snapshot, catalog)?;
        let target = vehicle(&snapshot, catalog, item)?;
        if !garage.0.contains(&Some(target)) {
            return Ok(snapshot);
        }
        if garage.0[0] == Some(target) {
            return Err(Error::Rejected(Reason::Primary));
        }
        if garage.0[destination] != Some(target) {
            return Err(nfs_storage::Error::Conflict.into());
        }
        let mut stored = 0;
        for record in snapshot.items.values() {
            if record.owner.is_none()
                && record.state == items::OWNED
                && !garage.0.contains(&Some(record.id))
                && catalog
                    .bindings()
                    .class(&record.definition.0)
                    .map_err(inventory::Error::from)?
                    == DefinitionClass::RaceVehicleItemData
            {
                stored += 1;
            }
        }
        if stored >= WAREHOUSE_CAPACITY {
            return Err(Error::Rejected(Reason::Full));
        }
        garage.0[destination] = None;
        if apply(
            repository,
            account,
            &snapshot,
            vec![Op::SetGarage(garage)],
            now,
        )? {
            return Ok(repository.snapshot(account)?);
        }
    }
    Err(nfs_storage::Error::Conflict.into())
}

/// Retrieve into an empty parked slot or exchange with its current car. The
/// displaced car remains owned in the warehouse, so a swap needs no extra space.
/// Retain the prior occupant from staging to reject a stale replacement.
pub fn take_from_warehouse(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    item: u64,
    secondary: u8,
    previous: Option<u64>,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    let destination = slot(secondary)?;
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        let mut garage = slots(&snapshot, catalog)?;
        let target = vehicle(&snapshot, catalog, item)?;
        if garage.0[destination] == Some(target) {
            return Ok(snapshot);
        }
        if garage.0[0] == Some(target) {
            return Err(Error::Rejected(Reason::Primary));
        }
        if garage.0.contains(&Some(target)) {
            return Err(Error::Rejected(Reason::Owner));
        }
        if garage.0[destination].map(ItemId::get) != previous {
            return Err(nfs_storage::Error::Conflict.into());
        }
        garage.0[destination] = Some(target);
        if apply(
            repository,
            account,
            &snapshot,
            vec![Op::SetGarage(garage)],
            now,
        )? {
            return Ok(repository.snapshot(account)?);
        }
    }
    Err(nfs_storage::Error::Conflict.into())
}
