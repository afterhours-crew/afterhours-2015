// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Durable REP unlocks create purchasable root records, not free equipped parts.
use crate::{inventory, persistent::key, purchase};
use nfs_storage::{
    AccountId, Batch, DefinitionGuid, InventoryRepository, ItemId, ItemRecord, Op, Snapshot,
    Timestamp, tables::Value,
};
use nfs_world_core::items::{self, Guid, InventoryCatalog};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub(crate) requirements: BTreeMap<Guid, i32>,
}
impl Policy {
    /// Only explicit, supported leaf unlocks from the installed REP graph.
    pub fn new(
        catalog: &InventoryCatalog,
        requirements: impl IntoIterator<Item = (Guid, i32)>,
    ) -> Result<Self, nfs_storage::Error> {
        let mut rows = BTreeMap::new();
        for (guid, score) in requirements {
            let d = catalog
                .definition(&guid)
                .map_err(|_| nfs_storage::Error::Config)?;
            if rows.len() >= items::MAX_COLLECTION
                || score <= 0
                || !purchase::part_class(d.class)
                || d.quantity != 1
                || d.flags.purchasable
                || d.flags.deprecated
                || !d.flags.requires_owner
                || d.flags.uses_scope
                || !d.sub_items.is_empty()
                || !d.additional_items.is_empty()
                || d.default_derived.is_none()
                || catalog.initial_definitions().contains(&guid)
                || rows.insert(guid, score).is_some()
            {
                return Err(nfs_storage::Error::Config);
            }
        }
        Ok(Self { requirements: rows })
    }
    pub fn requirements(&self) -> &BTreeMap<Guid, i32> {
        &self.requirements
    }

    /// Unconsumed grants survive later REP resets. Selecting an instance for
    /// purchase promotes it to owned; a later eligible load can grant it again.
    pub fn granted(snapshot: &Snapshot, guid: &Guid) -> bool {
        snapshot
            .items
            .values()
            .any(|i| i.definition.0 == *guid && i.state == items::PURCHASABLE && i.owner.is_none())
    }
    pub fn ensure(
        &self,
        repository: &dyn InventoryRepository,
        account: AccountId,
        catalog: &InventoryCatalog,
        mut snapshot: Snapshot,
        now: Timestamp,
    ) -> Result<Snapshot, inventory::Error> {
        const NAMESPACE: u64 = 1 << 57;
        if self.requirements.is_empty() {
            return Ok(snapshot);
        }
        for _ in 0..4 {
            let score = match snapshot
                .tables
                .get(&key("RepValuesTable"))
                .and_then(|t| t.rows.get(&0))
                .and_then(|r| r.get(&key("RepScore")))
            {
                Some(Value::Int(score)) => *score,
                _ => return Err(nfs_storage::Error::Config.into()),
            };
            inventory::collection(&snapshot, catalog.bindings())?;
            let missing: Vec<_> = self
                .requirements
                .iter()
                .filter(|(guid, minimum)| score >= **minimum && !Self::granted(&snapshot, guid))
                .map(|(guid, _)| *guid)
                .collect();
            if missing.is_empty() {
                return Ok(snapshot);
            }
            let (first, allocation) = purchase::reserve_ids(&snapshot, missing.len())?;
            let mut ops = vec![allocation];
            for (offset, guid) in missing.into_iter().enumerate() {
                let d = catalog.definition(&guid)?;
                ops.push(Op::Insert(ItemRecord {
                    id: ItemId::new(first + offset as u64).ok_or(nfs_storage::Error::Bounds)?,
                    definition: DefinitionGuid(guid),
                    owner: None,
                    state: items::PURCHASABLE,
                    buy_price: d.buy_price,
                    sell_price: d.sell_price,
                    default_items: vec![],
                    sub_items: vec![],
                    derived: d
                        .default_derived
                        .as_ref()
                        .ok_or(nfs_storage::Error::Config)?
                        .encode()?,
                }));
            }
            if snapshot.generation >= NAMESPACE {
                return Err(nfs_storage::Error::Bounds.into());
            }
            match repository.apply(
                account,
                &Batch {
                    id: NAMESPACE | snapshot.generation,
                    expected_generation: snapshot.generation,
                    ops,
                },
                now,
            ) {
                Ok(_) | Err(nfs_storage::Error::Conflict) => {
                    snapshot = repository.snapshot(account)?
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(nfs_storage::Error::Conflict.into())
    }
}

#[cfg(test)]
mod tests;
