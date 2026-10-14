// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One car-owned leaf air-filter edit. Preparation is transient; commit updates
//! only the car's installed-child list, retaining purchased and default spares.
use crate::{inventory, purchase::Offers};
use nfs_storage::{AccountId, Batch, InventoryRepository, ItemId, Op, Snapshot, Timestamp};
use nfs_world_core::items::{Collection, DefinitionClass, Derived, InventoryCatalog, Item, OWNED};

const BATCH_NAMESPACE: u64 = 1 << 61;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    account: AccountId,
    generation: u64,
    before: Item,
    after: Item,
    part: Item,
}
impl Edit {
    pub fn vehicle(&self) -> u64 {
        self.before.id
    }
    pub fn item(&self) -> u64 {
        self.part.id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn preview(&self, items: &Collection) -> Result<Collection, Error> {
        if items.items.get(&self.before.id) != Some(&self.before)
            || items.items.get(&self.part.id) != Some(&self.part)
        {
            return Err(Error::Conflict);
        }
        let mut next = items.clone();
        next.items.insert(self.after.id, self.after.clone());
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Shape,
    Unsupported,
    Ownership,
    Conflict,
    Inventory(inventory::Error),
    Store(nfs_storage::Error),
}
impl From<inventory::Error> for Error {
    fn from(error: inventory::Error) -> Self {
        Self::Inventory(error)
    }
}
impl From<nfs_storage::Error> for Error {
    fn from(error: nfs_storage::Error) -> Self {
        Self::Store(error)
    }
}

/// The caller supplies the current account inventory and validates the live
/// player/garage-car binding before staging. No database or money changes here.
pub fn prepare(
    account: AccountId,
    generation: u64,
    items: &Collection,
    vehicle: u64,
    part: u64,
    catalog: &InventoryCatalog,
    offers: &Offers,
) -> Result<Edit, Error> {
    items
        .encode(catalog.bindings())
        .map_err(inventory::Error::from)?;
    let car = items.items.get(&vehicle).ok_or(Error::Ownership)?;
    let selected = items.items.get(&part).ok_or(Error::Ownership)?;
    if car.owner != 0
        || car.state != OWNED
        || !matches!(car.derived, Derived::RaceVehicle { .. })
        || selected.owner != vehicle
        || selected.state != OWNED
    {
        return Err(Error::Ownership);
    }
    if catalog
        .bindings()
        .class(&selected.definition)
        .map_err(inventory::Error::from)?
        != DefinitionClass::AirFilterItemData
        || selected.derived != Derived::Empty
        || !selected.children.is_empty()
        || !selected.defaults.is_empty()
        || (!offers
            .definitions()
            .any(|guid| *guid == selected.definition)
            && !car.defaults.contains(&part))
    {
        return Err(Error::Unsupported);
    }
    let mut installed = Vec::new();
    for &id in &car.children {
        let child = items.items.get(&id).ok_or(Error::Shape)?;
        if child.owner != vehicle || child.state != OWNED {
            return Err(Error::Shape);
        }
        if catalog
            .bindings()
            .class(&child.definition)
            .map_err(inventory::Error::from)?
            == DefinitionClass::AirFilterItemData
        {
            installed.push(id);
        }
    }
    if installed.len() != 1 {
        return Err(Error::Unsupported);
    }
    let mut after = car.clone();
    if installed[0] != part {
        // Native leaf replacement removes the previous class and appends the
        // selected item. The displaced part remains owned, available to re-equip.
        after.children.retain(|id| *id != installed[0]);
        after.children.push(part);
    }
    let edit = Edit {
        account,
        generation,
        before: car.clone(),
        after,
        part: selected.clone(),
    };
    edit.preview(items)?
        .encode(catalog.bindings())
        .map_err(inventory::Error::from)?;
    Ok(edit)
}

/// Commit with optimistic concurrency. Repeating an already committed edit is
/// a no-op. A different newer car state is never silently rebased or overwritten.
/// Purchase/cash is a separate previously committed transaction.
pub fn commit(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    edit: &Edit,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    if account != edit.account {
        return Err(Error::Ownership);
    }
    let snapshot = repository.snapshot(account)?;
    let current = inventory::collection(&snapshot, catalog.bindings())?;
    let vehicle = ItemId::new(edit.vehicle()).ok_or(Error::Shape)?;
    if snapshot
        .garage
        .is_none_or(|slots| !slots.0.contains(&Some(vehicle)))
        || current.items.get(&edit.part.id) != Some(&edit.part)
    {
        return Err(Error::Ownership);
    }
    if current.items.get(&edit.vehicle()) == Some(&edit.after) {
        return Ok(snapshot);
    }
    if snapshot.generation != edit.generation || snapshot.generation >= BATCH_NAMESPACE {
        return Err(Error::Conflict);
    }
    let preview = edit.preview(&current)?;
    let record = inventory::records(&preview, catalog.bindings())?
        .into_iter()
        .find(|record| record.id == vehicle)
        .ok_or(Error::Shape)?;
    let batch = Batch {
        id: BATCH_NAMESPACE | snapshot.generation,
        expected_generation: snapshot.generation,
        ops: vec![Op::Update(record)],
    };
    let applied = repository.apply(account, &batch, now)?;
    if applied.replayed {
        let current = repository.snapshot(account)?;
        let collection = inventory::collection(&current, catalog.bindings())?;
        return if collection.items.get(&edit.vehicle()) == Some(&edit.after)
            && collection.items.get(&edit.part.id) == Some(&edit.part)
        {
            Ok(current)
        } else {
            Err(Error::Conflict)
        };
    }
    let mut committed = snapshot.clone();
    committed.items = nfs_storage::port::transition(&snapshot.items, &batch)?;
    committed.generation = applied.generation;
    committed.updated_at = now;
    Ok(committed)
}

#[cfg(test)]
mod tests;
