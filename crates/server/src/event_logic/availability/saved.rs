// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Project every durable input from one committed account view. Scene and
//! participant membership remain with their live owners.
use super::Content;
use crate::{
    persistent::{Loaded, key},
    progression,
};
use nfs_content::events::discovery::membership::Fact;
use nfs_storage::tables::Value;
use std::collections::BTreeMap;

impl Content {
    pub fn saved_facts(
        &self,
        progression: &progression::Content,
        loaded: &Loaded,
    ) -> Result<BTreeMap<Fact, bool>, nfs_content::Error> {
        let bad =
            |message: String| nfs_content::Error::Content(format!("map saved facts: {message}"));
        let values = progression
            .condition_facts(self.facts(), self.auxiliary(), loaded)
            .map_err(|error| {
                bad(format!(
                    "progression generation {}: {error:?}",
                    loaded.generation()
                ))
            })?;
        let mut result = BTreeMap::new();
        for (id, condition) in self.conditions() {
            let value = condition
                .evaluate(|fact| values.get(&fact).copied())
                .map_err(|error| bad(format!("condition {id}: {error:?}")))?;
            result.insert(Fact::Boolean(id.clone()), value);
        }
        for program in self
            .definitions()
            .memberships
            .iter()
            .map(|b| &b.program)
            .chain(self.inputs().values())
        {
            for id in program.ranges().keys() {
                let (range, store) = self
                    .integer(id)
                    .ok_or_else(|| bad(format!("range owner {id:?}")))?;
                let table = key(&store.table);
                let column = key(&store.column);
                let value = loaded
                    .tables()
                    .get(&table)
                    .and_then(|t| t.rows.get(&0))
                    .and_then(|r| r.get(&column));
                let Some(Value::Int(value)) = value else {
                    return Err(bad(format!(
                        "range {id:?}: integer store {}.{} missing or invalid in generation {}",
                        store.table,
                        store.column,
                        loaded.generation()
                    )));
                };
                result.insert(Fact::IntegerRange(id.clone()), range.contains(*value));
            }
        }
        Ok(result)
    }
}
