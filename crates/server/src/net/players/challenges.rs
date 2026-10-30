// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_services::challenges::{Catalog, Current, Query, Selected};
use std::sync::Arc;

const MAX_READS: usize = 8;

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    pub(super) catalog: Option<Arc<Catalog>>,
    pending: VecDeque<Query>,
    entries: BTreeMap<u32, Selected>,
    generation: Option<u64>,
    monthly_rank: u32,
}

#[derive(Clone, Copy, Debug)]
enum Rejected {
    Unavailable,
    Identity,
    Query,
    Queue,
}

impl State {
    fn prepare(
        &self,
        persona: u64,
        messages: &[frame::Message<'_>],
    ) -> Result<Option<Self>, Rejected> {
        let mut next = None;
        for message in messages {
            let frame::Message::ChallengePlayerSync(request) = message else {
                continue;
            };
            let catalog = self.catalog.as_ref().ok_or(Rejected::Unavailable)?;
            if persona == 0 || request.persona != persona {
                return Err(Rejected::Identity);
            }
            let query = Query {
                day_id: request.day_id,
                monthly_start: request.monthly_start as i64,
                ids: request.challenge_ids.clone(),
            };
            catalog
                .validate_query(&query)
                .map_err(|_| Rejected::Query)?;
            let state = next.get_or_insert_with(|| self.clone());
            if state.pending.contains(&query) {
                continue;
            }
            if state.pending.len() >= MAX_READS {
                return Err(Rejected::Queue);
            }
            state.pending.push_back(query);
        }
        Ok(next)
    }

    fn loaded(&mut self, current: &Current) -> Result<(), nfs_services::challenges::Error> {
        use nfs_services::challenges::Error;
        let catalog = self.catalog.as_ref().ok_or(Error::State)?;
        let query = self.pending.front().ok_or(Error::State)?;
        if self
            .generation
            .is_some_and(|generation| current.generation() < generation)
        {
            return Err(Error::State);
        }
        let selected = current.specific(catalog, query)?;
        // A fixed catalog bounds the union. Repeated reads replace the same IDs;
        // a new subset cannot discard previously cached challenge definitions.
        self.entries.extend(selected.entries);
        self.monthly_rank = selected.monthly_rank;
        self.generation = Some(selected.generation);
        self.pending.pop_front();
        Ok(())
    }
}

impl PlayerListener {
    pub(in crate::net) fn with_challenges(mut self, catalog: Option<Arc<Catalog>>) -> Self {
        self.challenges.catalog = catalog;
        self
    }

    pub(super) fn prepare_challenges(
        &mut self,
        messages: &[frame::Message<'_>],
    ) -> Result<Option<State>, replication::Error> {
        self.challenges
            .prepare(self.persona, messages)
            .map_err(|reason| {
                if refusal::logged(self.refused + 1) {
                    tracing::warn!(
                        connection = self.unsupported.connection(),
                        ?reason,
                        "challenge request refused"
                    );
                }
                match reason {
                    Rejected::Unavailable => replication::Error::Unsupported,
                    Rejected::Queue => replication::Error::Bound,
                    Rejected::Identity | Rejected::Query => replication::Error::Shape,
                }
            })
    }

    pub(in crate::net) fn has_challenge_request(&self) -> bool {
        !self.challenges.pending.is_empty()
    }

    pub(in crate::net) fn challenges_loaded(
        &mut self,
        current: Current,
    ) -> Result<(), nfs_services::challenges::Error> {
        self.challenges.loaded(&current)?;
        tracing::info!(
            connection = self.unsupported.connection(),
            generation = current.generation(),
            entries = self.challenges.entries.len(),
            "owned challenge cache loaded"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;
