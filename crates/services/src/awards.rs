// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Current-account menu statistics. Progress belongs to persistent tables;
//! records, social counters and entitlement declarations belong to a separate
//! versioned account document. No response bytes or imported account template
//! are retained. Updating gameplay progress/rewards is a separate service.
use crate::{
    persistent::{Loaded, key},
    reputation::{self, Field, Thresholds},
};
use nfs_protocol::autolog::*;
use nfs_storage::{
    Snapshot,
    tables::{Row, Value},
};
use std::collections::BTreeMap;

mod storage;
pub use storage::ensure_empty;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    State,
    Ineligible,
    Encode,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "awards {self:?}")
    }
}
impl std::error::Error for Error {}

/// Metadata for a local record. Zero timestamp/id and empty name denote no
/// associated record; no external screenshot or record is manufactured.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Record {
    pub name: String,
    pub screenshot: u64,
    pub modified: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Event {
    pub attempts: u32,
    pub position: u32,
    pub record: Record,
    pub kind: String,
}
/// Supplementary account data. This deliberately does not duplicate any table
/// score, objective flag, collection flag, medal or SpeedList count.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    pub activities: BTreeMap<u32, Record>,
    pub collectibles: BTreeMap<u32, Record>,
    /// Stable account record order, with unique event IDs. The wire list does
    /// not impose numeric event-ID order.
    pub events: Vec<(u32, Event)>,
    pub objective_times: BTreeMap<u64, u64>,
    pub rep_modified: u64,
    /// Received likes, reward level, sent likes, screenshot count.
    pub kickbacks: [u32; 4],
    /// Account declarations only; this service cannot grant external ownership.
    pub entitlements: BTreeMap<i32, bool>,
}

pub fn owns(component: u16, command: u16) -> bool {
    component == COMPONENT && command == GET_STATS_AND_AWARDS
}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 64 * 1024,
        max_depth: 4,
        max_values: 16 * 1024,
        max_collection: 512,
        max_byte_string: 1024,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(64 * 1024 + 32, 16, 64 * 1024).expect("constant limits")
}
fn integer(row: &Row, column: &str) -> Result<i32, Error> {
    match row.get(&key(column)) {
        Some(Value::Int(v)) => Ok(*v),
        _ => Err(Error::State),
    }
}
fn flag(row: &Row, column: &str) -> Result<bool, Error> {
    match integer(row, column)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::State),
    }
}
fn nonnegative(value: i32) -> Result<u32, Error> {
    value.try_into().map_err(|_| Error::State)
}

