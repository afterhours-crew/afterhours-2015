// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{CommandId, Error, Reason, apply, ledger};
use crate::{bank::Balance, inventory};
use nfs_storage::{AccountId, GarageSlots, InventoryRepository, ItemId, Op, Snapshot, Timestamp};
use nfs_world_core::items::{
    self, Collection, DefinitionClass, Guid, InventoryCatalog, Item, PurchaseRequest,
    PurchaseSelection, SellRequest,
};
use std::collections::{BTreeMap, BTreeSet};

/// Only explicitly supported installed car definitions and their REP thresholds.
/// Non-purchasable story, reward and traffic vehicles never become offers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Offers(BTreeMap<Guid, i32>);
impl Offers {
    pub fn new(
        catalog: &InventoryCatalog,
        entries: impl IntoIterator<Item = (Guid, i32)>,
    ) -> Result<Self, Error> {
        let mut offers = BTreeMap::new();
        for (guid, score) in entries {
            let d = catalog.definition(&guid).map_err(inventory::Error::from)?;
            if score < 0
                || offers.len() >= 512
                || d.class != DefinitionClass::RaceVehicleItemData
                || !d.flags.purchasable
                || d.flags.deprecated
                || d.flags.requires_owner
                || !d.flags.uses_scope
                || !matches!(d.quantity, 1 | u32::MAX)
                || d.default_derived.is_none()
                || offers.insert(guid, score).is_some()
            {
                return Err(Error::Rejected(Reason::Offer));
            }
            for extra in &d.additional_items {
                let e = catalog.definition(extra).map_err(inventory::Error::from)?;
                if e.class != DefinitionClass::LiveryCustomizationItemData
                    || e.flags.requires_owner
                    || e.flags.deprecated
                    || !e.sub_items.is_empty()
                    || !e.additional_items.is_empty()
                    || e.default_derived.is_none()
                {
                    return Err(Error::Rejected(Reason::Offer));
                }
            }
            // Reject unresolved stock constructors at content load, not after a debit.
            catalog
                .instantiate_vehicle(guid, 1)
                .map_err(inventory::Error::from)?;
        }
        Ok(Self(offers))
    }
    pub fn entries(&self) -> impl Iterator<Item = (&Guid, &i32)> {
        self.0.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Changed {
    pub snapshot: Snapshot,
    /// Purchased or removed records, including all required nested references.
    pub items: Collection,
    pub replayed: bool,
}
fn id(value: u64) -> Result<ItemId, Error> {
    ItemId::new(value).ok_or(Error::Rejected(Reason::Shape))
}
fn slots(snapshot: &Snapshot, catalog: &InventoryCatalog) -> Result<GarageSlots, Error> {
    inventory::collection(snapshot, catalog.bindings())?;
    let slots = snapshot.garage.ok_or(Error::Rejected(Reason::Owner))?;
    if slots.0[0].is_none() {
        return Err(Error::Rejected(Reason::Owner));
    }
    Ok(slots)
}
fn selected(
    request: &PurchaseRequest,
    snapshot: &Snapshot,
) -> Result<(Guid, Option<ItemId>), Error> {
    if request.owner != 0 || request.sequence < 2 {
        return Err(Error::Rejected(Reason::Shape));
    }
    match &request.selection {
        PurchaseSelection::Definitions(guids) if guids.len() == 1 => Ok((guids[0], None)),
        PurchaseSelection::Items(ids) if ids.len() == 1 => {
            let id = id(ids[0])?;
            let item = snapshot
                .items
                .get(&id)
                .filter(|r| {
                    r.owner.is_none()
                        && r.state == items::PURCHASABLE
                        && r.sub_items.is_empty()
                        && r.default_items.is_empty()
                })
                .ok_or(Error::Rejected(Reason::Offer))?;
            Ok((item.definition.0, Some(id)))
        }
        _ => Err(Error::Rejected(Reason::Shape)),
    }
}

/// Debit once, allocate the complete stock car, grant missing account wrap
/// definitions and reserve a free garage slot in one durable transaction. The
/// later placement RPC may move that reservation to the requested empty slot.
pub fn buy(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    offers: &Offers,
    command: CommandId,
    request: &PurchaseRequest,
    now: Timestamp,
) -> Result<Changed, Error> {
    if command.sequence != request.sequence {
        return Err(Error::Rejected(Reason::Shape));
    }
    let request_bytes = request.encode().map_err(inventory::Error::from)?;
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        if let Some(items) = ledger::replay(&snapshot, command, &request_bytes, catalog.bindings())?
        {
            return Ok(Changed {
                snapshot,
                items,
                replayed: true,
            });
        }
        let mut garage = slots(&snapshot, catalog)?;
        let free = garage
            .0
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Rejected(Reason::Full))?;
        let (guid, promoted) = selected(request, &snapshot)?;
        let minimum = offers.0.get(&guid).ok_or(Error::Rejected(Reason::Offer))?;
        let score = snapshot
            .tables
            .get(&crate::persistent::key("RepValuesTable"))
            .and_then(|t| t.rows.get(&0))
            .and_then(|r| r.get(&crate::persistent::key("RepScore")));
        if *minimum > 0
            && !matches!(score,Some(nfs_storage::tables::Value::Int(value)) if value >= minimum)
        {
            return Err(Error::Rejected(Reason::Offer));
        }
        let definition = catalog.definition(&guid).map_err(inventory::Error::from)?;
        if definition.quantity == 1
            && snapshot
                .items
                .values()
                .any(|r| r.owner.is_none() && r.state == items::OWNED && r.definition.0 == guid)
        {
            return Err(Error::Rejected(Reason::Quantity));
        }
        let balance = Balance::from_snapshot(&snapshot)?
            .debit(definition.buy_price)
            .ok_or(Error::Rejected(Reason::Funds))?;
        let (first, _) = crate::purchase::reserve_ids(&snapshot, 0)?;
        let mut built = catalog
            .instantiate_vehicle(guid, first)
            .map_err(inventory::Error::from)?;
        let car = promoted.map_or(first, ItemId::get);
        if car != first {
            let mut root = built
                .collection
                .items
                .remove(&first)
                .ok_or(nfs_storage::Error::Config)?;
            root.id = car;
            for item in built.collection.items.values_mut() {
                if item.owner == first {
                    item.owner = car;
                }
            }
            built.collection.items.insert(car, root);
        }
        let mut grants = BTreeSet::new();
        for guid in &definition.additional_items {
            if !grants.insert(*guid)
                || snapshot.items.values().any(|r| {
                    r.definition.0 == *guid && r.owner.is_none() && r.state == items::OWNED
                })
            {
                continue;
            }
            let d = catalog.definition(guid).map_err(inventory::Error::from)?;
            built.collection.items.insert(
                built.next_id,
                Item {
                    id: built.next_id,
                    definition: *guid,
                    owner: 0,
                    defaults: vec![],
                    children: vec![],
                    state: items::OWNED,
                    buy_price: d.buy_price,
                    sell_price: d.sell_price,
                    derived: d
                        .default_derived
                        .clone()
                        .ok_or(Error::Rejected(Reason::Offer))?,
                },
            );
            built.next_id = built
                .next_id
                .checked_add(1)
                .ok_or(nfs_storage::Error::Bounds)?;
        }
        built.collection.roots = built.collection.items.keys().copied().collect();
        let mut replacement = None;
        if let Some(old) = promoted {
            let mut offer = snapshot.items[&old].clone();
            offer.id = id(built.next_id)?;
            built.next_id = built
                .next_id
                .checked_add(1)
                .ok_or(nfs_storage::Error::Bounds)?;
            replacement = Some(offer);
        }
        let (_, allocator) = crate::purchase::reserve_ids(
            &snapshot,
            usize::try_from(built.next_id - first).map_err(|_| nfs_storage::Error::Bounds)?,
        )?;
        let mut combined = inventory::collection(&snapshot, catalog.bindings())?;
        combined.items.extend(built.collection.items.clone());
        combined.roots = combined.items.keys().copied().collect();
        let mut ops = inventory::records(&combined, catalog.bindings())?
            .into_iter()
            .filter(|r| built.collection.items.contains_key(&r.id.get()))
            .map(|r| {
                if Some(r.id) == promoted {
                    Op::Update(r)
                } else {
                    Op::Insert(r)
                }
            })
            .collect::<Vec<_>>();
        if let Some(offer) = replacement {
            ops.push(Op::Insert(offer));
        }
        garage.0[free] = Some(id(car)?);
        ops.extend([allocator, balance.store(), Op::SetGarage(garage)]);
        ops.extend(ledger::record(
            &snapshot,
            command,
            &request_bytes,
            &built.collection,
            catalog.bindings(),
        )?);
        if apply(repository, account, &snapshot, ops, now)? {
            return Ok(Changed {
                snapshot: repository.snapshot(account)?,
                items: built.collection,
                replayed: false,
            });
        }
    }
    Err(nfs_storage::Error::Conflict.into())
}

