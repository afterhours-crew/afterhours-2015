// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::net::items::story::{Committed, Request};
use crate::progression::{Branch, entities::Role};
use nfs_world::replication::entity;

impl PlayerListener {
    pub(in crate::net) fn take_story(&mut self) -> Option<Request> {
        // Entry notifications and their referenced creations are ordering
        // barriers. Publish them before accepting the resulting durable step.
        if !self.pending_entries.is_empty() || !self.pending.is_empty() {
            return None;
        }
        self.story_pending.pop_front()
    }
    pub(in crate::net) fn story_committed(
        &mut self,
        request: &Request,
        committed: Committed,
    ) -> Result<(), replication::Error> {
        if !self
            .players
            .owns_participant(HOST_SELECTOR as u8, self.persona, request.participant)
        {
            return Err(replication::Error::UnknownObject);
        }
        let before = self
            .persistent
            .as_deref()
            .ok_or(replication::Error::Unsupported)?;
        if committed.loaded.generation() < before.generation() {
            return Err(replication::Error::Shape);
        }
        let calls = progression_calls(
            &request.progression,
            before,
            &committed.loaded,
            &self.players,
            request.participant,
        )?;
        if calls.len() > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len()) {
            return Err(replication::Error::Bound);
        }
        let mut population = self.population.clone();
        let inventory = self.inventory.as_ref().map(|old| Inventory {
            slots: old.slots,
            items: committed.items.clone(),
        });
        if let (Some(population), Some(old), Some(next)) =
            (&mut population, &self.inventory, &inventory)
        {
            population.refresh_inventory(
                &self.players,
                HOST_SELECTOR as u8,
                self.persona,
                old,
                next,
            )?;
        }
        self.pending_rpcs.extend(calls);
        self.population = population;
        self.inventory = inventory;
        self.persistent = Some(committed.loaded);
        Ok(())
    }
}

fn progression_calls(
    content: &crate::progression::Content,
    before: &crate::persistent::Loaded,
    after: &crate::persistent::Loaded,
    players: &Players,
    participant: u16,
) -> Result<Vec<HostRpc>, replication::Error> {
    let old = content
        .catalog
        .restore(before, content.settings)
        .map_err(|_| replication::Error::Shape)?;
    let new = content
        .catalog
        .restore(after, content.settings)
        .map_err(|_| replication::Error::Shape)?;
    let mut activated = vec![];
    let mut completed = vec![];
    for branch in [Branch::Intro, Branch::Main] {
        let blueprint = content
            .construction
            .blueprint(branch)
            .ok_or(replication::Error::Unsupported)?;
        let mut roots=players.objects().iter().filter_map(|(id,o)|match &o.initial {
            replication::Initial::Entity{prefix,fields} if prefix.asset==blueprint.asset() && matches!(fields.first(),Some(entity::Initial::Root{reference,..}) if *reference==participant)=>Some((id,fields)),_=>None
        });
        let (ghost, fields) = roots.next().ok_or(replication::Error::UnknownObject)?;
        if roots.next().is_some() {
            return Err(replication::Error::DuplicateObject);
        }
        for (index, role) in blueprint.roles().iter().enumerate() {
            let Role::Objective { id } = role else {
                continue;
            };
            let old = old
                .branch(branch)
                .objectives()
                .get(id)
                .ok_or(replication::Error::Shape)?;
            let new = new
                .branch(branch)
                .objectives()
                .get(id)
                .ok_or(replication::Error::Shape)?;
            let rpc = fields
                .get(index)
                .and_then(entity::Initial::rpc)
                .ok_or(replication::Error::TypeMismatch)?;
            let call = |method| {
                HostRpc::Query(nfs_world::participants::EntityCall {
                    entity: ghost,
                    selector: rpc.selector,
                    serial: rpc.serial,
                    method,
                })
            };
            if !old.active && new.active && !new.completed {
                activated.push(call(0));
            }
            if !old.completed && new.completed {
                completed.push(call(2));
            }
        }
    }
    activated.extend(completed);
    Ok(activated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_storage::{InventoryRepository, Timestamp};
    use std::sync::Arc;
    #[test]
    fn committed_objectives_publish_in_order_once_and_roll_back_under_pressure() {
        let (mut listener, participant, items) = super::super::tests::setup();
        let (repo, account, snapshot) = crate::progression::tests::local();
        let tables = crate::progression::tests::tables();
        let progression = Arc::new(
            crate::progression::Content::from_json(&crate::progression::tests::combined()).unwrap(),
        );
        let loaded = Arc::new(tables.view(&snapshot).unwrap());
        let restored = progression
            .catalog
            .restore(&loaded, progression.settings)
            .unwrap();
        let blueprint_scene = listener.players.objects().scene(909).unwrap();
        for branch in [Branch::Intro, Branch::Main] {
            let creation = progression
                .construction
                .build(
                    branch,
                    restored.branch(branch),
                    participant,
                    blueprint_scene,
                    |_, _| {
                        Some(crate::progression::entities::SceneEndpoint {
                            ghost: blueprint_scene,
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
        listener.persistent = Some(loaded.clone());
        let policy = progression.catalog.story_policy(progression.settings);
        let nfs_services::story::Prepared::Mutation(batch) = policy
            .complete(
                &snapshot,
                snapshot.generation,
                policy.next_operation(&snapshot).unwrap(),
                10,
                vec![],
            )
            .unwrap()
        else {
            panic!()
        };
        repo.apply(account, &batch, Timestamp(5)).unwrap();
        let after = Arc::new(tables.view(&repo.snapshot(account).unwrap()).unwrap());
        let request = Request {
            participant,
            player_index: 0,
            progression,
            action: crate::net::items::story::Action::Complete { objective: 10 },
        };
        let committed = Committed {
            loaded: after.clone(),
            items,
            notifications: vec![],
        };
        let call = nfs_world::participants::EntityCall {
            entity: blueprint_scene,
            selector: 1,
            serial: nfs_protocol::world::rpc::Serial::new(1).unwrap(),
            method: 0,
        };
        listener
            .pending_rpcs
            .resize(nfs_world::session::MAX_RPC_QUEUE, HostRpc::Query(call));
        assert_eq!(
            listener.story_committed(&request, committed.clone()),
            Err(replication::Error::Bound)
        );
        assert_eq!(
            listener.persistent.as_ref().unwrap().generation(),
            loaded.generation()
        );
        listener.pending_rpcs.clear();
        listener
            .story_committed(&request, committed.clone())
            .unwrap();
        assert_eq!(
            listener
                .pending_rpcs
                .iter()
                .map(|r| match r {
                    HostRpc::Query(c) => (c.selector, c.method),
                    _ => panic!(),
                })
                .collect::<Vec<_>>(),
            [(1, 0), (2, 2)]
        );
        listener.pending_rpcs.clear();
        listener.story_committed(&request, committed).unwrap();
        assert!(listener.pending_rpcs.is_empty());
        assert_eq!(
            listener.persistent.as_ref().unwrap().generation(),
            after.generation()
        );
    }
}
