// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Plan a server-calculated REP credit and its unlocks as one storage change.
//! This is a domain boundary, not a wire request or a driving-score calculator.
//! The caller must establish the activity, calculate its scores from installed
//! rules, and retain the original batch ID and generation when retrying a credit.
use super::{Field, Scores, Thresholds};
use crate::{category_unlocks, inventory, item_unlocks, persistent};
use nfs_storage::{Batch, Op, Snapshot, tables::Value};
use nfs_world_core::items::InventoryCatalog;

pub struct Policy<'a> {
    pub tables: &'a persistent::Catalog,
    pub inventory: &'a InventoryCatalog,
    pub thresholds: &'a Thresholds,
    pub categories: Option<&'a category_unlocks::Policy>,
    pub items: Option<&'a item_unlocks::Policy>,
    pub notifications: Option<&'a crate::messages::unlocks::Policy>,
}

impl Policy<'_> {
    /// Prepare from one initialized account snapshot. Total and category credits
    /// are separate inputs: their relationship belongs to the scoring rules.
    /// Negative credits, empty awards and overflow fail without any mutation.
    /// Other rewards, when supported, must join these operations in the same
    /// generation-guarded batch. Do not rebase a successfully applied credit.
    pub fn prepare(
        &self,
        snapshot: &Snapshot,
        credit: Scores,
        batch_id: u64,
    ) -> Result<Batch, inventory::Error> {
        let loaded = self.tables.view(snapshot)?;
        let old = Scores::load(&loaded).map_err(|_| nfs_storage::Error::Config)?;
        let columns = [
            ("RepScore", old.total, credit.total),
            ("BuildScore", old.build, credit.build),
            ("CrewScore", old.crew, credit.crew),
            ("OutlawScore", old.outlaw, credit.outlaw),
            ("SpeedScore", old.speed, credit.speed),
            ("StyleScore", old.style, credit.style),
        ];
        if columns.iter().any(|(_, _, delta)| *delta < 0)
            || columns.iter().all(|(_, _, delta)| *delta == 0)
        {
            return Err(nfs_storage::Error::Invalid.into());
        }
        let table_key = persistent::key("RepValuesTable");
        let mut table = snapshot
            .tables
            .get(&table_key)
            .ok_or(nfs_storage::Error::Config)?
            .clone();
        let row = table.rows.entry(0).or_default();
        for (column, value, delta) in columns {
            row.insert(
                persistent::key(column),
                Value::Int(value.checked_add(delta).ok_or(nfs_storage::Error::Bounds)?),
            );
        }
        let mut proposed = snapshot.clone();
        proposed.tables.insert(table_key, table.clone());
        let scores =
            Scores::load(&self.tables.view(&proposed)?).map_err(|_| nfs_storage::Error::Config)?;
        let level = self.thresholds.evaluate(scores).get(Field::Level);
        table
            .rows
            .entry(0)
            .or_default()
            .insert(persistent::key("RepLevel"), Value::Int(level));
        proposed.tables.insert(table_key, table.clone());
        self.tables.view(&proposed)?;
        let mut ops = vec![Op::SetTable(table_key, table)];
        if let Some(policy) = self.categories {
            ops.extend(policy.operations(&proposed, self.inventory)?);
        }
        if let Some(policy) = self.items {
            ops.extend(policy.operations(&proposed, self.inventory)?);
        }
        let mut batch = Batch {
            id: batch_id,
            expected_generation: snapshot.generation,
            ops,
        };
        if let Some(policy) = self.notifications {
            batch
                .ops
                .extend(policy.operations(snapshot, self.inventory, &batch)?);
        }
        batch.validate()?;
        Ok(batch)
    }
}

#[cfg(test)]
mod tests;
