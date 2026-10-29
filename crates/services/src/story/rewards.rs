// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A story award can precede objective completion. Its item tree, allocator and
//! semantic receipt commit together; completion does not grant it a second time.
use super::{Branch, Catalog, Policy, key};
use crate::inventory;
use nfs_storage::{
    AccountId, Batch, Error, InventoryRepository, ItemId, Op, Snapshot, Timestamp,
    tables::{Row, Table, Value},
};
use nfs_world_core::items::{Guid, InventoryCatalog};
use std::collections::BTreeMap;

const VERSION: i32 = 1;
const BATCH_NAMESPACE: u64 = 1 << 54;
const FORMAT: u32 = 0;
const DEFINITION: u32 = 0;
const ITEM: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub definition: Guid,
    pub item: ItemId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    pub receipt: Receipt,
    pub batch: Option<Batch>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claimed {
    pub snapshot: Snapshot,
    pub receipt: Receipt,
    pub replayed: bool,
}

fn guid(value: &Value) -> Result<Guid, Error> {
    let Value::String(s) = value else {
        return Err(Error::Config);
    };
    if s.len() != 32
        || !s
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(Error::Config);
    }
    let mut out = [0; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| Error::Config)?;
    }
    if out == [0; 16] {
        return Err(Error::Config);
    }
    Ok(out)
}
fn text(guid: Guid) -> String {
    guid.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn read(
    snapshot: &Snapshot,
    catalog: &Catalog,
) -> Result<BTreeMap<u64, Receipt>, Error> {
    let Some(table) = snapshot.tables.get(&key("AfterhoursStoryRewards")) else {
        return Ok(BTreeMap::new());
    };
    table.validate()?;
    if table.rows.get(&0) != Some(&Row::from([(FORMAT, Value::Int(VERSION))])) {
        return Err(Error::Config);
    }
    let ids = catalog.objective_ids(Branch::Intro);
    let mut receipts = BTreeMap::new();
    for (&id, row) in table.rows.iter().filter(|(id, _)| **id != 0) {
        if !ids.contains(&id) || row.len() != 2 {
            return Err(Error::Config);
        }
        let definition = guid(row.get(&DEFINITION).ok_or(Error::Config)?)?;
        let item = ItemId::new(super::decimal(row, ITEM)?).ok_or(Error::Config)?;
        receipts.insert(id, Receipt { definition, item });
    }
    Ok(receipts)
}

impl Policy<'_> {
    /// A launcher skip can cross the first-car award. Prepare the authored tree,
    /// allocator, selection, objectives and once-only receipt in one transaction.
    /// No intermediate checkpoint or item grant is persisted.
    pub fn jump_with_vehicle(
        &self,
        snapshot: &Snapshot,
        expected: u64,
        operation: u64,
        target: super::Checkpoint,
        grant: Option<(&InventoryCatalog, u64, Guid)>,
    ) -> Result<super::Prepared, inventory::Error> {
        let prepared = self.jump(snapshot, expected, operation, target)?;
        if prepared == super::Prepared::Replayed {
            if let Some((_, objective, definition)) = grant
                && self
                    .reward(snapshot, objective)?
                    .is_none_or(|receipt| receipt.definition != definition)
            {
                return Err(Error::Conflict.into());
            }
            return Ok(prepared);
        }
        let (super::Prepared::Mutation(mut batch), Some((catalog, objective, definition))) =
            (prepared.clone(), grant)
        else {
            return Ok(prepared);
        };
        let order = self
            .catalog
            .intro_order(self.settings)
            .map_err(|_| Error::Config)?;
        let choice = order
            .iter()
            .position(|id| *id == objective)
            .ok_or(Error::Invalid)?;
        let destination = match target {
            super::Checkpoint::AfterIntro => order.len(),
            super::Checkpoint::Objective(id) => order
                .iter()
                .position(|item| *item == id)
                .ok_or(Error::Invalid)?,
        };
        if destination <= choice || self.reward(snapshot, objective)?.is_some() {
            return Err(Error::Invalid.into());
        }
        if snapshot.items.values().any(|item| {
            item.owner.is_none()
                && catalog
                    .bindings()
                    .class(&item.definition.0)
                    .is_ok_and(|class| {
                        class == nfs_world_core::items::DefinitionClass::RaceVehicleItemData
                    })
        }) {
            return Err(Error::Conflict.into());
        }
        let super::Prepared::Mutation(selection) = self.jump(
            snapshot,
            expected,
            operation,
            super::Checkpoint::Objective(objective),
        )?
        else {
            return Err(Error::Conflict.into());
        };
        let mut at_choice = snapshot.clone();
        // The shadow represents the result of that edit; its ledger records
        // the previous generation just as a committed snapshot would.
        at_choice.generation = expected.checked_add(1).ok_or(Error::Bounds)?;
        for op in selection.ops {
            let Op::SetTable(key, table) = op else {
                return Err(Error::Config.into());
            };
            at_choice.tables.insert(key, table);
        }
        let grant = self.grant_vehicle(
            &at_choice,
            at_choice.generation,
            objective,
            definition,
            catalog,
        )?;
        let grant_batch = grant.batch.ok_or(Error::Conflict)?;
        batch.ops.extend(grant_batch.ops);
        let mut slots = snapshot.garage.ok_or(Error::Config)?;
        if slots.0.iter().any(Option::is_some) {
            return Err(Error::Conflict.into());
        }
        slots.0[0] = Some(grant.receipt.item);
        batch.ops.push(Op::SetGarage(slots));
        batch.validate()?;
        Ok(super::Prepared::Mutation(batch))
    }

    /// Finish the authored selection after its grant. The selected slot and
    /// objective commit together. Other owned cars remain available, including
    /// a full garage's former primary car in the warehouse.
    pub fn select_reward(
        &self,
        snapshot: &Snapshot,
        objective: u64,
        receipt: Receipt,
    ) -> Result<super::Prepared, Error> {
        if self.reward(snapshot, objective)? != Some(receipt) {
            return Err(Error::Conflict);
        }
        let view = self.inspect(snapshot)?;
        if view
            .milestones
            .iter()
            .any(|m| m.id == objective && m.completed)
        {
            return Ok(super::Prepared::Replayed);
        }
        let item = snapshot.items.get(&receipt.item).ok_or(Error::Invalid)?;
        if item.owner.is_some()
            || item.definition.0 != receipt.definition
            || item.state != nfs_world_core::items::OWNED
        {
            return Err(Error::Invalid);
        }
        let mut slots = snapshot.garage.ok_or(Error::Config)?;
        if let Some(index) = slots.0.iter().position(|id| *id == Some(receipt.item)) {
            slots.0.swap(0, index);
        } else {
            let previous = slots.0[0].replace(receipt.item);
            if let Some(empty) = slots.0[1..].iter_mut().find(|id| id.is_none()) {
                *empty = previous;
            }
        }
        let super::Prepared::Mutation(mut batch) = self.complete(
            snapshot,
            snapshot.generation,
            self.next_operation(snapshot)?,
            objective,
            vec![],
        )?
        else {
            return Err(Error::Conflict);
        };
        batch.ops.push(Op::SetGarage(slots));
        batch.validate()?;
        Ok(super::Prepared::Mutation(batch))
    }
    /// Synchronous repository port: execute on a blocking worker. A competing
    /// transaction can share a generation retry key, so reload and verify the
    /// semantic receipt before acknowledging success to the sequence.
    pub fn claim_vehicle(
        &self,
        repository: &dyn InventoryRepository,
        account: AccountId,
        objective: u64,
        definition: Guid,
        catalog: &InventoryCatalog,
        now: Timestamp,
    ) -> Result<Claimed, inventory::Error> {
        for _ in 0..8 {
            let snapshot = repository.snapshot(account)?;
            let grant = self.grant_vehicle(
                &snapshot,
                snapshot.generation,
                objective,
                definition,
                catalog,
            )?;
            let Some(batch) = grant.batch else {
                return Ok(Claimed {
                    snapshot,
                    receipt: grant.receipt,
                    replayed: true,
                });
            };
            match repository.apply(account, &batch, now) {
                Ok(applied) => {
                    let snapshot = repository.snapshot(account)?;
                    let verified = self.grant_vehicle(
                        &snapshot,
                        snapshot.generation,
                        objective,
                        definition,
                        catalog,
                    )?;
                    if verified.batch.is_some() {
                        return Err(Error::Conflict.into());
                    }
                    return Ok(Claimed {
                        snapshot,
                        receipt: verified.receipt,
                        replayed: applied.replayed,
                    });
                }
                Err(Error::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::Conflict.into())
    }
    pub fn reward(&self, snapshot: &Snapshot, objective: u64) -> Result<Option<Receipt>, Error> {
        Ok(read(snapshot, self.catalog)?.get(&objective).copied())
    }

    /// A replay acknowledges the previously earned car, regardless of which
    /// preview was selected. It never changes its tuning or grants another tree.
    /// Legacy completed saves adopt their existing primary car as the receipt.
    /// Normal first-time awards still use `claim_vehicle` and its conflict checks.
    pub fn claim_selection(
        &self,
        repository: &dyn InventoryRepository,
        account: AccountId,
        objective: u64,
        definition: Guid,
        catalog: &InventoryCatalog,
        now: Timestamp,
    ) -> Result<Claimed, inventory::Error> {
        for _ in 0..8 {
            let snapshot = repository.snapshot(account)?;
            let view = self.inspect(&snapshot)?;
            let milestone = view
                .milestones
                .iter()
                .find(|m| m.id == objective)
                .ok_or(Error::Invalid)?;
            let receipt = self.reward(&snapshot, objective)?;
            if receipt.is_none() && !milestone.existing && !milestone.reward_claimed {
                return self
                    .claim_vehicle(repository, account, objective, definition, catalog, now);
            }
            if !milestone.completed
                && (!milestone.active
                    || view.milestones.get(view.position).map(|m| m.id) != Some(objective))
            {
                return Err(Error::Invalid.into());
            }
            let item = match receipt {
                Some(receipt) => snapshot.items.get(&receipt.item),
                None => snapshot
                    .garage
                    .and_then(|s| s.0[0])
                    .and_then(|id| snapshot.items.get(&id)),
            }
            .ok_or(Error::Conflict)?;
            if item.owner.is_some()
                || item.state != nfs_world_core::items::OWNED
                || !catalog
                    .bindings()
                    .class(&item.definition.0)
                    .is_ok_and(|class| {
                        class == nfs_world_core::items::DefinitionClass::RaceVehicleItemData
                    })
                || receipt.is_some_and(|r| r.definition != item.definition.0)
            {
                return Err(Error::Conflict.into());
            }
            inventory::collection(&snapshot, catalog.bindings())?;
            let saved = Receipt {
                definition: item.definition.0,
                item: item.id,
            };
            if receipt.is_some() {
                return Ok(Claimed {
                    snapshot,
                    receipt: saved,
                    replayed: true,
                });
            }
            let mut table = snapshot
                .tables
                .get(&key("AfterhoursStoryRewards"))
                .cloned()
                .unwrap_or_else(|| Table {
                    rows: BTreeMap::from([(0, Row::from([(FORMAT, Value::Int(VERSION))]))]),
                });
            table.rows.insert(
                objective,
                Row::from([
                    (DEFINITION, Value::String(text(saved.definition))),
                    (ITEM, Value::String(saved.item.get().to_string())),
                ]),
            );
            if snapshot.generation >= BATCH_NAMESPACE {
                return Err(Error::Bounds.into());
            }
            let batch = Batch {
                id: BATCH_NAMESPACE | snapshot.generation,
                expected_generation: snapshot.generation,
                ops: vec![Op::SetTable(key("AfterhoursStoryRewards"), table)],
            };
            match repository.apply(account, &batch, now) {
                Ok(_) | Err(Error::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::Conflict.into())
    }

    /// The sequence owner supplies the authored award definition, never a raw
    /// client definition. This only grants a vehicle for the active objective.
    /// Selecting a garage slot and completing the objective are later operations.
    pub fn grant_vehicle(
        &self,
        snapshot: &Snapshot,
        expected: u64,
        objective: u64,
        definition: Guid,
        catalog: &InventoryCatalog,
    ) -> Result<Grant, inventory::Error> {
        if let Some(receipt) = self.reward(snapshot, objective)? {
            if receipt.definition != definition {
                return Err(Error::Conflict.into());
            }
            let item = snapshot.items.get(&receipt.item).ok_or(Error::Conflict)?;
            if item.owner.is_some()
                || item.definition.0 != definition
                || item.state != nfs_world_core::items::OWNED
            {
                return Err(Error::Conflict.into());
            }
            inventory::collection(snapshot, catalog.bindings())?;
            return Ok(Grant {
                receipt,
                batch: None,
            });
        }
        if snapshot.generation != expected {
            return Err(Error::Conflict.into());
        }
        let view = self.inspect(snapshot)?;
        let milestone = view
            .milestones
            .get(view.position)
            .filter(|m| m.id == objective && m.active && !m.completed)
            .ok_or(Error::Invalid)?;
        if milestone.reward_claimed || milestone.existing {
            return Err(Error::Conflict.into());
        }
        if snapshot.generation >= BATCH_NAMESPACE {
            return Err(Error::Bounds.into());
        }
        let (first, _) = crate::purchase::reserve_ids(snapshot, 0)?;
        let built = inventory::vehicle_grant(snapshot, catalog, definition, first)?;
        let receipt = Receipt {
            definition,
            item: ItemId::new(first).ok_or(Error::Bounds)?,
        };
        let (_, allocator) = crate::purchase::reserve_ids(
            snapshot,
            usize::try_from(built.next_id - first).map_err(|_| Error::Bounds)?,
        )?;
        let mut table = snapshot
            .tables
            .get(&key("AfterhoursStoryRewards"))
            .cloned()
            .unwrap_or_else(|| Table {
                rows: BTreeMap::from([(0, Row::from([(FORMAT, Value::Int(VERSION))]))]),
            });
        table.rows.insert(
            objective,
            Row::from([
                (DEFINITION, Value::String(text(definition))),
                (ITEM, Value::String(first.to_string())),
            ]),
        );
        let mut ops = inventory::records(&built.collection, catalog.bindings())?
            .into_iter()
            .map(Op::Insert)
            .collect::<Vec<_>>();
        ops.extend([
            allocator,
            Op::SetTable(key("AfterhoursStoryRewards"), table),
        ]);
        let batch = Batch {
            id: BATCH_NAMESPACE | snapshot.generation,
            expected_generation: snapshot.generation,
            ops,
        };
        batch.validate()?;
        Ok(Grant {
            receipt,
            batch: Some(batch),
        })
    }
}
