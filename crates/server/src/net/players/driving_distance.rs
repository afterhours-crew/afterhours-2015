// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::items::{
    distance::Writer,
    story::{Action, Request},
};
use nfs_world::{
    driving_distance::{Metrics, Sample, Session},
    participants::HostRpc,
};

const MAX_LIFETIMES: usize = 16;

#[derive(Clone, Debug)]
struct Lifetime {
    ghost: u16,
    publishers: [u16; 2],
    car: Option<u16>,
    session: Session,
    sampled: bool,
    published: Option<Metrics>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    lifetimes: BTreeMap<u16, Lifetime>,
    writer: Writer,
    cumulative: i32,
    committed: Option<i32>,
    queued: Option<i32>,
    retry_at: u64,
    pub failures: usize,
}

impl State {
    fn failure(&mut self, world_ms: u64, error: replication::Error) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at = world_ms.saturating_add(1000);
        if refusal::logged(self.failures) {
            tracing::warn!(
                ?error,
                world_ms,
                failures = self.failures,
                "driving distance deferred; movement remains independent"
            );
        }
    }
}

impl PlayerListener {
    pub(super) fn validate_distance_commit(
        &self,
        action: &Action,
    ) -> Result<(), replication::Error> {
        if let Action::Distance { writer, cumulative } = action
            && (*writer != self.distance.writer
                || *cumulative < 0
                || *cumulative > self.distance.cumulative
                || (self.distance.queued != Some(*cumulative)
                    && self
                        .distance
                        .committed
                        .is_none_or(|done| done < *cumulative)))
        {
            return Err(replication::Error::Shape);
        }
        Ok(())
    }
    /// Sampling and publication are a side system. A fault is visible to the
    /// health gate, but cannot poison the client's reliable movement frame.
    pub(super) fn advance_driving_distance(&mut self, world_ms: u64) {
        self.prune_distance_publications();
        if world_ms < self.distance.retry_at {
            return;
        }
        if let Err(error) = self.stage_driving_distance(world_ms) {
            self.distance.failure(world_ms, error);
        }
    }

