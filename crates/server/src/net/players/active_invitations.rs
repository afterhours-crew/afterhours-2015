// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    messages::Manager,
    net::items::story::{Action, Request},
    progression::Branch,
};

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    generation: Option<u64>,
    wanted: BTreeSet<(Manager, u32)>,
    queued: BTreeSet<(Manager, u32)>,
    power: BTreeMap<(Manager, u64), Choice>,
}

#[derive(Clone, Debug)]
struct Choice {
    started_ms: u64,
    hash: Option<u32>,
    queued: bool,
}
impl Choice {
    fn select(
        &mut self,
        plan: &nfs_content::messages::power::Invitation,
        world_ms: u64,
        power: Option<(u32, u32)>,
    ) -> Result<(), replication::Error> {
        if world_ms < self.started_ms {
            return Err(replication::Error::Shape);
        }
        if self.hash.is_none() {
            self.hash = match power {
                Some((current, stock)) => Some(
                    plan.choose(current, stock)
                        .map_err(|_| replication::Error::Shape)?,
                ),
                None if u128::from(world_ms - self.started_ms)
                    >= plan.default_after().as_nanos().div_ceil(1_000_000) =>
                {
                    Some(plan.hashes()[0])
                }
                None => None,
            };
        }
        Ok(())
    }
}

impl PlayerListener {
    /// Evaluate committed objective state only when its generation changes.
    /// Queue pressure defers a send; only the worker's commit announces it.
    pub(super) fn advance_active_invitations(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        if world_ms < self.world_ms {
            return Err(replication::Error::Shape);
        }
        let (Some(progression), Some(loaded)) = (&self.progression, &self.persistent) else {
            return Ok(());
        };
        let Some(content) = &progression.messages else {
            return Ok(());
        };
        if content.active.is_empty() && content.power.is_empty() {
            return Ok(());
        }
        if content.active.len() > 64 || content.power.len() > 16 {
            return Err(replication::Error::Bound);
        }
        let Some(participant) = self
            .players
            .player_participants()
            .filter(|&p| {
                self.players
                    .owns_participant(HOST_SELECTOR as u8, self.persona, p)
            })
            .find(|&p| self.entered(p))
        else {
            return Ok(());
        };
        let player_index = self
            .players
            .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
            .ok_or(replication::Error::UnknownObject)?;
        let mut next = self.active_invitations.clone();
        if next.generation.is_some_and(|g| g > loaded.generation()) {
            return Err(replication::Error::Shape);
        }
        if next.generation != Some(loaded.generation()) {
            let restored = progression
                .catalog
                .restore(loaded, progression.settings)
                .map_err(|_| replication::Error::Shape)?;
            let normal = nfs_services::messages::rows(loaded.tables(), Manager::Normal.table())
                .map_err(|_| replication::Error::Shape)?;
            let underground =
                nfs_services::messages::rows(loaded.tables(), Manager::Underground.table())
                    .map_err(|_| replication::Error::Shape)?;
            let held: BTreeSet<_> = normal
                .iter()
                .map(|r| (Manager::Normal, r.hash))
                .chain(underground.iter().map(|r| (Manager::Underground, r.hash)))
                .collect();
            let mut wanted = BTreeSet::new();
            for invitation in &content.active {
                let manager = content
                    .manager(invitation.manager)
                    .ok_or(replication::Error::Shape)?;
                let objective = restored
                    .branch(Branch::Main)
                    .objectives()
                    .get(&invitation.objective)
                    .ok_or(replication::Error::UnknownObject)?;
                if objective.active && !held.contains(&(manager, invitation.hash)) {
                    wanted.insert((manager, invitation.hash));
                }
            }
            next.wanted = wanted;
            next.queued.retain(|key| !held.contains(key));
            next.generation = Some(loaded.generation());
            for plan in &content.power {
                let manager = content
                    .manager(plan.manager())
                    .ok_or(replication::Error::Shape)?;
                let state = restored
                    .branch(Branch::Main)
                    .objectives()
                    .get(&plan.objective())
                    .ok_or(replication::Error::UnknownObject)?;
                let held_count = plan
                    .hashes()
                    .iter()
                    .filter(|h| held.contains(&(manager, **h)))
                    .count();
                if held_count > 1 {
                    return Err(replication::Error::Shape);
                }
                let key = (manager, plan.objective());
                if state.active && held_count == 0 {
                    next.power.entry(key).or_insert(Choice {
                        started_ms: world_ms,
                        hash: None,
                        queued: false,
                    });
                } else {
                    next.power.remove(&key);
                }
            }
        }
        let capacity = MAX_PENDING.saturating_sub(self.story_pending.len());
        let selected: Vec<_> = next
            .wanted
            .difference(&next.queued)
            .take(capacity)
            .copied()
            .collect();
        let mut jobs = selected
            .iter()
            .map(|&(manager, hash)| Request {
                participant,
                player_index: u32::from(player_index),
                progression: progression.clone(),
                action: Action::Message { hash, manager },
            })
            .collect::<Vec<_>>();
        next.queued.extend(selected);
        // Resolve the participant's current owned vehicle, not an arbitrary
        // account car. No vehicle yet is an authored delayed-default case;
        // a present vehicle with invalid/missing statistics remains an error.
        let power = if next.power.values().any(|p| p.hash.is_none()) {
            self.world_cars
                .iter()
                .find(|(_, (owner, ready))| *owner == participant && *ready)
                .map(|(&car, _)| {
                    let scoring = self
                        .scoring
                        .as_ref()
                        .ok_or(replication::Error::Unsupported)?;
                    let stats = self
                        .vehicle_stats(scoring, &self.players, car)
                        .ok_or(replication::Error::Shape)?;
                    Ok::<_, replication::Error>((stats.max_hp, stats.stock_max_hp))
                })
                .transpose()?
        } else {
            None
        };
        for plan in &content.power {
            let manager = content
                .manager(plan.manager())
                .ok_or(replication::Error::Shape)?;
            let Some(choice) = next.power.get_mut(&(manager, plan.objective())) else {
                continue;
            };
            choice.select(plan, world_ms, power)?;
            if let Some(hash) = choice.hash
                && !choice.queued
                && jobs.len() < MAX_PENDING.saturating_sub(self.story_pending.len())
            {
                jobs.push(Request {
                    participant,
                    player_index: u32::from(player_index),
                    progression: progression.clone(),
                    action: Action::PowerMessage {
                        objective: plan.objective(),
                        hash,
                        manager,
                    },
                });
                choice.queued = true;
            }
        }
        self.active_invitations = next;
        self.story_pending.extend(jobs);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