/// Resale policy: credit the car's quoted resale value and remove its entire
/// ownership subtree, including installed and spare parts. Account-owned wraps
/// and shared-wrap blobs remain available. The active/last car cannot be sold;
/// select another car first. Never refund a removed item twice.
pub fn sell(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    command: CommandId,
    request: &SellRequest,
    now: Timestamp,
) -> Result<Changed, Error> {
    if command.sequence != request.sequence || request.items.len() != 1 {
        return Err(Error::Rejected(Reason::Shape));
    }
    let request_bytes = request.encode().map_err(inventory::Error::from)?;
    let car = id(request.items[0])?;
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        if let Some(items) = ledger::replay(&snapshot, command, &request_bytes, catalog.bindings())?
        {
            return Ok(Changed {
                snapshot,
                items,
                replayed: true,
            });
        }
        let mut garage = slots(&snapshot, catalog)?;
        let slot = garage
            .0
            .iter()
            .position(|v| *v == Some(car))
            .ok_or(Error::Rejected(Reason::Owner))?;
        if slot == 0 {
            return Err(Error::Rejected(Reason::Primary));
        }
        let record = snapshot
            .items
            .get(&car)
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
        let mut removed = BTreeSet::from([car]);
        loop {
            let prior = removed.len();
            removed.extend(
                snapshot
                    .items
                    .values()
                    .filter(|r| r.owner.is_some_and(|p| removed.contains(&p)))
                    .map(|r| r.id)
                    .collect::<Vec<_>>(),
            );
            if removed.len() == prior {
                break;
            }
        }
        if snapshot
            .items
            .values()
            .any(|r| !removed.contains(&r.id) && r.references().any(|p| removed.contains(&p)))
        {
            return Err(nfs_storage::Error::DanglingReference.into());
        }
        let all = inventory::collection(&snapshot, catalog.bindings())?;
        let records = all
            .items
            .into_iter()
            .filter(|(key, _)| removed.contains(&ItemId::new(*key).expect("validated ID")))
            .collect::<BTreeMap<_, _>>();
        let sold = Collection {
            roots: records.keys().copied().collect(),
            items: records,
        };
        let balance = Balance(
            Balance::from_snapshot(&snapshot)?
                .0
                .checked_add(record.sell_price)
                .ok_or(nfs_storage::Error::Bounds)?,
        );
        garage.0[slot] = None;
        let mut ops = removed.into_iter().map(Op::Remove).collect::<Vec<_>>();
        ops.extend([balance.store(), Op::SetGarage(garage)]);
        ops.extend(ledger::record(
            &snapshot,
            command,
            &request_bytes,
            &sold,
            catalog.bindings(),
        )?);
        if apply(repository, account, &snapshot, ops, now)? {
            return Ok(Changed {
                snapshot: repository.snapshot(account)?,
                items: sold,
                replayed: false,
            });
        }
    }
    Err(nfs_storage::Error::Conflict.into())
}

