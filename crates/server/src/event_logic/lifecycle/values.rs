// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

/// Borrowed observer stores from either an idle host or an active candidate.
pub struct Values<'a> {
    pub(super) plan: &'a Plan,
    pub(super) observers: &'a Observers,
}
impl Host {
    pub fn values(&self) -> Values<'_> {
        Values {
            plan: &self.plan,
            observers: &self.observers,
        }
    }
}
impl Values<'_> {
    pub fn members(&self, source: &SetSource) -> Result<BTreeSet<Participant>, Error> {
        self.plan.validate_set(source)?;
        Ok(self
            .observers
            .source_members(self.plan, source)
            .cloned()
            .unwrap_or_default())
    }
    pub fn boolean(
        &self,
        source: &BooleanSource,
        participant: Participant,
    ) -> Result<Option<bool>, Error> {
        self.plan.validate_boolean(source)?;
        Ok(self.observers.stored_boolean(source, participant))
    }
    pub fn integer(
        &self,
        source: &IntegerSource,
        participant: Participant,
    ) -> Result<Option<i32>, Error> {
        self.plan.validate_integer(source)?;
        Ok(self
            .observers
            .rankings
            .values
            .get(&(source.clone(), participant))
            .copied())
    }
    pub fn float(&self, source: &Property, participant: Participant) -> Result<Option<f32>, Error> {
        source.validate().map_err(|_| Error::Invalid)?;
        Ok(self
            .observers
            .floats
            .values
            .get(&(source.clone(), participant))
            .copied())
    }
    pub fn ranking(&self, sorter: usize) -> Result<&[nfs_world::scoring::rank_query::Row], Error> {
        self.observers.rankings.rows(sorter)
    }
}
