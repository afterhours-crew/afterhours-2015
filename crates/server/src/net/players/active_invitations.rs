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
}

impl PlayerListener {
    /// Evaluate committed objective state only when its generation changes.
    /// Queue pressure defers a send; only the worker's commit announces it.
    pub(super) fn advance_active_invitations(&mut self) -> Result<(), replication::Error> {
        let (Some(progression), Some(loaded)) = (&self.progression, &self.persistent) else {
            return Ok(());
        };
        let Some(content) = &progression.messages else {
            return Ok(());
        };
        if content.active.is_empty() {
            return Ok(());
        }
        if content.active.len() > 64 {
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
        }
        let capacity = MAX_PENDING.saturating_sub(self.story_pending.len());
        let selected: Vec<_> = next
            .wanted
            .difference(&next.queued)
            .take(capacity)
            .copied()
            .collect();
        let jobs = selected
            .iter()
            .map(|&(manager, hash)| Request {
                participant,
                player_index: u32::from(player_index),
                progression: progression.clone(),
                action: Action::Message { hash, manager },
            })
            .collect::<Vec<_>>();
        next.queued.extend(selected);
        self.active_invitations = next;
        self.story_pending.extend(jobs);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
