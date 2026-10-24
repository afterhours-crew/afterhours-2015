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