/// A complete committed view. Construct it from the *same* repository snapshot
/// used for Loaded, never from a second read that could race a transaction.
pub struct Current {
    loaded: Loaded,
    state: State,
    reputation: reputation::Values,
}
impl Current {
    pub fn from_snapshot(
        snapshot: &Snapshot,
        loaded: Loaded,
        thresholds: &Thresholds,
    ) -> Result<Self, Error> {
        if snapshot.generation != loaded.generation() {
            return Err(Error::State);
        }
        let state = State::load(snapshot)?;
        let reputation =
            thresholds.evaluate(reputation::Scores::load(&loaded).map_err(|_| Error::State)?);
        Ok(Self {
            loaded,
            state,
            reputation,
        })
    }
    pub fn generation(&self) -> u64 {
        self.loaded.generation()
    }
    pub fn reply(&self, wire: &[u8], persona: i64) -> Result<Vec<u8>, Error> {
        let d = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = d.frame;
        if d.consumed != wire.len()
            || !owns(f.fields.routing_a, f.fields.routing_b)
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
            || persona <= 0
        {
            return Err(Error::Ineligible);
        }
        let q =
            StatsAndAwardsRequest::decode(f.body, body_limits()).map_err(|_| Error::Ineligible)?;
        if q.blaze_id != Some(persona)
            || q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
        {
            return Err(Error::Ineligible);
        }
        let body = self.body(persona)?;
        nfs_fire2::encode(
            nfs_fire2::Frame {
                fields: nfs_fire2::Fields {
                    category: 1,
                    ..f.fields
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
    fn rows(&self, table: &str) -> Result<&BTreeMap<u64, Row>, Error> {
        let rows = &self
            .loaded
            .tables()
            .get(&key(table))
            .ok_or(Error::State)?
            .rows;
        if rows.len() > 512 {
            return Err(Error::State);
        }
        Ok(rows)
    }
    fn scalar(&self, table: &str, column: &str) -> Result<i32, Error> {
        self.loaded
            .stores()
            .get(&(key(table), key(column)))
            .copied()
            .ok_or(Error::State)
    }
    fn body(&self, persona: i64) -> Result<Vec<u8>, Error> {
        let empty = Record::default();
        let mut activities = Vec::new();
        let activity_rows = self.rows("ActivitiesTable")?;
        if self
            .state
            .activities
            .keys()
            .any(|id| !activity_rows.contains_key(&u64::from(*id)))
        {
            return Err(Error::State);
        }
        for (&id, row) in activity_rows {
            let id = u32::try_from(id).map_err(|_| Error::State)?;
            let r = self.state.activities.get(&id).unwrap_or(&empty);
            activities.push(StatsAndAwardsActivity {
                collected: Some(flag(row, "Collected")?),
                persistence_key: Some(id),
                record_name: Some(r.name.as_bytes()),
                screenshot_id: Some(r.screenshot),
                last_modified: Some(r.modified),
                ..Default::default()
            });
        }
        let mut collectibles = Vec::new();
        let collectible_rows = self.rows("CollectiblesTable")?;
        if self
            .state
            .collectibles
            .keys()
            .any(|id| !collectible_rows.contains_key(&u64::from(*id)))
        {
            return Err(Error::State);
        }
        for (&id, row) in collectible_rows {
            let id = u32::try_from(id).map_err(|_| Error::State)?;
            let r = self.state.collectibles.get(&id).unwrap_or(&empty);
            collectibles.push(StatsAndAwardsCollectible {
                collected: Some(flag(row, "Collected")?),
                persistence_key: Some(id),
                record_name: Some(r.name.as_bytes()),
                screenshot_id: Some(r.screenshot),
                last_modified: Some(r.modified),
                ..Default::default()
            });
        }
        let mut objectives = Vec::new();
        let objective_rows = self.rows("ProgressionObjective")?;
        if self
            .state
            .objective_times
            .keys()
            .any(|id| !objective_rows.contains_key(id))
        {
            return Err(Error::State);
        }
        for (&id, row) in objective_rows {
            objectives.push(StatsAndAwardsProgressionObjective {
                active: Some(flag(row, "Active")?),
                completed: Some(flag(row, "Completed")?),
                persistence_key: Some(id),
                last_modified: Some(*self.state.objective_times.get(&id).unwrap_or(&0)),
                ..Default::default()
            });
        }
        let events = self
            .state
            .events
            .iter()
            .map(|(id, e)| StatsAndAwardsEvent {
                attempts: Some(e.attempts),
                position: Some(e.position),
                event_id: Some(*id),
                record_name: Some(e.record.name.as_bytes()),
                screenshot_id: Some(e.record.screenshot),
                last_modified: Some(e.record.modified),
                type_string: Some(e.kind.as_bytes()),
                ..Default::default()
            })
            .collect();
        let general = |name: &str| {
            self.scalar("GameplayStatsTable", name)
                .and_then(nonnegative)
        };
        let medal = |name: &str| self.scalar("PrestigeMedalTable", name);
        let speedlist = |name: &str| {
            self.scalar("SpeedListStatsTable", name)
                .and_then(nonnegative)
        };
        let rep = |field| nonnegative(self.reputation.get(field));
        let positions = (1..=8)
            .map(|p| Ok((p, speedlist(&format!("P{p}"))?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let [received, reward, sent, screenshots] = self.state.kickbacks;
        StatsAndAwardsResponse {
            blaze_id: Some(persona),
            activities: Some(StatsAndAwardsActivities(activities)),
            collectibles: Some(StatsAndAwardsCollectibles(collectibles)),
            race_events: Some(StatsAndAwardsEvents(events)),
            progression_objectives: Some(StatsAndAwardsObjectives(objectives)),
            general_stats: Some(StatsAndAwardsGeneralStats {
                biggest_fine_escaped: Some(general("LargestFineEscaped")?),
                biggest_fine: Some(general("LargestFineBusted")?),
                cash_earned: Some(general("CashEarned")?),
                distance_driven: Some(general("DistanceDriven")?),
                distance_drifted: Some(general("DistanceDrifted")?),
                // The persisted i32 is a bit-preserving identifier, not a signed counter.
                favorite_car_id: Some(
                    self.scalar("GameplayStatsTable", "FavoriteVehicleId")? as u32
                ),
                time_played: Some(u64::from(general("TotalTimePlayed")?)),
                top_speed: Some(general("TopSpeed")?),
                ..Default::default()
            }),
            kickbacks: Some(StatsAndAwardsKickbacks {
                total_received_likes: Some(received),
                reward_level: Some(reward),
                total_sent_likes: Some(sent),
                screenshot_count: Some(screenshots),
                ..Default::default()
            }),
            prestige_medal_stats: Some(PrestigeMedalStats {
                build: Some(medal("BuildMedal")?),
                crew: Some(medal("CrewMedal")?),
                final_event: Some(medal("FinalEventMedal")?),
                outlaw: Some(medal("OutlawMedal")?),
                speed: Some(medal("SpeedMedal")?),
                style: Some(medal("StyleMedal")?),
                ..Default::default()
            }),
            rep_scores: Some(StatsAndAwardsRepScores {
                build_score: Some(rep(Field::Build)?),
                crew_score: Some(rep(Field::Crew)?),
                outlaw_score: Some(rep(Field::Outlaw)?),
                rep_level: Some(rep(Field::Level)?),
                rep_score: Some(rep(Field::Total)?),
                speed_score: Some(rep(Field::Speed)?),
                style_score: Some(rep(Field::Style)?),
                last_modified: Some(self.state.rep_modified),
                ..Default::default()
            }),
            speed_list_stats: Some(SpeedListStats {
                finished_count: Some(speedlist("Finished")?),
                positions_breakdown: Some(PositionsBreakdown(positions)),
                started_count: Some(speedlist("Started")?),
                ..Default::default()
            }),
            entitlements: Some(StatsAndAwardsEntitlements(
                self.state
                    .entitlements
                    .iter()
                    .map(|(&k, &v)| (k, v))
                    .collect(),
            )),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)
    }
}

#[cfg(test)]
mod tests;