    fn stage_driving_distance(&mut self, world_ms: u64) -> Result<(), replication::Error> {
        let Some(logic) = self.logic.as_deref() else {
            return Ok(());
        };
        let Some(binding) = logic.driving_distance() else {
            return Ok(());
        };
        let Some(seconds) = self.event_seconds(world_ms) else {
            return Ok(());
        };
        let Some(loaded) = self.persistent.as_deref() else {
            return Ok(());
        };
        let Some(progression) = self.progression.clone() else {
            return Ok(());
        };
        let mut next = self.distance.clone();
        let mut live = BTreeSet::new();
        for (&participant, ghosts) in &self.logic_ghosts {
            let Some(&ghost) = ghosts.get(&binding.role) else {
                continue;
            };
            if !self
                .players
                .owns_participant(HOST_SELECTOR as u8, self.persona, participant)
            {
                continue;
            }
            let publishers = logic.distance_publishers(&self.players, ghost, participant)?;
            live.insert(participant);
            if !next
                .lifetimes
                .get(&participant)
                .is_some_and(|life| life.ghost == ghost)
            {
                if !next.lifetimes.contains_key(&participant)
                    && next.lifetimes.len() >= MAX_LIFETIMES
                {
                    return Err(replication::Error::Bound);
                }
                next.lifetimes.insert(
                    participant,
                    Lifetime {
                        ghost,
                        publishers,
                        car: None,
                        session: Session::new(0, seconds, f64::from(binding.period_seconds))
                            .map_err(|_| replication::Error::Shape)?,
                        sampled: false,
                        published: None,
                    },
                );
            }
            let life = next
                .lifetimes
                .get_mut(&participant)
                .expect("inserted lifetime");
            let mut cars = self
                .world_cars
                .iter()
                .filter(|(_, (p, ready))| *p == participant && *ready);
            let car = cars.next().map(|(&car, _)| car);
            if cars.next().is_some() {
                return Err(replication::Error::DuplicateObject);
            }
            // Explicit lifecycle relocation, never a guessed maximum speed.
            let relocating = self
                .pending_teleports
                .iter()
                .any(|(_, c)| c.participant == participant)
                || matches!(
                    self.participants.stage(participant),
                    Some(
                        nfs_world::participants::Stage::Relocating
                            | nfs_world::participants::Stage::IntroRelocating
                            | nfs_world::participants::Stage::IntroTeleporting
                    )
                );
            let sample = if relocating {
                None
            } else {
                car.map(|car| {
                    self.players
                        .vehicle_physics(HOST_SELECTOR as u8, self.persona, car)
                        .and_then(|physics| {
                            Sample::from_physics(&physics).map_err(|_| replication::Error::Shape)
                        })
                })
                .transpose()?
            };
            if life.car != car || relocating {
                life.session
                    .rebase(seconds, sample)
                    .map_err(|_| replication::Error::Shape)?;
                life.car = car;
            } else {
                let before = life
                    .session
                    .metres()
                    .map_err(|_| replication::Error::Shape)?;
                if let Some(total) = life
                    .session
                    .advance(seconds, sample)
                    .map_err(|_| replication::Error::Shape)?
                {
                    next.cumulative = next
                        .cumulative
                        .checked_add(total - before)
                        .ok_or(replication::Error::Bound)?;
                    life.sampled = sample.is_some();
                }
            }
        }
        next.lifetimes
            .retain(|participant, _| live.contains(participant));
        let needs_commit = next.committed != Some(next.cumulative)
            && (next.cumulative > 0 || next.lifetimes.values().any(|life| life.sampled));
        let request =
            if needs_commit && next.queued.is_none() && self.story_pending.len() < MAX_PENDING {
                // A retiring logic may leave uncommitted movement; the participant
                // remains the account owner during garage load and flushes it here.
                self.players
                    .player_participants()
                    .find(|p| {
                        self.players
                            .owns_participant(HOST_SELECTOR as u8, self.persona, *p)
                    })
                    .map(|participant| {
                        let player_index = self
                            .players
                            .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
                            .ok_or(replication::Error::UnknownObject)?;
                        next.queued = Some(next.cumulative);
                        Ok(Request {
                            participant,
                            player_index: u32::from(player_index),
                            progression,
                            action: Action::Distance {
                                writer: next.writer.clone(),
                                cumulative: next.cumulative,
                            },
                        })
                    })
                    .transpose()?
            } else {
                None
            };
        // Creation/deletion output is an ordering barrier for these references.
        let mut calls = Vec::new();
        // Publish the latest committed total even if newer movement is waiting
        // for storage. Requiring a completely drained backlog starves updates
        // whenever a commit takes longer than one sampling interval.
        if next.committed.is_some() && self.pending_entries.is_empty() && self.pending.is_empty() {
            let metres = loaded
                .stores()
                .get(&(
                    crate::persistent::key(nfs_services::driving_distance::STATS),
                    crate::persistent::key(nfs_services::driving_distance::COLUMN),
                ))
                .copied()
                .ok_or(replication::Error::Shape)?;
            let values = Metrics::from_metres(metres, binding.metres_per_mile)
                .map_err(|_| replication::Error::Shape)?;
            let capacity =
                nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len());
            for (&participant, life) in &mut next.lifetimes {
                if !life.sampled {
                    continue;
                }
                let selectors =
                    logic.distance_publishers(&self.players, life.ghost, participant)?;
                let Some(replication::Initial::Participant(owner)) = self
                    .players
                    .objects()
                    .get(participant)
                    .map(|object| &object.initial)
                else {
                    return Err(replication::Error::TypeMismatch);
                };
                let changed = [
                    (
                        selectors[0],
                        values.kilometres,
                        life.published.map(|v| v.kilometres),
                    ),
                    (selectors[1], values.miles, life.published.map(|v| v.miles)),
                ];
                let count = changed
                    .iter()
                    .filter(|(_, value, old)| Some(*value) != *old)
                    .count();
                if count > capacity.saturating_sub(calls.len()) {
                    break;
                }
                for (selector, value, old) in changed {
                    if old != Some(value) {
                        calls.push(HostRpc::GameMode(crate::event_logic::call(
                            self.players.objects(),
                            life.ghost,
                            selector,
                            0,
                            Some(owner.player),
                            nfs_world::game_mode::Arguments::Word(value as u32),
                        )?));
                    }
                }
                life.published = Some(values);
            }
        }
        self.distance = next;
        self.pending_rpcs.extend(calls);
        self.story_pending.extend(request);
        Ok(())
    }

    pub(super) fn distance_committed(&mut self, action: &Action) {
        if let Action::Distance { writer, cumulative } = action
            && *writer == self.distance.writer
        {
            self.distance.committed = Some(self.distance.committed.unwrap_or(0).max(*cumulative));
            if self.distance.queued == Some(*cumulative) {
                self.distance.queued = None;
            }
        }
    }

    pub(super) fn distance_failed(&mut self, action: &Action) {
        if let Action::Distance { writer, cumulative } = action
            && *writer == self.distance.writer
        {
            if self.distance.queued == Some(*cumulative) {
                self.distance.queued = None;
            }
            self.distance
                .failure(self.world_ms, replication::Error::Shape);
        }
    }

    /// A deletion may be admitted after a stat was queued but before output
    /// capacity became available. Do not send that stat after its target dies.
    pub(in crate::net) fn prune_distance_publications(&mut self) {
        for (&participant, life) in &self.distance.lifetimes {
            let current = self
                .logic
                .as_ref()
                .and_then(|logic| logic.driving_distance())
                .and_then(|binding| {
                    self.logic_ghosts
                        .get(&participant)?
                        .get(&binding.role)
                        .copied()
                });
            if current == Some(life.ghost) && self.players.objects().get(life.ghost).is_some() {
                continue;
            }
            self.pending_rpcs.retain(|call| !matches!(call, HostRpc::GameMode(call)
                if call.target == life.ghost && call.method == 0 && life.publishers.contains(&call.selector)));
        }
    }
}

#[cfg(test)]
mod tests;
