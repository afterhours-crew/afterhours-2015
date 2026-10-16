// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed inventory boundary over the synchronous repository port.
//! The network edge must run these repository calls on a blocking worker.
//! Local account identity is supplied by configuration, never by an item id.
use nfs_storage::{
    AccountId, Batch, DefinitionGuid, GarageSlots, InventoryRepository, ItemId, ItemRecord, Op,
    Snapshot, Timestamp,
};
use nfs_world_core::items::{
    self, Catalog, Collection, Derived, Envelope, InventoryCatalog, Item, Request,
};

/// Reserved per-account retry key for the one initial inventory transaction.
/// Subsequent progression batches must use other keys.
pub const INITIAL_BATCH: u64 = 1;
/// Reserved one-time key for garage initialization on migrated profiles.
pub const INITIAL_GARAGE_BATCH: u64 = u64::MAX;

fn vehicle(record: &ItemRecord, catalog: &Catalog) -> Result<bool, Error> {
    Ok(record.owner.is_none()
        && record.state == items::OWNED
        && catalog.class(&record.definition.0)? == items::DefinitionClass::RaceVehicleItemData)
}

/// Local initial policy, not a claim about official fresh-account selection:
/// select the lowest owned root vehicle ID as primary; secondary slots empty.
fn initial_garage(snapshot: &Snapshot, catalog: &Catalog) -> Result<GarageSlots, Error> {
    let mut slots = GarageSlots::default();
    for record in snapshot.items.values() {
        if vehicle(record, catalog)? {
            slots.0[0] = Some(record.id);
            break;
        }
    }
    Ok(slots)
}

fn ensure_garage(
    repository: &dyn InventoryRepository,
    account: AccountId,
    mut snapshot: Snapshot,
    catalog: &Catalog,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    if snapshot.garage.is_none() {
        let batch = Batch {
            id: INITIAL_GARAGE_BATCH,
            expected_generation: snapshot.generation,
            ops: vec![Op::SetGarage(initial_garage(&snapshot, catalog)?)],
        };
        match repository.apply(account, &batch, now) {
            Ok(_) | Err(nfs_storage::Error::Conflict) => {}
            Err(error) => return Err(error.into()),
        }
        snapshot = repository.snapshot(account)?;
        if snapshot.garage.is_none() {
            return Err(nfs_storage::Error::Conflict.into());
        }
    }
    collection(&snapshot, catalog)?;
    Ok(snapshot)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Wire(items::Error),
    Store(nfs_storage::Error),
}
impl From<items::Error> for Error {
    fn from(e: items::Error) -> Self {
        Self::Wire(e)
    }
}
impl From<nfs_storage::Error> for Error {
    fn from(e: nfs_storage::Error) -> Self {
        Self::Store(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "inventory {self:?}")
    }
}
impl std::error::Error for Error {}

fn id(value: u64) -> Result<ItemId, Error> {
    ItemId::new(value).ok_or(Error::Store(nfs_storage::Error::Invalid))
}

/// Resolve every class-specific suffix and reject malformed/cyclic stored
/// graphs before serializing any reply. Unknown numeric item states survive.
pub fn collection(snapshot: &Snapshot, catalog: &Catalog) -> Result<Collection, Error> {
    nfs_storage::port::check_stored(&snapshot.items)?;
    if let Some(slots) = snapshot.garage {
        slots.validate_items(&snapshot.items)?;
        for id in slots.0.into_iter().flatten() {
            if !vehicle(&snapshot.items[&id], catalog)? {
                return Err(nfs_storage::Error::Config.into());
            }
        }
    }
    let mut result = Collection::default();
    for (key, record) in &snapshot.items {
        let class = catalog.class(&record.definition.0)?;
        let item = Item {
            id: key.get(),
            definition: record.definition.0,
            owner: record.owner.map_or(0, ItemId::get),
            defaults: record.default_items.iter().map(|id| id.get()).collect(),
            children: record.sub_items.iter().map(|id| id.get()).collect(),
            state: record.state,
            buy_price: record.buy_price,
            sell_price: record.sell_price,
            derived: Derived::decode(class.layout(), &record.derived)?,
        };
        result.items.insert(item.id, item);
    }
    result.roots = result.items.keys().copied().collect();
    result.encode(catalog)?;
    Ok(result)
}

/// Convert complete typed state, with both codec and repository bounds.
/// Wire item-reference bytes and the repository's internal byte order are
/// intentionally separated through ItemId, including IDs above i64::MAX.
pub fn records(collection: &Collection, catalog: &Catalog) -> Result<Vec<ItemRecord>, Error> {
    collection.encode(catalog)?;
    let mut stored = std::collections::BTreeMap::new();
    for item in collection.items.values() {
        let record = ItemRecord {
            id: id(item.id)?,
            definition: DefinitionGuid(item.definition),
            owner: if item.owner == 0 {
                None
            } else {
                Some(id(item.owner)?)
            },
            state: item.state,
            buy_price: item.buy_price,
            sell_price: item.sell_price,
            default_items: item
                .defaults
                .iter()
                .map(|value| id(*value))
                .collect::<Result<_, _>>()?,
            sub_items: item
                .children
                .iter()
                .map(|value| id(*value))
                .collect::<Result<_, _>>()?,
            derived: item.derived.encode()?,
        };
        record.validate()?;
        stored.insert(record.id, record);
    }
    nfs_storage::port::check_stored(&stored)?;
    Ok(stored.into_values().collect())
}

