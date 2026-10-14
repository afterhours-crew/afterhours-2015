// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned garage edits. Preparation is transient; one commit updates the
//! car, its selected installed parts and optional client-calculated dyno cache,
//! retaining purchased and default spares.
use crate::{inventory, purchase::Offers};
use nfs_storage::{AccountId, Batch, InventoryRepository, ItemId, Op, Snapshot, Timestamp};
use nfs_world_core::items::{Collection, Derived, InventoryCatalog, Item, OWNED};
use std::collections::{BTreeMap, BTreeSet};

const BATCH_NAMESPACE: u64 = 1 << 61;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    account: AccountId,
    generation: u64,
    before: Item,
    after: Item,
    selected: BTreeMap<u64, Item>,
    parts: BTreeMap<u64, (Item, Item)>,
}
impl Edit {
    pub fn vehicle(&self) -> u64 {
        self.before.id
    }
    pub fn selected(&self) -> impl Iterator<Item = u64> + '_ {
        self.selected.keys().copied()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn preview(&self, items: &Collection) -> Result<Collection, Error> {
        if items.items.get(&self.before.id) != Some(&self.before)
            || self
                .selected
                .values()
                .any(|part| items.items.get(&part.id) != Some(part))
        {
            return Err(Error::Conflict);
        }
        let mut next = items.clone();
        next.items.insert(self.after.id, self.after.clone());
        for (id, (before, after)) in &self.parts {
            if items.items.get(id) != Some(before) {
                return Err(Error::Conflict);
            }
            next.items.insert(*id, after.clone());
        }
        Ok(next)
    }
    pub fn with_dyno(mut self, stats: nfs_world_core::items::DynoStats) -> Result<Self, Error> {
        stats
            .apply(&mut self.after.derived)
            .map_err(|_| Error::Shape)?;
        Ok(self)
    }

    /// Apply leaf selections in request order. Different categories accumulate;
    /// later selections of the same category replace the earlier preview only.
    pub fn with_part(
        mut self,
        items: &Collection,
        part: u64,
        catalog: &InventoryCatalog,
        offers: &Offers,
    ) -> Result<Self, Error> {
        let preview = self.preview(items)?;
        let selected = preview.items.get(&part).ok_or(Error::Ownership)?;
        if selected.owner != self.vehicle() || selected.state != OWNED {
            return Err(Error::Ownership);
        }
        let class = catalog
            .bindings()
            .class(&selected.definition)
            .map_err(inventory::Error::from)?;
        if !crate::purchase::performance_class(class)
            || !selected.children.is_empty()
            || !selected.defaults.is_empty()
            || (!offers
                .definitions()
                .any(|guid| *guid == selected.definition)
                && !self.before.defaults.contains(&part))
        {
            return Err(Error::Unsupported);
        }
        let mut installed = Vec::new();
        for &id in &self.after.children {
            let child = preview.items.get(&id).ok_or(Error::Shape)?;
            if child.owner != self.vehicle() || child.state != OWNED {
                return Err(Error::Ownership);
            }
            if catalog
                .bindings()
                .class(&child.definition)
                .map_err(inventory::Error::from)?
                == class
            {
                installed.push(id);
            }
        }
        if installed.len() > 1 {
            return Err(Error::Unsupported);
        }
        if installed.first() != Some(&part) {
            self.after.children.retain(|id| !installed.contains(id));
            self.after.children.push(part);
        }
        self.selected
            .insert(part, items.items.get(&part).ok_or(Error::Shape)?.clone());
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }

    fn saved(&self, items: &Collection) -> bool {
        items.items.get(&self.after.id) == Some(&self.after)
            && self
                .parts
                .iter()
                .all(|(id, (_, after))| items.items.get(id) == Some(after))
    }

