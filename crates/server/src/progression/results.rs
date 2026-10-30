// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_content::progression::{conditions::Fact, results::Completion};

impl Content {
    pub fn with_objective_results(mut self, rules: Vec<Completion>) -> Result<Self, Failure> {
        let main = self.catalog.objective_ids(Branch::Main);
        let all: BTreeSet<_> = main
            .iter()
            .copied()
            .chain(self.catalog.objective_ids(Branch::Intro))
            .collect();
        let mut ids = BTreeSet::new();
        if rules.len() > 64
            || rules.iter().any(|r| {
                !main.contains(&r.objective)
                    || !ids.insert(r.objective)
                    || r.categories.is_empty()
                    || r.categories.len() > 128
                    || !r.enabled.facts().any(|f| f == Fact::Active(r.objective))
                    || r.enabled.facts().any(|f| match f {
                        Fact::Active(id) | Fact::Completed(id) => !all.contains(&id),
                        Fact::MessageRead { .. } => false,
                    })
            })
        {
            return Err(Failure::ProfileConfig);
        }
        self.objective_results = rules;
        Ok(self)
    }

    /// Attribute an owned completed event using its exact authored categories
    /// and the account's committed conditions. Unknown condition inputs fail;
    /// an inactive story variant is not silently paid as an ordinary attempt.
    pub fn result_objective(
        &self,
        categories: &BTreeSet<u32>,
        loaded: &Loaded,
    ) -> Result<Option<u64>, Failure> {
        let selected: Vec<_> = self
            .objective_results
            .iter()
            .filter(|r| r.categories.is_subset(categories))
            .collect();
        let [rule] = selected.as_slice() else {
            return if selected.is_empty() {
                Ok(None)
            } else {
                Err(Failure::ProfileConfig)
            };
        };
        let restored = self.catalog.restore(loaded, self.settings)?;
        let mut facts = BTreeMap::new();
        for fact in rule.enabled.facts() {
            let value = match fact {
                Fact::Active(id) | Fact::Completed(id) => {
                    let state = [Branch::Intro, Branch::Main]
                        .into_iter()
                        .find_map(|b| restored.branch(b).objectives().get(&id))
                        .ok_or(Failure::ProfileConfig)?;
                    match fact {
                        Fact::Active(_) => state.active && !state.completed,
                        _ => state.completed,
                    }
                }
                Fact::MessageRead { hash, manager } => {
                    let content = self.messages.as_ref().ok_or(Failure::ProfileConfig)?;
                    if !content.catalog.by_hash(hash).is_some_and(|m| m.persisted) {
                        return Err(Failure::ProfileConfig);
                    }
                    let manager = content.manager(manager).ok_or(Failure::ProfileConfig)?;
                    if !loaded
                        .tables()
                        .contains_key(&crate::persistent::key(manager.table()))
                    {
                        return Err(Failure::ProfileConfig);
                    }
                    let rows = nfs_services::messages::rows(loaded.tables(), manager.table())
                        .map_err(|_| Failure::ProfileConfig)?;
                    rows.iter()
                        .rev()
                        .find(|row| row.hash == hash)
                        .is_some_and(|row| row.read)
                }
            };
            facts.insert(fact, value);
        }
        if !rule
            .enabled
            .evaluate(|fact| facts.get(&fact).copied())
            .map_err(|_| Failure::ProfileConfig)?
        {
            return Err(Failure::ProfileConfig);
        }
        Ok(Some(rule.objective))
    }
}