/// Initialize only generation-zero empty state. An intentionally emptied
/// inventory at a later generation stays empty. Racing first loads converge
/// on the committed repository state, without replacing existing progress.
pub fn load(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    first_id: u64,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    let snapshot = repository.open(account)?;
    if snapshot.generation != 0 || !snapshot.items.is_empty() {
        collection(&snapshot, catalog.bindings())?;
        return ensure_garage(repository, account, snapshot, catalog.bindings(), now);
    }
    let initial = catalog.instantiate_initial(first_id)?;
    let records = records(&initial.collection, catalog.bindings())?;
    let initial_snapshot = Snapshot {
        items: records.iter().cloned().map(|r| (r.id, r)).collect(),
        ..Snapshot::default()
    };
    let mut ops: Vec<_> = records.into_iter().map(Op::Insert).collect();
    ops.push(Op::SetGarage(initial_garage(
        &initial_snapshot,
        catalog.bindings(),
    )?));
    let batch = Batch {
        id: INITIAL_BATCH,
        expected_generation: 0,
        ops,
    };
    batch.validate()?;
    match repository.apply(account, &batch, now) {
        Ok(_) | Err(nfs_storage::Error::Conflict) => {}
        Err(error) => return Err(error.into()),
    }
    let snapshot = repository.snapshot(account)?;
    if snapshot.generation == 0 {
        return Err(nfs_storage::Error::Conflict.into());
    }
    collection(&snapshot, catalog.bindings())?;
    Ok(snapshot)
}

pub struct Reply {
    pub generation: u64,
    pub bytes: Vec<u8>,
    /// Same committed snapshot used to generate the inventory bytes.
    pub garage: [Option<u64>; 5],
    pub persistent: Option<std::sync::Arc<crate::persistent::Loaded>>,
}
impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InventoryReply")
            .field("generation", &self.generation)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Other operation bytes return None before opening or changing a profile.
/// The caller queues a successful reply only after this function returns;
/// new inventory has already committed before its bytes become available.
pub fn answer_load(
    request: &[u8],
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    first_id: u64,
    now: Timestamp,
) -> Result<Option<Reply>, Error> {
    answer_load_with_tables(request, repository, account, catalog, first_id, now, None)
}

/// Optional local table provider shares the final committed inventory snapshot.
pub fn answer_load_with_tables(
    request: &[u8],
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    first_id: u64,
    now: Timestamp,
    tables: Option<&crate::persistent::Catalog>,
) -> Result<Option<Reply>, Error> {
    answer_load_with_policy(
        request, repository, account, catalog, first_id, now, tables, None,
    )
}

/// The inventory and category mask share the final committed account snapshot.
#[allow(clippy::too_many_arguments)]
pub fn answer_load_with_policy(
    request: &[u8],
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    first_id: u64,
    now: Timestamp,
    tables: Option<&crate::persistent::Catalog>,
    unlocks: Option<&crate::category_unlocks::Policy>,
) -> Result<Option<Reply>, Error> {
    answer_load_with_unlocks(
        request, repository, account, catalog, first_id, now, tables, unlocks, None,
    )
}

/// Reconcile category and item grants before exposing the final inventory.
#[allow(clippy::too_many_arguments)]
pub fn answer_load_with_unlocks(
    request: &[u8],
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    first_id: u64,
    now: Timestamp,
    tables: Option<&crate::persistent::Catalog>,
    unlocks: Option<&crate::category_unlocks::Policy>,
    items: Option<&crate::item_unlocks::Policy>,
) -> Result<Option<Reply>, Error> {
    let request = Request::decode(request)?;
    if request.operation != items::LOAD_INVENTORY {
        return Ok(None);
    }
    let snapshot = load(repository, account, catalog, first_id, now)?;
    let (snapshot, persistent) = match tables {
        Some(tables) => {
            let (snapshot, loaded) = tables.ensure_loaded(repository, account, snapshot, now)?;
            (snapshot, Some(std::sync::Arc::new(loaded)))
        }
        None => (snapshot, None),
    };
    let snapshot = match unlocks {
        Some(policy) => policy.ensure(repository, account, catalog, snapshot, now)?,
        None => snapshot,
    };
    let snapshot = match items {
        Some(policy) => policy.ensure(repository, account, catalog, snapshot, now)?,
        None => snapshot,
    };
    let persistent = if unlocks.is_some() || items.is_some() {
        tables
            .map(|t| t.view(&snapshot).map(std::sync::Arc::new))
            .transpose()?
    } else {
        persistent
    };
    let collection = collection(&snapshot, catalog.bindings())?;
    let bytes =
        Envelope::from_collection(request.sequence, &collection, catalog.bindings())?.encode()?;
    Ok(Some(Reply {
        generation: snapshot.generation,
        bytes,
        persistent,
        garage: snapshot
            .garage
            .ok_or(nfs_storage::Error::Config)?
            .0
            .map(|id| id.map(ItemId::get)),
    }))
}

#[cfg(test)]
mod tests;
