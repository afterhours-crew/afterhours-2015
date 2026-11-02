// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::net::items::story::{Action, Request};
use nfs_services::messages::{dynamic::Identity, unlocks::Pending};
use nfs_world::scoring::delay::{Definition, Manager};

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    clock: Option<u64>,
    delays: Option<Manager<(u32, u64)>>,
    pending: BTreeMap<u32, u64>,
    ready: Vec<(u32, u64)>,
    queued: BTreeSet<(u32, u64)>,
    first: Option<BTreeMap<u32, Identity>>,
}

impl State {
    pub(super) fn failed(&mut self, hash: u32, signal: u64) {
        self.queued.remove(&(hash, signal));
    }
    pub(super) fn seed(
        &mut self,
        content: &crate::messages::Content,
        loaded: &crate::persistent::Loaded,
    ) -> Result<(), replication::Error> {
        if self.first.is_none() {
            let mut first = BTreeMap::new();
            for row in content.dynamic_identities(loaded)? {
                first.entry(row.row.hash).or_insert(row);
            }
            self.first = Some(first);
        }
        Ok(())
    }
    pub(super) fn added(&mut self, identity: Identity) {
        self.first
            .get_or_insert_default()
            .entry(identity.row.hash)
            .or_insert(identity);
    }
    pub(super) fn first(
        &self,
        hash: u32,
        content: &crate::messages::Content,
        loaded: &crate::persistent::Loaded,
    ) -> Result<Option<Identity>, replication::Error> {
        match &self.first {
            Some(first) => Ok(first.get(&hash).cloned()),
            None => Ok(content
                .dynamic_identities(loaded)?
                .into_iter()
                .find(|i| i.row.hash == hash)),
        }
    }

    /// Restart policy: a durable pending intent receives its full authored delay
    /// in the new session. No wall-clock deadline is fabricated from save time.
    fn advance(
        &mut self,
        content: &nfs_content::messages::unlocks::Delivery,
        rows: &[Pending],
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        if self.clock.is_some_and(|old| world_ms < old) {
            return Err(replication::Error::Shape);
        }
        if self.delays.is_none() {
            self.delays = Some(
                Manager::new(
                    content.notifications.len(),
                    &content
                        .notifications
                        .iter()
                        .enumerate()
                        .map(|(i, _)| {
                            (
                                i as u16,
                                Definition {
                                    auto_start: false,
                                    run_once: false,
                                    remove_duplicates: true,
                                },
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .map_err(|_| replication::Error::Shape)?,
            );
        }
        let delays = self.delays.as_mut().ok_or(replication::Error::Shape)?;
        if let Some(old) = self.clock {
            self.ready.extend(
                delays
                    .advance_owned((world_ms - old) as f32 / 1000.0)
                    .map_err(|_| replication::Error::Shape)?
                    .into_iter()
                    .map(|(_, context)| context),
            );
        }
        self.clock = Some(world_ms);
        let current: BTreeMap<_, _> = rows.iter().map(|p| (p.hash, p.signal)).collect();
        for (index, rule) in content.notifications.iter().enumerate() {
            let id = index as u16;
            let value = current.get(&rule.hash).copied();
            if value != self.pending.get(&rule.hash).copied() {
                self.ready.retain(|(hash, _)| *hash != rule.hash);
                self.queued.retain(|(hash, _)| *hash != rule.hash);
                match value {
                    Some(signal) => {
                        delays.replace_owned(id, rule.delay.as_secs_f32(), (rule.hash, signal))
                    }
                    None => delays.cancel_owned(id),
                }
                .map_err(|_| replication::Error::Shape)?;
            }
        }
        self.pending = current;
        Ok(())
    }
}

impl PlayerListener {
    pub(super) fn advance_unlock_notifications(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        let (Some(progression), Some(loaded)) = (&self.progression, &self.persistent) else {
            return Ok(());
        };
        let Some(content) = &progression.messages else {
            return Ok(());
        };
        let Some(delivery) = &content.unlocks else {
            return Ok(());
        };
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
        let policy = content
            .unlock_policy()
            .map_err(|_| replication::Error::Shape)?
            .ok_or(replication::Error::Shape)?;
        let rows = policy
            .pending(loaded.owned(nfs_services::messages::unlocks::TABLE))
            .map_err(|_| replication::Error::Shape)?;
        if rows.iter().any(|r| r.signal > loaded.generation()) {
            return Err(replication::Error::Shape);
        }
        let mut next = self.unlock_notifications.clone();
        next.seed(content, loaded)?;
        next.advance(delivery, &rows, world_ms)?;
        let capacity = MAX_PENDING.saturating_sub(self.story_pending.len());
        let mut jobs = vec![];
        for &(hash, signal) in &next.ready {
            if jobs.len() < capacity && next.queued.insert((hash, signal)) {
                jobs.push(Request {
                    participant,
                    player_index: u32::from(player_index),
                    progression: progression.clone(),
                    action: Action::UnlockMessage { hash, signal },
                });
            }
        }
        self.unlock_notifications = next;
        self.story_pending.extend(jobs);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
