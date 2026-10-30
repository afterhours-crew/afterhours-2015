// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned challenge selection. A read populates a world cache; it does
//! not grant an award, advance progress or manufacture a world acknowledgement.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    pub day_id: u32,
    pub monthly_start: i64,
    pub ids: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selected {
    pub definition: Definition,
    pub progress: Progress,
    pub obtained: BTreeSet<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Specific {
    pub generation: u64,
    pub entries: BTreeMap<u32, Selected>,
    pub monthly_rank: u32,
}

impl Catalog {
    /// Local catalog policy: exact period, known unique IDs, at most the
    /// configured 64 definitions. Do not silently substitute another period.
    pub fn validate_query(&self, query: &Query) -> Result<(), Error> {
        if query.day_id != self.day_id
            || query.monthly_start != self.monthly_start
            || query.ids.len() > self.challenges.len()
        {
            return Err(Error::Ineligible);
        }
        let mut seen = BTreeSet::new();
        for id in &query.ids {
            if !seen.insert(id) || !self.challenges.iter().any(|entry| entry.id == *id) {
                return Err(Error::Ineligible);
            }
        }
        Ok(())
    }
}

impl Current {
    pub fn specific(&self, catalog: &Catalog, query: &Query) -> Result<Specific, Error> {
        catalog.validate_query(query)?;
        self.state.validate()?;
        let mut entries = BTreeMap::new();
        for id in &query.ids {
            let definition = catalog
                .challenges
                .iter()
                .find(|entry| entry.id == *id)
                .ok_or(Error::Ineligible)?
                .clone();
            let progress = self
                .state
                .progress
                .get(&(query.day_id, *id))
                .cloned()
                .unwrap_or_default();
            let obtained = catalog
                .awards
                .iter()
                .find(|(challenge, _)| challenge == id)
                .into_iter()
                .flat_map(|(_, awards)| awards)
                .filter(|award| self.state.obtained.contains(&(query.day_id, *id, award.id)))
                .map(|award| award.id)
                .collect();
            entries.insert(
                *id,
                Selected {
                    definition,
                    progress,
                    obtained,
                },
            );
        }
        Ok(Specific {
            generation: self.generation,
            entries,
            monthly_rank: self
                .state
                .monthly_ranks
                .get(&query.monthly_start)
                .copied()
                .unwrap_or(0),
        })
    }
}
