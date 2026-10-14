// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Atomic ownership of one unique, car-owned performance part. Equipping is a
//! separate ItemBuilder transaction. The caller supplies an explicit offer set;
//! merely appearing in the item catalog does not authorize an offer or unlock.
use crate::{bank::Balance, inventory};
use nfs_storage::{
    AccountId, Batch, DefinitionGuid, InventoryRepository, ItemId, ItemRecord, Op, Snapshot,
    Timestamp,
    tables::{Table, Value},
};
use nfs_world_core::items::{
    self, Collection, DefinitionClass, Guid, InventoryCatalog, PurchaseRequest, PurchaseSelection,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_OFFERS: usize = 256;
const BATCH_NAMESPACE: u64 = 1 << 62;

/// Performance classes whose leaf replacement is keyed by exact class. Visual
/// parts may additionally use location and compatibility rules and are separate.
pub fn performance_class(class: DefinitionClass) -> bool {
    use DefinitionClass::*;
    matches!(
        class,
        AirFilterItemData
            | BrakesItemData
            | CamShaftItemData
            | ClutchItemData
            | ControlArmTuningItemData
            | CylinderHeadsItemData
            | DifferentialTuningItemData
            | EcuItemData
            | ElectricSystemItemData
            | EngineBlockItemData
            | EngineDisplacementItemData
            | ExhaustManifoldItemData
            | ExhaustSystemItemData
            | ForcedInductionItemData
            | FuelSystemItemData
            | GearboxTuningItemData
            | GearsItemData
            | HandbrakeTuningItemData
            | IgnitionItemData
            | IntakeManifoldItemData
            | NosTuningItemData
            | PersistantTuningItemData
            | RadiatorItemData
            | SteeringTuningItemData
            | SuspensionTuningItemData
            | SwaybarTuningItemData
            | TireCompositionItemData
    )
}

/// Offers must already be resolved from installed compatibility/unlock policy.
/// No client-supplied prices or definitions are added to this set on demand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Offers {
    definitions: BTreeSet<Guid>,
    vehicle_scopes: Option<BTreeMap<Guid, BTreeSet<Guid>>>,
}
impl Offers {
    pub fn definitions(&self) -> impl Iterator<Item = &Guid> {
        self.definitions.iter()
    }
    pub fn vehicle_scopes(&self) -> Option<&BTreeMap<Guid, BTreeSet<Guid>>> {
        self.vehicle_scopes.as_ref()
    }
    /// Resolve the installed vehicle's explicit part list before a purchase or
    /// equip. Legacy explicit offer sets have no additional vehicle restriction.
    pub fn allows(&self, vehicle: &Guid, part: &Guid) -> bool {
        self.definitions.contains(part)
            && self.vehicle_scopes.as_ref().is_none_or(|scopes| {
                scopes
                    .get(vehicle)
                    .is_some_and(|scope| scope.contains(part))
            })
    }
    pub fn with_vehicle_scopes(
        mut self,
        catalog: &InventoryCatalog,
        entries: impl IntoIterator<Item = (Guid, Vec<Guid>)>,
    ) -> Result<Self, Error> {
        let invalid = || Error::Rejected(Reason::Offer);
        let mut scopes = BTreeMap::new();
        let mut total = 0usize;
        for (vehicle, parts) in entries {
            total = total.checked_add(parts.len()).ok_or_else(invalid)?;
            if scopes.len() >= 512
                || total > 262_144
                || parts.len() > items::MAX_COLLECTION
                || catalog.bindings().class(&vehicle).ok()
                    != Some(DefinitionClass::RaceVehicleItemData)
            {
                return Err(invalid());
            }
            let mut scope = BTreeSet::new();
            for part in parts {
                if catalog.definition(&part).is_err() || !scope.insert(part) {
                    return Err(invalid());
                }
            }
            if scopes.insert(vehicle, scope).is_some() {
                return Err(invalid());
            }
        }
        self.vehicle_scopes = Some(scopes);
        Ok(self)
    }
    pub fn new(guids: impl IntoIterator<Item = Guid>) -> Result<Self, Error> {
        let mut set = BTreeSet::new();
        for guid in guids {
            if set.len() == MAX_OFFERS || !set.insert(guid) {
                return Err(Error::Rejected(Reason::Offer));
            }
        }
        Ok(Self {
            definitions: set,
            vehicle_scopes: None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    Shape,
    Offer,
    Owner,
    Funds,
    AlreadyOwned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Rejected(Reason),
    Storage(nfs_storage::Error),
    Inventory(inventory::Error),
}
impl From<nfs_storage::Error> for Error {
    fn from(value: nfs_storage::Error) -> Self {
        Self::Storage(value)
    }
}
impl From<inventory::Error> for Error {
    fn from(value: inventory::Error) -> Self {
        Self::Inventory(value)
    }
}

#[derive(Clone, Debug)]
pub struct Purchased {
    pub snapshot: Snapshot,
    pub items: Collection,
    pub balance: Balance,
    pub replayed: bool,
}

fn table_key() -> u32 {
    crate::persistent::key("AfterhoursPurchases")
}
fn text(value: u64) -> Value {
    Value::String(value.to_string())
}
fn number(row: &BTreeMap<u32, Value>, column: u32) -> Result<u64, Error> {
    let Some(Value::String(value)) = row.get(&column) else {
        return Err(nfs_storage::Error::Config.into());
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|_| nfs_storage::Error::Config)?;
    if parsed.to_string() != *value {
        return Err(nfs_storage::Error::Config.into());
    }
    Ok(parsed)
}
fn guid_text(guid: Guid) -> String {
    guid.iter().map(|b| format!("{b:02x}")).collect()
}

fn ledger(snapshot: &Snapshot) -> Result<(Table, u64), Error> {
    let minimum = snapshot
        .items
        .keys()
        .next_back()
        .map_or(Some(1), |id| id.get().checked_add(1))
        .ok_or(nfs_storage::Error::Bounds)?;
    let table = snapshot
        .tables
        .get(&table_key())
        .cloned()
        .unwrap_or_else(|| Table {
            rows: BTreeMap::from([(0, BTreeMap::from([(0, Value::Int(1)), (1, text(minimum))]))]),
        });
    let row = table.rows.get(&0).ok_or(nfs_storage::Error::Config)?;
    if row.len() != 2 || row.get(&0) != Some(&Value::Int(1)) {
        return Err(nfs_storage::Error::Config.into());
    }
    let next = number(row, 1)?;
    if next < minimum {
        return Err(nfs_storage::Error::Config.into());
    }
    for (&id, row) in table.rows.iter().filter(|(id, _)| **id != 0) {
        if id >= next || row.len() != 4 || row.get(&0) != Some(&Value::Int(1)) {
            return Err(nfs_storage::Error::Config.into());
        }
        number(row, 1)?;
        number(row, 3)?;
        let Some(Value::String(guid)) = row.get(&2) else {
            return Err(nfs_storage::Error::Config.into());
        };
        if guid.len() != 32
            || !guid
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(nfs_storage::Error::Config.into());
        }
    }
    Ok((table, next))
}

fn selected(request: &PurchaseRequest) -> Result<Guid, Error> {
    if request.sequence < 2 {
        return Err(Error::Rejected(Reason::Shape));
    }
    match &request.selection {
        PurchaseSelection::Definitions(guids) if guids.len() == 1 => Ok(guids[0]),
        _ => Err(Error::Rejected(Reason::Shape)),
    }
}

fn reply(
    snapshot: Snapshot,
    catalog: &InventoryCatalog,
    id: ItemId,
    replayed: bool,
) -> Result<Purchased, Error> {
    let mut all = inventory::collection(&snapshot, catalog.bindings())?;
    let item = all
        .items
        .remove(&id.get())
        .ok_or(nfs_storage::Error::Config)?;
    if !item.children.is_empty() || !item.defaults.is_empty() {
        return Err(nfs_storage::Error::Config.into());
    }
    Ok(Purchased {
        balance: Balance::from_snapshot(&snapshot)?,
        snapshot,
        items: Collection {
            roots: vec![id.get()],
            items: BTreeMap::from([(id.get(), item)]),
        },
        replayed,
    })
}

/// Commit inventory, cash, monotonically allocated ID and durable purchase
/// receipt in one account transaction. Retries use the owner/definition receipt,
/// never the client's sequence as a global repository batch ID. A receipt whose
/// item disappeared fails closed rather than granting or charging again.
pub fn buy(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &InventoryCatalog,
    offers: &Offers,
    request: &PurchaseRequest,
    now: Timestamp,
) -> Result<Purchased, Error> {
    let guid = selected(request)?;
    if !offers.definitions.contains(&guid) {
        return Err(Error::Rejected(Reason::Offer));
    }
    let definition = catalog.definition(&guid).map_err(inventory::Error::from)?;
    // The offer set has already resolved compatibility and unlocks. Purchase
    // still enforces the supported leaf ownership/default construction contract.
    if !performance_class(definition.class)
        || definition.quantity != 1
        || !definition.flags.requires_owner
        || !definition.flags.purchasable
        || definition.flags.deprecated
        || definition.flags.uses_scope
        || !definition.sub_items.is_empty()
        || !definition.additional_items.is_empty()
        || definition.default_derived.is_none()
    {
        return Err(Error::Rejected(Reason::Offer));
    }
    let owner = ItemId::new(request.owner).ok_or(Error::Rejected(Reason::Owner))?;
    for _ in 0..4 {
        let snapshot = repository.snapshot(account)?;
        inventory::collection(&snapshot, catalog.bindings())?;
        let vehicle = snapshot
            .items
            .get(&owner)
            .ok_or(Error::Rejected(Reason::Owner))?;
        if vehicle.owner.is_some()
            || vehicle.state != items::OWNED
            || catalog
                .bindings()
                .class(&vehicle.definition.0)
                .map_err(inventory::Error::from)?
                != DefinitionClass::RaceVehicleItemData
            || snapshot
                .garage
                .is_none_or(|slots| !slots.0.contains(&Some(owner)))
        {
            return Err(Error::Rejected(Reason::Owner));
        }
        if !offers.allows(&vehicle.definition.0, &guid) {
            return Err(Error::Rejected(Reason::Offer));
        }
        let (mut receipts, next) = ledger(&snapshot)?;
        let definition_text = Value::String(guid_text(guid));
        let matches: Vec<_> = receipts
            .rows
            .iter()
            .filter(|(id, row)| {
                **id != 0
                    && row.get(&1) == Some(&text(owner.get()))
                    && row.get(&2) == Some(&definition_text)
            })
            .collect();
        if matches.len() > 1 {
            return Err(nfs_storage::Error::Config.into());
        }
        if let Some(id) = matches.first().map(|(id, _)| **id) {
            let id = ItemId::new(id).ok_or(nfs_storage::Error::Config)?;
            let record = snapshot.items.get(&id).ok_or(nfs_storage::Error::Config)?;
            if record.owner != Some(owner)
                || record.definition.0 != guid
                || record.state != items::OWNED
            {
                return Err(nfs_storage::Error::Config.into());
            }
            return reply(snapshot, catalog, id, true);
        }
        if snapshot.items.values().any(|item| {
            item.owner == Some(owner) && item.definition.0 == guid && item.state == items::OWNED
        }) {
            return Err(Error::Rejected(Reason::AlreadyOwned));
        }
        let balance = Balance::from_snapshot(&snapshot)?
            .debit(definition.buy_price)
            .ok_or(Error::Rejected(Reason::Funds))?;
        let id = ItemId::new(next).ok_or(nfs_storage::Error::Config)?;
        let following = next.checked_add(1).ok_or(nfs_storage::Error::Bounds)?;
        receipts
            .rows
            .get_mut(&0)
            .ok_or(nfs_storage::Error::Config)?
            .insert(1, text(following));
        receipts.rows.insert(
            next,
            BTreeMap::from([
                (0, Value::Int(1)),
                (1, text(owner.get())),
                (2, definition_text),
                (3, text(u64::from(definition.buy_price))),
            ]),
        );
        receipts.validate()?;
        if snapshot.generation >= BATCH_NAMESPACE {
            return Err(nfs_storage::Error::Bounds.into());
        }
        let record = ItemRecord {
            id,
            definition: DefinitionGuid(guid),
            owner: Some(owner),
            state: items::OWNED,
            buy_price: definition.buy_price,
            sell_price: definition.sell_price,
            default_items: vec![],
            sub_items: vec![],
            derived: definition
                .default_derived
                .as_ref()
                .ok_or(Error::Rejected(Reason::Offer))?
                .encode()
                .map_err(inventory::Error::from)?,
        };
        // The namespace is reserved for purchase attempts. Competing purchases
        // on this generation may replay a winner: re-read the receipt before
        // acknowledging, rather than mistaking a different purchase for ours.
        let batch = Batch {
            id: BATCH_NAMESPACE | snapshot.generation,
            expected_generation: snapshot.generation,
            ops: vec![
                Op::Insert(record),
                balance.store(),
                Op::SetTable(table_key(), receipts),
            ],
        };
        match repository.apply(account, &batch, now) {
            Ok(applied) if !applied.replayed => {
                // Construct the exact committed view without racing a later
                // writer's snapshot. All adapters share these pure transitions.
                let mut committed = snapshot.clone();
                committed.items = nfs_storage::port::transition(&snapshot.items, &batch)?;
                for op in batch.ops {
                    if let Op::SetTable(key, table) = op {
                        committed.tables.insert(key, table);
                    }
                }
                committed.generation = applied.generation;
                committed.updated_at = now;
                return reply(committed, catalog, id, false);
            }
            Ok(_) | Err(nfs_storage::Error::Conflict) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(nfs_storage::Error::Conflict.into())
}

#[cfg(test)]
mod tests;
