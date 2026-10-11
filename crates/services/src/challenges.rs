// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Challenge reads from explicit local catalog/period data and committed account
//! history. Reading cannot grant rewards or advance progress. Native period/time
//! values are opaque configuration; no wall-clock conversion or rotation is guessed.
use nfs_protocol::challenge::*;
use nfs_storage::Snapshot;
use std::collections::{BTreeMap, BTreeSet};

mod content;
mod storage;
pub use content::{Award, Catalog, Definition, Rank};
pub use storage::ensure_empty;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    State,
    Ineligible,
    Encode,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "challenges {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    pub complete: bool,
    pub count_one: u32,
    pub count_two: u32,
}

/// Durable account history, scoped by the configured native day/month identity.
/// Empty history means no progress or obtained rewards under the local policy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    pub progress: BTreeMap<(u32, u32), Progress>,
    pub obtained: BTreeSet<(u32, u32, u64)>,
    pub monthly_ranks: BTreeMap<i64, u32>,
}

pub fn owns(component: u16, command: u16) -> bool {
    component == COMPONENT && command == GET_DAILY_CHALLENGES
}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 16 * 1024,
        max_depth: 4,
        max_values: 4096,
        max_collection: 64,
        max_byte_string: 128,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(16 * 1024 + 16, 0, 16 * 1024).expect("constant limits")
}

pub struct Current {
    generation: u64,
    state: State,
}
impl std::fmt::Debug for Current {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurrentChallenges")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
impl Current {
    pub fn from_snapshot(snapshot: &Snapshot) -> Result<Self, Error> {
        Ok(Self {
            generation: snapshot.generation,
            state: State::load(snapshot)?,
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn reply(&self, catalog: &Catalog, wire: &[u8], persona: i64) -> Result<Vec<u8>, Error> {
        let decoded = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let frame = decoded.frame;
        if decoded.consumed != wire.len()
            || !owns(frame.fields.routing_a, frame.fields.routing_b)
            || frame.fields.category != 0
            || frame.fields.slot != 0
            || frame.fields.reserved != [0, 0]
            || !frame.metadata.is_empty()
            || persona <= 0
        {
            return Err(Error::Ineligible);
        }
        let query = GetDailyChallengesRequest::decode(frame.body, body_limits())
            .map_err(|_| Error::Ineligible)?;
        if query.blaze_id != Some(persona)
            || query.debug_start_day != Some(0)
            || query.force_debug_start_day != Some(false)
            || query.unknown_field_count() != 0
            || query.encode(body_limits()).map_err(|_| Error::Ineligible)? != frame.body
        {
            return Err(Error::Ineligible);
        }
        let body = catalog.body(&self.state, persona)?;
        nfs_fire2::encode(
            nfs_fire2::Frame {
                fields: nfs_fire2::Fields {
                    category: 1,
                    ..frame.fields
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
}

impl Catalog {
    pub fn body(&self, state: &State, persona: i64) -> Result<Vec<u8>, Error> {
        if persona <= 0 {
            return Err(Error::Ineligible);
        }
        state.validate()?;
        // Each definition retains its explicit ordering; state joins by identity.
        let awards = self
            .awards
            .iter()
            .map(|(challenge, awards)| {
                (
                    *challenge,
                    awards
                        .iter()
                        .map(|a| AwardData {
                            id: Some(a.id),
                            item_guid: Some(a.item_guid.as_bytes()),
                            obtained: Some(state.obtained.contains(&(
                                self.day_id,
                                *challenge,
                                a.id,
                            ))),
                            kind: Some(a.kind),
                            unlock_value: Some(a.unlock_value),
                            ..Default::default()
                        })
                        .collect(),
                )
            })
            .collect();
        let progress = self
            .challenges
            .iter()
            .map(|challenge| {
                let value = state
                    .progress
                    .get(&(self.day_id, challenge.id))
                    .cloned()
                    .unwrap_or_default();
                ChallengeProgress {
                    complete: Some(value.complete),
                    current_count_one: Some(value.count_one),
                    current_count_two: Some(value.count_two),
                    id: Some(challenge.id),
                    ..Default::default()
                }
            })
            .collect();
        let challenges = self
            .challenges
            .iter()
            .map(|c| ChallengeInstance {
                car_id: Some(c.car_id),
                count_one: Some(c.count_one),
                count_two: Some(c.count_two),
                day: Some(c.day.as_bytes()),
                event_id: Some(c.event_id),
                id: Some(c.id),
                kind: Some(c.kind),
                weekly: Some(c.weekly),
                type_override_id: Some(c.type_override_id),
                ..Default::default()
            })
            .collect();
        let ranks = self
            .ranks
            .iter()
            .map(|r| MonthlyRankInstance {
                id: Some(r.id),
                rank_unlock: Some(r.unlock),
                ..Default::default()
            })
            .collect();
        GeneratedChallengesDataResponse {
            blaze_id: Some(persona),
            awards: Some(ChallengeAwards(awards)),
            day_id: Some(self.day_id),
            monthly_ranks: Some(MonthlyRanks(ranks)),
            progress: Some(ChallengeProgressList(progress)),
            monthly_rank: Some(
                state
                    .monthly_ranks
                    .get(&self.monthly_start)
                    .copied()
                    .unwrap_or(0),
            ),
            monthly_start: Some(self.monthly_start),
            challenges: Some(ChallengeInstances(challenges)),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)
    }
}

#[cfg(test)]
mod tests;
