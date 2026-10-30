// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only condition projection from one committed account generation.
//! Auxiliary definitions describe persistence ownership, not gameplay support.
use super::*;
use nfs_content::progression::conditions::{Fact, owners::StateOwners};

impl Content {
    pub fn condition_facts(
        &self,
        requested: &BTreeSet<Fact>,
        auxiliary: &StateOwners,
        loaded: &Loaded,
    ) -> Result<BTreeMap<Fact, bool>, Failure> {
        if requested.len() > 4096
            || auxiliary.objectives.len() + auxiliary.integers.len() > 2048
            || auxiliary.objectives.contains_key(&0)
            || auxiliary.integers.contains(&0)
            || self.catalog.definition.branches().values().any(|b| {
                b.objectives()
                    .iter()
                    .any(|(id, _)| auxiliary.objectives.contains_key(id))
                    || b.ints().iter().any(|id| auxiliary.integers.contains(id))
            })
        {
            return Err(Failure::ProfileConfig);
        }
        let restored = self.catalog.restore(loaded, self.settings)?;
        let mut messages = BTreeMap::new();
        let mut objectives = BTreeMap::new();
        let mut result = BTreeMap::new();
        for &fact in requested {
            let value = match fact {
                Fact::Active(id) | Fact::Completed(id) => {
                    let state = if let Some(state) = [Branch::Intro, Branch::Main]
                        .into_iter()
                        .find_map(|b| restored.branch(b).objectives().get(&id))
                    {
                        *state
                    } else {
                        if let std::collections::btree_map::Entry::Vacant(entry) =
                            objectives.entry(id)
                        {
                            let initial = auxiliary
                                .objectives
                                .get(&id)
                                .ok_or(Failure::ProfileConfig)?;
                            entry.insert(restore_objective(loaded, id, initial, self.settings)?);
                        }
                        objectives[&id]
                    };
                    match fact {
                        Fact::Active(_) => state.active && !state.completed,
                        _ => state.completed,
                    }
                }
                Fact::PositiveInt(id) => {
                    let value = [Branch::Intro, Branch::Main]
                        .into_iter()
                        .find_map(|b| restored.branch(b).ints().get(&id).copied());
                    match value {
                        Some(value) => value > 0,
                        None if auxiliary.integers.contains(&id) => {
                            int_cell(loaded, VALUES, id, INTS)?.unwrap_or(0) > 0
                        }
                        None => return Err(Failure::ProfileConfig),
                    }
                }
                Fact::MessageRead { hash, manager } => {
                    let content = self.messages.as_ref().ok_or(Failure::ProfileConfig)?;
                    if !content.catalog.by_hash(hash).is_some_and(|m| m.persisted) {
                        return Err(Failure::ProfileConfig);
                    }
                    let owner = content.manager(manager).ok_or(Failure::ProfileConfig)?;
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        messages.entry(manager)
                    {
                        if !loaded
                            .tables()
                            .contains_key(&crate::persistent::key(owner.table()))
                        {
                            return Err(Failure::ProfileConfig);
                        }
                        entry.insert(
                            nfs_services::messages::rows(loaded.tables(), owner.table())
                                .map_err(|_| Failure::ProfileConfig)?,
                        );
                    }
                    messages[&manager]
                        .iter()
                        .rev()
                        .find(|row| row.hash == hash)
                        .is_some_and(|row| row.read)
                }
            };
            result.insert(fact, value);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
