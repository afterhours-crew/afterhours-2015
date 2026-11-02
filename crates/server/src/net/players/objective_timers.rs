// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    net::items::story::{Action, Request},
    progression::Branch,
};

impl PlayerListener {
    pub(super) fn advance_objective_timers(
        &mut self,
        world_ms: u64,
    ) -> Result<(), replication::Error> {
        let (Some(progression), Some(loaded)) = (&self.progression, &self.persistent) else {
            return Ok(());
        };
        if progression.objective_delays.is_empty() {
            return Ok(());
        }
        let owners: Vec<_> = self
            .players
            .player_participants()
            .filter(|&p| {
                self.players
                    .owns_participant(HOST_SELECTOR as u8, self.persona, p)
            })
            .filter(|&p| self.entered(p))
            .collect();
        let needs_snapshot = owners.iter().any(|p| {
            self.objective_timers
                .get(p)
                .is_none_or(|state| state.generation() != Some(loaded.generation()))
        });
        let restored = needs_snapshot
            .then(|| progression.catalog.restore(loaded, progression.settings))
            .transpose()
            .map_err(|_| replication::Error::Shape)?;
        let mut timers = self.objective_timers.clone();
        timers.retain(|p, _| owners.contains(p));
        let mut jobs = Vec::new();
        for participant in owners {
            let player_index = self
                .players
                .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
                .ok_or(replication::Error::UnknownObject)?;
            let capacity = MAX_PENDING
                .saturating_sub(self.story_pending.len())
                .saturating_sub(jobs.len());
            let due = timers.entry(participant).or_default().advance(
                &progression.objective_delays,
                loaded.generation(),
                world_ms,
                |id| {
                    restored
                        .as_ref()?
                        .branch(Branch::Main)
                        .objectives()
                        .get(&id)
                        .copied()
                },
                capacity,
            )?;
            for objective in due {
                jobs.push(Request {
                    participant,
                    player_index: u32::from(player_index),
                    progression: progression.clone(),
                    action: Action::Complete { objective },
                });
            }
        }
        self.objective_timers = timers;
        self.story_pending.extend(jobs);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_storage::{InventoryRepository, Timestamp};
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn elapsed_timer_commits_through_the_account_worker_before_world_publication() {
        let (mut listener, participant, _) = super::super::prologue::tests::setup();
        let (repository, account, snapshot) = crate::progression::tests::local();
        let repository = Arc::new(repository);
        let progression = Arc::new(
            crate::progression::Content::from_json(&crate::progression::tests::combined())
                .unwrap()
                .with_objective_delays(vec![nfs_content::progression::delays::Delay {
                    objective: 30,
                    duration: Duration::from_millis(1500),
                }])
                .unwrap(),
        );
        let tables = Arc::new(crate::progression::tests::tables());
        let service = crate::net::items::Service::new(
            repository.clone(),
            Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap()),
        )
        .with_persistent(tables.clone());
        let profile = service.bind(account);
        let mut request = Request {
            participant,
            player_index: 0,
            progression: progression.clone(),
            action: Action::Complete { objective: 10 },
        };
        let mut loaded = Arc::new(tables.view(&snapshot).unwrap());
        for objective in [10, 20] {
            request.action = Action::Complete { objective };
            loaded = profile
                .story_for_test(&request, Timestamp(10))
                .await
                .unwrap()
                .loaded;
        }
        listener.progression = Some(progression.clone());
        listener.persistent = Some(loaded.clone());
        assert!(listener.entered(participant));
        let restored = progression
            .catalog
            .restore(&loaded, progression.settings)
            .unwrap();
        let scene = listener.players.objects().scene(909).unwrap();
        for branch in [Branch::Intro, Branch::Main] {
            let creation = progression
                .construction
                .build(
                    branch,
                    restored.branch(branch),
                    participant,
                    scene,
                    |_, _| {
                        Some(crate::progression::entities::SceneEndpoint {
                            ghost: scene,
                            selector: 1,
                        })
                    },
                )
                .unwrap();
            listener
                .players
                .spawn_entities(vec![creation], progression.construction.content())
                .unwrap();
        }
        let before = repository.snapshot(account).unwrap();
        let calls = listener.pending_rpcs.len();
        listener.advance_objective_timers(100).unwrap();
        listener.advance_objective_timers(1599).unwrap();
        assert!(listener.story_pending.is_empty());
        listener.story_pending = VecDeque::from(vec![request; MAX_PENDING]);
        listener.advance_objective_timers(1600).unwrap();
        assert_eq!(listener.story_pending.len(), MAX_PENDING);
        listener.story_pending.clear();
        listener.advance_objective_timers(1601).unwrap();
        let request = listener.take_story().unwrap();
        assert_eq!(request.action, Action::Complete { objective: 30 });
        assert_eq!(request.participant, participant);
        assert_eq!(repository.snapshot(account).unwrap(), before);
        assert_eq!(listener.pending_rpcs.len(), calls);
        listener.advance_objective_timers(2000).unwrap();
        assert!(listener.take_story().is_none());
        let committed = profile
            .story_for_test(&request, Timestamp(20))
            .await
            .unwrap();
        assert!(
            progression
                .catalog
                .restore(&committed.loaded, progression.settings)
                .unwrap()
                .branch(Branch::Main)
                .objectives()[&30]
                .completed
        );
        // Durable state is committed, but this connection has not published it.
        assert_eq!(
            listener.persistent.as_ref().unwrap().generation(),
            loaded.generation()
        );
        listener
            .story_committed(&request, committed.clone())
            .unwrap();
        assert!(
            listener.pending_rpcs.iter().skip(calls).any(
                |call| matches!(call,nfs_world::participants::HostRpc::Query(q) if q.method==2)
            )
        );
        let published = listener.pending_rpcs.len();
        listener.story_committed(&request, committed).unwrap();
        assert_eq!(listener.pending_rpcs.len(), published);
        listener.advance_objective_timers(2001).unwrap();
        assert!(listener.take_story().is_none());
        let saved = repository.snapshot(account).unwrap();
        profile
            .story_for_test(&request, Timestamp(21))
            .await
            .unwrap();
        assert_eq!(repository.snapshot(account).unwrap(), saved);
    }
}