    /// Resolve current children in preorder; defaults and owned spares are not
    /// installed targets. All edited parts and the car commit in one batch.
    pub fn with_handling(
        mut self,
        items: &Collection,
        catalog: &InventoryCatalog,
        changes: nfs_world_core::items::HandlingChanges,
        transmission: Option<[u8; 2]>,
    ) -> Result<Self, Error> {
        let preview = self.preview(items)?;
        let mut installed = Vec::new();
        let mut visited = BTreeSet::new();
        let mut pending = vec![(self.vehicle(), 0, 0)];
        while let Some((id, owner, depth)) = pending.pop() {
            if depth > nfs_world_core::items::MAX_DEPTH || !visited.insert(id) {
                return Err(Error::Shape);
            }
            let item = preview.items.get(&id).ok_or(Error::Shape)?;
            if item.owner != owner || item.state != OWNED {
                return Err(Error::Ownership);
            }
            installed.push((
                id,
                catalog
                    .bindings()
                    .class(&item.definition)
                    .map_err(inventory::Error::from)?,
            ));
            pending.extend(
                item.children
                    .iter()
                    .rev()
                    .map(|&child| (child, id, depth + 1)),
            );
        }
        for (category, value) in changes.iter() {
            let class = nfs_world_core::items::HandlingChanges::class(category)
                .map_err(|_| Error::Shape)?;
            let id = installed
                .iter()
                .find(|(_, kind)| *kind == class)
                .map(|(id, _)| *id)
                .ok_or(Error::Unsupported)?;
            if category == 30 {
                let values = transmission.ok_or(Error::Unsupported)?;
                if !values.contains(&value) {
                    return Err(Error::Unsupported);
                }
                let Derived::RaceVehicle {
                    vehicle_item_flags, ..
                } = &mut self.after.derived
                else {
                    return Err(Error::Shape);
                };
                *vehicle_item_flags =
                    (*vehicle_item_flags & !4) | if value == values[0] { 4 } else { 0 };
            } else {
                let part = preview.items.get(&id).ok_or(Error::Shape)?;
                let (_, next) = self
                    .parts
                    .entry(id)
                    .or_insert_with(|| (part.clone(), part.clone()));
                nfs_world_core::items::HandlingChanges::apply(category, value, &mut next.derived)
                    .map_err(|_| Error::Shape)?;
            }
        }
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
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
    prepare_vehicle(account, generation, items, vehicle, catalog)?
        .with_part(items, part, catalog, offers)
}

/// A dyno result can arrive without changing the selected part. It is still
/// bound to an owned garage car and the exact inventory generation it measured.
pub fn prepare_dyno(
    account: AccountId,
    generation: u64,
    items: &Collection,
    vehicle: u64,
    stats: nfs_world_core::items::DynoStats,
    catalog: &InventoryCatalog,
) -> Result<Edit, Error> {
    prepare_vehicle(account, generation, items, vehicle, catalog)?.with_dyno(stats)
}

pub fn prepare_vehicle(
    account: AccountId,
    generation: u64,
    items: &Collection,
    vehicle: u64,
    catalog: &InventoryCatalog,
) -> Result<Edit, Error> {
    items
        .encode(catalog.bindings())
        .map_err(inventory::Error::from)?;
    let car = items.items.get(&vehicle).ok_or(Error::Ownership)?;
    if car.owner != 0 || car.state != OWNED || !matches!(car.derived, Derived::RaceVehicle { .. }) {
        return Err(Error::Ownership);
    }
    Ok(Edit {
        account,
        generation,
        before: car.clone(),
        after: car.clone(),
        selected: BTreeMap::new(),
        parts: BTreeMap::new(),
    })
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
        || edit.selected.values().any(|part| {
            !edit.parts.contains_key(&part.id) && current.items.get(&part.id) != Some(part)
        })
    {
        return Err(Error::Ownership);
    }
    if edit.saved(&current) {
        return Ok(snapshot);
    }
    if snapshot.generation != edit.generation || snapshot.generation >= BATCH_NAMESPACE {
        return Err(Error::Conflict);
    }
    let preview = edit.preview(&current)?;
    let records = inventory::records(&preview, catalog.bindings())?
        .into_iter()
        .filter(|record| record.id == vehicle || edit.parts.contains_key(&record.id.get()))
        .filter(|record| snapshot.items.get(&record.id) != Some(record))
        .map(Op::Update)
        .collect();
    let batch = Batch {
        id: BATCH_NAMESPACE | snapshot.generation,
        expected_generation: snapshot.generation,
        ops: records,
    };
    let applied = repository.apply(account, &batch, now)?;
    if applied.replayed {
        let current = repository.snapshot(account)?;
        let collection = inventory::collection(&current, catalog.bindings())?;
        return if edit.saved(&collection)
            && edit.selected.values().all(|part| {
                edit.parts.contains_key(&part.id) || collection.items.get(&part.id) == Some(part)
            }) {
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
