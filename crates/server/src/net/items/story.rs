// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::net) enum Action {
    Complete {
        objective: u64,
    },
    Choose {
        sequence: u16,
        node: u16,
        objective: u64,
        definition: [u8; 16],
    },
}
#[derive(Clone, Debug)]
pub(in crate::net) struct Request {
    pub participant: u16,
    pub player_index: u32,
    pub progression: Arc<crate::progression::Content>,
    pub action: Action,
}
#[derive(Clone)]
pub(in crate::net) struct Committed {
    pub loaded: Arc<crate::persistent::Loaded>,
    pub items: Arc<nfs_world::items::Collection>,
    pub notifications: Vec<Vec<u8>>,
    pub reward: Option<nfs_services::story::rewards::Receipt>,
}

impl Profile {
    pub(super) async fn story(
        &self,
        request: &Request,
        now: Timestamp,
    ) -> Result<Committed, Error> {
        let service = self.service.clone();
        let account = self.account;
        let request = request.clone();
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .acquire_owned()
                .await
                .map_err(|_| Error::Worker)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let tables = service.persistent.as_deref().ok_or(Error::Worker)?;
                let initial = inventory::load(
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    1,
                    now,
                )
                .map_err(Error::Inventory)?;
                let before = inventory::collection(&initial, service.catalog.bindings())
                    .map_err(Error::Inventory)?;
                let policy = request
                    .progression
                    .catalog
                    .story_policy(request.progression.settings);
                let mut reward = None;
                let snapshot = match request.action {
                    Action::Choose {
                        objective,
                        definition,
                        ..
                    } => {
                        let claimed = policy
                            .claim_vehicle(
                                service.repository.as_ref(),
                                account,
                                objective,
                                definition,
                                &service.catalog,
                                now,
                            )
                            .map_err(Error::Inventory)?;
                        reward = Some(claimed.receipt);
                        let mut result = None;
                        for _ in 0..8 {
                            let snapshot = service
                                .repository
                                .snapshot(account)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            match policy
                                .select_reward(&snapshot, objective, claimed.receipt)
                                .map_err(|e| Error::Inventory(e.into()))?
                            {
                                nfs_services::story::Prepared::Replayed => {
                                    result = Some(snapshot);
                                    break;
                                }
                                nfs_services::story::Prepared::Mutation(batch) => {
                                    match service.repository.apply(account, &batch, now) {
                                        Ok(_) | Err(nfs_storage::Error::Conflict) => continue,
                                        Err(e) => return Err(Error::Inventory(e.into())),
                                    }
                                }
                            }
                        }
                        result.ok_or(Error::Inventory(nfs_storage::Error::Conflict.into()))?
                    }
                    Action::Complete { objective } => {
                        let mut result = None;
                        for _ in 0..8 {
                            let snapshot = service
                                .repository
                                .snapshot(account)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let view = policy
                                .inspect(&snapshot)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            if view
                                .milestones
                                .iter()
                                .any(|m| m.id == objective && m.completed)
                            {
                                result = Some(snapshot);
                                break;
                            }
                            let operation = policy
                                .next_operation(&snapshot)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let prepared = policy
                                .complete(
                                    &snapshot,
                                    snapshot.generation,
                                    operation,
                                    objective,
                                    vec![],
                                )
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let nfs_services::story::Prepared::Mutation(batch) = prepared else {
                                return Err(Error::Worker);
                            };
                            match service.repository.apply(account, &batch, now) {
                                Ok(_) => continue, // verify the semantic objective after a retry-key collision
                                Err(nfs_storage::Error::Conflict) => continue,
                                Err(e) => return Err(Error::Inventory(e.into())),
                            }
                        }
                        result.ok_or(Error::Inventory(nfs_storage::Error::Conflict.into()))?
                    }
                };
                let loaded = Arc::new(
                    tables
                        .view(&snapshot)
                        .map_err(|e| Error::Inventory(e.into()))?,
                );
                let items = Arc::new(
                    inventory::collection(&snapshot, service.catalog.bindings())
                        .map_err(Error::Inventory)?,
                );
                // A story award must deliver its complete item tree before the
                // sequence callback. The connection cannot acknowledge a subset.
                let (notifications, unannounced) = credit_notifications(
                    request.player_index,
                    &before,
                    &items,
                    service.catalog.bindings(),
                )
                .map_err(|e| Error::Inventory(e.into()))?;
                if unannounced != 0 {
                    return Err(Error::Worker);
                }
                Ok(Committed {
                    loaded,
                    items,
                    notifications,
                    reward,
                })
            })
            .await
            .map_err(|_| Error::Worker)?
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn selection_worker_grants_selects_and_completes_before_acknowledgement() {
        use nfs_world::items::{
            Definition, DefinitionClass, DefinitionFlags, Derived, Layout, OwnershipLevel,
        };
        let (repository, account, _) = crate::progression::tests::local();
        let repository = Arc::new(repository);
        let catalog = Arc::new(
            InventoryCatalog::new(
                [(
                    [9; 16],
                    Definition {
                        class: DefinitionClass::RaceVehicleItemData,
                        buy_price: 100,
                        sell_price: 50,
                        quantity: u32::MAX,
                        ownership_level: OwnershipLevel::Claimable,
                        flags: DefinitionFlags {
                            uses_scope: true,
                            ..Default::default()
                        },
                        sub_items: vec![],
                        additional_items: vec![],
                        default_derived: Some(
                            Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap(),
                        ),
                    },
                )],
                vec![],
            )
            .unwrap(),
        );
        let service = Service::new(repository.clone(), catalog)
            .with_persistent(Arc::new(crate::progression::tests::tables()));
        let request = Request {
            participant: 19,
            player_index: 4,
            progression: Arc::new(
                crate::progression::Content::from_json(&crate::progression::tests::combined())
                    .unwrap(),
            ),
            action: Action::Choose {
                sequence: 50,
                node: 7,
                objective: 10,
                definition: [9; 16],
            },
        };
        let committed = service
            .bind(account)
            .story(&request, Timestamp(8))
            .await
            .unwrap();
        let reward = committed.reward.unwrap();
        assert_eq!(committed.loaded.garage()[0], Some(reward.item.get()));
        assert_eq!(
            committed.items.items[&reward.item.get()].definition,
            [9; 16]
        );
        assert!(!committed.notifications.is_empty());
        let saved = repository.snapshot(account).unwrap();
        assert!(
            request
                .progression
                .catalog
                .story_policy(request.progression.settings)
                .inspect(&saved)
                .unwrap()
                .milestones[0]
                .completed
        );
        let retry = service
            .bind(account)
            .story(&request, Timestamp(9))
            .await
            .unwrap();
        assert_eq!(retry.reward, Some(reward));
        assert!(retry.notifications.is_empty());
        assert_eq!(repository.snapshot(account).unwrap(), saved);
    }
    #[tokio::test]
    async fn completion_uses_the_account_worker_and_exact_retries_are_inert() {
        let (repository, account, before) = crate::progression::tests::local();
        let repository = Arc::new(repository);
        let catalog = Arc::new(InventoryCatalog::new([], vec![]).unwrap());
        let service = Service::new(repository.clone(), catalog)
            .with_persistent(Arc::new(crate::progression::tests::tables()));
        let profile = service.bind(account);
        let request = Request {
            participant: 19,
            player_index: 4,
            progression: Arc::new(
                crate::progression::Content::from_json(&crate::progression::tests::combined())
                    .unwrap(),
            ),
            action: Action::Complete { objective: 10 },
        };
        let committed = profile.story(&request, Timestamp(9)).await.unwrap();
        assert!(committed.notifications.is_empty());
        assert!(committed.loaded.generation() > before.generation);
        let saved = repository.snapshot(account).unwrap();
        let repeated = profile.story(&request, Timestamp(10)).await.unwrap();
        assert_eq!(saved, repository.snapshot(account).unwrap());
        assert_eq!(repeated.loaded.generation(), committed.loaded.generation());
    }
}
