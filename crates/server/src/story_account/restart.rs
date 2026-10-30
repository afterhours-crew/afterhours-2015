// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An explicit fresh-career reset, sharing account-creation defaults.
use super::*;
use nfs_storage::{Batch, Op};

impl Editor<'_> {
    pub(super) fn restart_batch(
        &self,
        before: &Snapshot,
        operation: u64,
        target: Checkpoint,
    ) -> Result<Batch> {
        let policy = self.rules.policy();
        if policy
            .inspect(before)?
            .milestones
            .first()
            .map(|point| Checkpoint::Objective(point.id))
            != Some(target)
        {
            return Err("A career restart must begin at the opening.".into());
        }
        let inventory = nfs_services::fresh_account::prologue_inventory(
            self.rules
                .items
                .as_ref()
                .ok_or("Missing inventory defaults.")?,
        )?;
        let mut fresh = nfs_services::fresh_account::initial_state(
            self.account,
            &inventory,
            self.rules
                .persistent
                .as_ref()
                .ok_or("Missing persistent defaults.")?,
            Timestamp(0),
        )?;
        // Retain the server-issued session high-water mark: no old garage
        // command may become a new purchase after restarting a career.
        if let Some(table) = before.tables.get(&key("AfterhoursGarageSessions")) {
            fresh
                .tables
                .insert(key("AfterhoursGarageSessions"), table.clone());
        }
        fresh.generation = before.generation;
        let Prepared::Mutation(story) = policy.jump(&fresh, fresh.generation, operation, target)?
        else {
            return Err("Unexpected restart retry while preparing defaults.".into());
        };
        for op in story.ops {
            let Op::SetTable(id, table) = op else {
                return Err("Invalid opening story state.".into());
            };
            fresh.tables.insert(id, table);
        }
        let mut ops: Vec<_> = fresh.items.into_values().map(Op::Insert).collect();
        ops.push(Op::SetGarage(
            fresh.garage.ok_or("Missing initial garage.")?,
        ));
        ops.extend(
            fresh
                .tables
                .into_iter()
                .map(|(id, table)| Op::SetTable(id, table)),
        );
        let batch = Batch {
            id: story.id,
            expected_generation: before.generation,
            ops,
        };
        batch.validate()?;
        Ok(batch)
    }
}