/// Select by resolved item identity, retaining the primary identity seen when
/// the RPC was staged. A repeated command cannot swap the two cars back.
pub fn select(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    item: u64,
    previous_primary: u64,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    let target = id(item)?;
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        let mut garage = slots(&snapshot, catalog)?;
        if garage.0[0] == Some(target) {
            return Ok(snapshot);
        }
        if garage.0[0] != Some(id(previous_primary)?) {
            return Err(nfs_storage::Error::Conflict.into());
        }
        let slot = garage
            .0
            .iter()
            .position(|v| *v == Some(target))
            .ok_or(Error::Rejected(Reason::Owner))?;
        garage.0.swap(0, slot);
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

/// Confirm or relocate a purchase reservation. It can never replace another
/// car or move the primary car into a secondary display slot.
pub fn place(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    item: u64,
    secondary: u8,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    if secondary >= 4 {
        return Err(Error::Rejected(Reason::Shape));
    }
    let target = id(item)?;
    let destination = usize::from(secondary) + 1;
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        let mut garage = slots(&snapshot, catalog)?;
        if garage.0[destination] == Some(target) {
            return Ok(snapshot);
        }
        if garage.0[destination].is_some() {
            return Err(Error::Rejected(Reason::Full));
        }
        let source = garage
            .0
            .iter()
            .position(|v| *v == Some(target))
            .ok_or(Error::Rejected(Reason::Owner))?;
        if source == 0 {
            return Err(Error::Rejected(Reason::Primary));
        }
        garage.0.swap(source, destination);
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
