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
    /// A unique phone message joins its manager's table; the world
    /// announces it only after this commit.
    Message {
        hash: u32,
        manager: crate::messages::Manager,
    },
    /// One authored conditional invitation. The worker checks the restrictor
    /// against its latest snapshot, so concurrent sessions cannot send two tiers.
    ConditionalMessage {
        objective: u64,
        hash: u32,
        manager: crate::messages::Manager,
    },
    /// The client reported a stored message's read state.
    MessageRead {
        hash: u32,
        manager: crate::messages::Manager,
        read: bool,
    },
    UnlockMessage {
        hash: u32,
        signal: u64,
    },
    DynamicMessageRead {
        identity: nfs_services::messages::dynamic::Identity,
        read: bool,
    },
    /// The client accepted a tutorial: its seen value becomes durable.
    Tutorial {
        key: u32,
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
    #[cfg(test)]
    pub(in crate::net) async fn story_for_test(
        &self,
        request: &Request,
        now: Timestamp,
    ) -> Result<Committed, String> {
        self.story(request, now)
            .await
            .map_err(|error| format!("{error:?}"))
    }
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
                    Action::UnlockMessage { hash, signal } => {
                        let content = request.progression.messages.as_ref().ok_or(Error::Worker)?;
                        let delivery = content.unlocks.as_ref().ok_or(Error::Worker)?;
                        let rule = delivery
                            .notifications
                            .iter()
                            .find(|n| n.hash == hash)
                            .ok_or(Error::Worker)?;
                        let manager = content.manager(rule.manager).ok_or(Error::Worker)?;
                        let notification_policy = content
                            .unlock_policy()
                            .map_err(|_| Error::Worker)?
                            .ok_or(Error::Worker)?;
                        let mut result = None;
                        for _ in 0..8 {
                            let snapshot = service
                                .repository
                                .snapshot(account)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let pending = notification_policy
                                .pending(snapshot.tables.get(&crate::persistent::key(
                                    nfs_services::messages::unlocks::TABLE,
                                )))
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let Some(row) = pending
                                .into_iter()
                                .find(|r| r.hash == hash && r.signal == signal)
                            else {
                                result = Some(snapshot);
                                break;
                            };
                            let bytes = row
                                .part
                                .map(|item_data_id| {
                                    nfs_world::messages::CarPartAttachment { item_data_id }
                                        .encode()
                                        .to_vec()
                                })
                                .unwrap_or_default();
                            let batch = nfs_services::messages::unlocks::deliver(
                                &snapshot,
                                row,
                                manager.table(),
                                delivery.dynamic_capacity,
                                &bytes,
                            )
                            .map_err(|e| Error::Inventory(e.into()))?
                            .ok_or(Error::Worker)?;
                            match service.repository.apply(account, &batch, now) {
                                Ok(_) | Err(nfs_storage::Error::Conflict) => continue,
                                Err(e) => return Err(Error::Inventory(e.into())),
                            }
                        }
                        result.ok_or(Error::Inventory(nfs_storage::Error::Conflict.into()))?
                    }
                    Action::DynamicMessageRead { ref identity, read } => {
                        let delivery = request
                            .progression
                            .messages
                            .as_ref()
                            .and_then(|c| c.unlocks.as_ref())
                            .ok_or(Error::Worker)?;
                        let mut result = None;
                        for _ in 0..8 {
                            let snapshot = service
                                .repository
                                .snapshot(account)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let Some(batch) = nfs_services::messages::dynamic::set_read_owned(
                                &snapshot,
                                delivery.dynamic_capacity,
                                identity,
                                read,
                            )
                            .map_err(|e| Error::Inventory(e.into()))?
                            else {
                                result = Some(snapshot);
                                break;
                            };
                            match service.repository.apply(account, &batch, now) {
                                Ok(_) | Err(nfs_storage::Error::Conflict) => continue,
                                Err(e) => return Err(Error::Inventory(e.into())),
                            }
                        }
                        result.ok_or(Error::Inventory(nfs_storage::Error::Conflict.into()))?
                    }
                    Action::Choose {
                        objective,
                        definition,
                        ..
                    } => {
                        let claimed = policy
                            .claim_selection(
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
                    Action::Message { hash, manager }
                    | Action::ConditionalMessage { hash, manager, .. }
                    | Action::MessageRead { hash, manager, .. } => {
                        let mut result = None;
                        for _ in 0..8 {
                            let snapshot = service
                                .repository
                                .snapshot(account)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let table = manager.table();
                            let prepared = if let Action::ConditionalMessage { objective, .. } =
                                request.action
                            {
                                let content =
                                    request.progression.messages.as_ref().ok_or(Error::Worker)?;
                                let hashes = content
                                    .conditional_hashes(objective, manager)
                                    .filter(|hashes| hashes.contains(&hash))
                                    .ok_or(Error::Worker)?;
                                let prepared = nfs_services::messages::append_exclusive(
                                    &snapshot, table, hash, &hashes,
                                )
                                .map_err(|e| Error::Inventory(e.into()))?;
                                if prepared.is_some()
                                    && !policy
                                        .objective(&snapshot, objective)
                                        .map_err(|e| Error::Inventory(e.into()))?
                                        .active
                                {
                                    return Err(Error::Worker);
                                }
                                Ok(prepared)
                            } else if let Action::MessageRead { read, .. } = request.action {
                                nfs_services::messages::set_read(&snapshot, table, hash, read)
                            } else {
                                nfs_services::messages::append(&snapshot, table, hash)
                            }
                            .map_err(|e| Error::Inventory(e.into()))?;
                            let Some(batch) = prepared else {
                                result = Some(snapshot);
                                break;
                            };
                            match service.repository.apply(account, &batch, now) {
                                Ok(_) | Err(nfs_storage::Error::Conflict) => continue,
                                Err(e) => return Err(Error::Inventory(e.into())),
                            }
                        }
                        result.ok_or(Error::Inventory(nfs_storage::Error::Conflict.into()))?
                    }
                    Action::Tutorial { key } => {
                        let mut result = None;
                        for _ in 0..8 {
                            let snapshot = service
                                .repository
                                .snapshot(account)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let Some(batch) = nfs_services::tutorials::mark(&snapshot, key)
                                .map_err(|e| Error::Inventory(e.into()))?
                            else {
                                result = Some(snapshot);
                                break;
                            };
                            match service.repository.apply(account, &batch, now) {
                                Ok(_) | Err(nfs_storage::Error::Conflict) => continue,
                                Err(e) => return Err(Error::Inventory(e.into())),
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
                            // Keep the intro summary/ledger consistency gate even
                            // when the requested objective belongs to main story.
                            policy
                                .inspect(&snapshot)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            let state = policy
                                .objective(&snapshot, objective)
                                .map_err(|e| Error::Inventory(e.into()))?;
                            if state.completed {
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
                let (notifications, unannounced) = award_notifications(
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

/// What a server start did with an account whose intro is unfinished. The
/// prologue has no playable checkpoint after its opening, and its first
/// durable completion happens as the first drive starts, so an interrupted
/// session would otherwise come back to the ordinary garage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resume {
    /// Not started, already finished, or nothing safe to change.
    Unchanged,
    /// Interrupted before any car was owned: the intro starts again from its
    /// first objective. Nothing had been granted, so nothing is taken back.
    Restarted,
    /// Interrupted after a car was owned (the starter choice is a claimed
    /// reward and cannot be offered twice): the intro is completed without
    /// rewards, keeping the car and opening the career.
    Finished,
    /// No car and an unfinished intro that this session cannot run.
    Stranded,
}
impl Resume {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Restarted => "restarted",
            Self::Finished => "finished",
            Self::Stranded => "stranded",
        }
    }
}

/// `start` is the prologue's first objective when this session can run it.
fn resume_plan(
    view: &nfs_services::story::View,
    owns_car: bool,
    start: Option<u64>,
) -> (Resume, Option<nfs_services::story::Checkpoint>) {
    use nfs_services::story::Checkpoint;
    let total = view.milestones.len();
    if view.position == total {
        return (Resume::Unchanged, None);
    }
    if owns_car {
        return if view.position == 0 {
            (Resume::Unchanged, None)
        } else {
            (Resume::Finished, Some(Checkpoint::AfterIntro))
        };
    }
    match (start, view.milestones.first()) {
        (Some(start), Some(first)) if start == first.id => {
            if view.position == 0 {
                (Resume::Unchanged, None)
            } else {
                (Resume::Restarted, Some(Checkpoint::Objective(start)))
            }
        }
        _ => (Resume::Stranded, None),
    }
}

/// Blocking. One retry-safe story checkpoint batch at most; the account must
/// not be in use by a session.
pub(in crate::net) fn resume_intro(
    repository: &dyn InventoryRepository,
    account: AccountId,
    policy: &nfs_services::story::Policy<'_>,
    owns_car: impl Fn(&nfs_storage::Snapshot) -> bool,
    start: Option<u64>,
    now: Timestamp,
) -> Result<Resume, nfs_storage::Error> {
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        let view = policy.inspect(&snapshot)?;
        let (resume, target) = resume_plan(&view, owns_car(&snapshot), start);
        let Some(target) = target else {
            return Ok(resume);
        };
        let operation = policy.next_operation(&snapshot)?;
        match policy.jump(&snapshot, snapshot.generation, operation, target)? {
            nfs_services::story::Prepared::Replayed => return Ok(resume),
            nfs_services::story::Prepared::Mutation(batch) => {
                match repository.apply(account, &batch, now) {
                    Ok(_) => return Ok(resume),
                    Err(nfs_storage::Error::Conflict) => continue,
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Err(nfs_storage::Error::Conflict)
}

impl Profile {
    /// Run once at server start, before any listener accepts a session.
    pub async fn resume_intro(
        &self,
        progression: Arc<crate::progression::Content>,
        start: Option<u64>,
        now: Timestamp,
    ) -> Result<Resume, crate::Failure> {
        let service = self.service.clone();
        let account = self.account;
        tokio::task::spawn_blocking(move || {
            // Each step names its failure: the start continues unchanged on
            // any of them, so the log is the only place the cause survives.
            let tables = service.persistent.as_ref().ok_or_else(|| {
                tracing::warn!("intro resume check needs persistent tables; none configured");
                crate::Failure::ProfileConfig
            })?;
            let snapshot = inventory::load(
                service.repository.as_ref(),
                account,
                &service.catalog,
                1,
                now,
            )
            .map_err(|error| {
                tracing::warn!(?error, "intro resume check could not load the inventory");
                crate::Failure::ProfileConfig
            })?;
            tables
                .ensure_loaded(service.repository.as_ref(), account, snapshot, now)
                .map_err(|error| {
                    tracing::warn!(?error, "intro resume check could not load the tables");
                    crate::Failure::ProfileConfig
                })?;
            let bindings = service.catalog.bindings();
            let owns_car = |snapshot: &nfs_storage::Snapshot| {
                snapshot.items.values().any(|item| {
                    bindings.class(&item.definition.0).is_ok_and(|class| {
                        class == nfs_world::items::DefinitionClass::RaceVehicleItemData
                    })
                })
            };
            resume_intro(
                service.repository.as_ref(),
                account,
                &progression.catalog.story_policy(progression.settings),
                owns_car,
                start,
                now,
            )
            .map_err(|error| {
                tracing::warn!(?error, "intro resume check could not settle the story");
                crate::Failure::ProfileConfig
            })
        })
        .await
        .map_err(|_| crate::Failure::Output)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// One claimable starter car, definition `[9; 16]`.
    fn car_catalog() -> Arc<InventoryCatalog> {
        use nfs_world::items::{
            Definition, DefinitionClass, DefinitionFlags, Derived, Layout, OwnershipLevel,
        };
        Arc::new(
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
        )
    }
    fn progression() -> Arc<crate::progression::Content> {
        Arc::new(
            crate::progression::Content::from_json(&crate::progression::tests::combined()).unwrap(),
        )
    }
    #[tokio::test]
    async fn selection_worker_grants_selects_and_completes_before_acknowledgement() {
        let (repository, account, _) = crate::progression::tests::local();
        let repository = Arc::new(repository);
        let catalog = car_catalog();
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
    async fn a_message_is_stored_once_and_its_read_is_durable() {
        let repository = Arc::new(nfs_storage::MemoryRepository::default());
        let account = nfs_storage::AccountId::from_owned_config([43; 16]).unwrap();
        let catalog = Arc::new(InventoryCatalog::new([], vec![]).unwrap());
        // The installed StaticMessage columns (layer9 #77: Asset, Read).
        let mut tables = crate::persistent::tests::content();
        let message = tables["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == "StaticMessage")
            .unwrap();
        message["columns"] = serde_json::json!([
            {"name":"Asset","key":crate::persistent::key("Asset"),"value_type":"int","default":"0"},
            {"name":"Read","key":crate::persistent::key("Read"),"value_type":"int","default":"0"}
        ]);
        let tables = Arc::new(crate::persistent::Catalog::from_json(&tables).unwrap());
        repository.open(account).unwrap();
        repository
            .apply(
                account,
                &nfs_storage::Batch {
                    id: 1,
                    expected_generation: 0,
                    ops: vec![nfs_storage::Op::SetGarage(Default::default())],
                },
                Timestamp(1),
            )
            .unwrap();
        let (before, _) = tables
            .ensure_loaded(
                repository.as_ref(),
                account,
                repository.snapshot(account).unwrap(),
                Timestamp(2),
            )
            .unwrap();
        let service = Service::new(repository.clone(), catalog).with_persistent(tables);
        let profile = service.bind(account);
        let request = Request {
            participant: 19,
            player_index: 4,
            progression: progression(),
            action: Action::Message {
                hash: 1_422_063_331,
                manager: crate::messages::Manager::Normal,
            },
        };
        let rows = |loaded: &crate::persistent::Loaded| {
            nfs_services::messages::rows(loaded.tables(), nfs_services::messages::STATIC).unwrap()
        };
        let committed = profile.story(&request, Timestamp(9)).await.unwrap();
        assert!(committed.loaded.generation() > before.generation);
        assert_eq!(
            rows(&committed.loaded),
            [nfs_services::messages::Row {
                index: 0,
                hash: 1_422_063_331,
                read: false
            }]
        );
        let saved = repository.snapshot(account).unwrap();
        let repeated = profile.story(&request, Timestamp(10)).await.unwrap();
        assert_eq!(repository.snapshot(account).unwrap(), saved);
        assert_eq!(rows(&repeated.loaded).len(), 1);
        let read = Request {
            action: Action::MessageRead {
                hash: 1_422_063_331,
                manager: crate::messages::Manager::Normal,
                read: true,
            },
            ..request.clone()
        };
        let marked = profile.story(&read, Timestamp(11)).await.unwrap();
        assert!(rows(&marked.loaded)[0].read);
        let saved = repository.snapshot(account).unwrap();
        profile.story(&read, Timestamp(12)).await.unwrap();
        assert_eq!(repository.snapshot(account).unwrap(), saved);
        let unread = Request {
            action: Action::MessageRead {
                hash: 1_422_063_331,
                manager: crate::messages::Manager::Normal,
                read: false,
            },
            ..request.clone()
        };
        let changed = profile.story(&unread, Timestamp(13)).await.unwrap();
        assert!(!rows(&changed.loaded)[0].read);
        let saved = repository.snapshot(account).unwrap();
        // A rebound worker reads durable state; duplicates and an unknown row
        // on the other manager cannot mark this message or invent a new one.
        let rebound = service.bind(account);
        rebound.story(&unread, Timestamp(14)).await.unwrap();
        let other = Request {
            action: Action::MessageRead {
                hash: 1_422_063_331,
                manager: crate::messages::Manager::Underground,
                read: true,
            },
            ..request
        };
        rebound.story(&other, Timestamp(15)).await.unwrap();
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
        // A later objective is its own durable operation. Retrying the earlier
        // result after advancing must neither rewind progress nor pay twice.
        let next = Request {
            action: Action::Complete { objective: 20 },
            ..request.clone()
        };
        let later = profile.story(&next, Timestamp(11)).await.unwrap();
        assert!(later.loaded.generation() > committed.loaded.generation());
        let saved = repository.snapshot(account).unwrap();
        let rebound = service.bind(account);
        rebound.story(&request, Timestamp(12)).await.unwrap();
        rebound.story(&next, Timestamp(13)).await.unwrap();
        assert_eq!(repository.snapshot(account).unwrap(), saved);
    }

    #[tokio::test]
    async fn main_completion_returns_its_committed_snapshot_and_retries_across_branches() {
        let (repository, account, _) = crate::progression::tests::local();
        let repository = Arc::new(repository);
        let service = Service::new(
            repository.clone(),
            Arc::new(InventoryCatalog::new([], vec![]).unwrap()),
        )
        .with_persistent(Arc::new(crate::progression::tests::tables()));
        let profile = service.bind(account);
        let mut request = Request {
            participant: 19,
            player_index: 4,
            progression: progression(),
            action: Action::Complete { objective: 30 },
        };
        // A main objective with an unmet prerequisite cannot write anything.
        let before = repository.snapshot(account).unwrap();
        assert!(profile.story(&request, Timestamp(8)).await.is_err());
        assert_eq!(repository.snapshot(account).unwrap(), before);
        for objective in [10, 20] {
            request.action = Action::Complete { objective };
            profile.story(&request, Timestamp(9)).await.unwrap();
        }
        let before = repository.snapshot(account).unwrap();
        request.action = Action::Complete { objective: 30 };
        let committed = profile.story(&request, Timestamp(10)).await.unwrap();
        let saved = repository.snapshot(account).unwrap();
        assert_eq!(committed.loaded.generation(), saved.generation);
        assert_eq!(saved.generation, before.generation + 1);
        let policy = request
            .progression
            .catalog
            .story_policy(request.progression.settings);
        let main = policy.objective(&saved, 30).unwrap();
        assert!(main.completed && main.played && !main.active);
        let restored = request
            .progression
            .catalog
            .restore(&committed.loaded, request.progression.settings)
            .unwrap();
        assert!(
            restored
                .branch(crate::progression::Branch::Main)
                .objectives()[&30]
                .completed
        );
        // Rebinding and late intro retries must not write again or rewind main.
        let rebound = service.bind(account);
        for objective in [30, 10, 20, 30] {
            request.action = Action::Complete { objective };
            let retry = rebound.story(&request, Timestamp(11)).await.unwrap();
            assert_eq!(retry.loaded.generation(), saved.generation);
            assert_eq!(repository.snapshot(account).unwrap(), saved);
        }
        request.action = Action::Complete { objective: 999 };
        assert!(rebound.story(&request, Timestamp(12)).await.is_err());
        assert_eq!(repository.snapshot(account).unwrap(), saved);
    }

    fn view(position: usize) -> nfs_services::story::View {
        nfs_services::story::View {
            generation: 3,
            position,
            milestones: [10, 20]
                .into_iter()
                .enumerate()
                .map(|(index, id)| nfs_services::story::Milestone {
                    id,
                    active: index == position,
                    completed: index < position,
                    played: false,
                    edited: false,
                    existing: false,
                    reward_claimed: false,
                })
                .collect(),
            stats_consistent: true,
        }
    }
    #[test]
    fn resume_plan_restarts_without_a_car_and_finishes_with_one() {
        use nfs_services::story::Checkpoint;
        let cases = [
            // (position, owns car, prologue start) -> decision
            (0, false, Some(10), Resume::Unchanged, None),
            (0, true, Some(10), Resume::Unchanged, None),
            (0, true, None, Resume::Unchanged, None),
            (0, false, None, Resume::Stranded, None),
            (
                1,
                false,
                Some(10),
                Resume::Restarted,
                Some(Checkpoint::Objective(10)),
            ),
            (1, false, None, Resume::Stranded, None),
            (1, false, Some(20), Resume::Stranded, None),
            (
                1,
                true,
                Some(10),
                Resume::Finished,
                Some(Checkpoint::AfterIntro),
            ),
            (
                1,
                true,
                None,
                Resume::Finished,
                Some(Checkpoint::AfterIntro),
            ),
            (2, false, None, Resume::Unchanged, None),
            (2, true, Some(10), Resume::Unchanged, None),
        ];
        for (position, car, start, resume, target) in cases {
            assert_eq!(
                resume_plan(&view(position), car, start),
                (resume, target),
                "position {position} car {car} start {start:?}"
            );
        }
    }
    #[test]
    fn interrupted_intro_without_a_car_restarts_once_and_keeps_other_state() {
        let (repository, account, fresh) = crate::progression::tests::local();
        let progression = progression();
        let policy = progression.catalog.story_policy(progression.settings);
        let no_car = |_: &nfs_storage::Snapshot| false;
        // A fresh intro and a missing prologue: nothing is written.
        for start in [Some(10), None] {
            let expected = if start.is_some() {
                Resume::Unchanged
            } else {
                Resume::Stranded
            };
            assert_eq!(
                resume_intro(&repository, account, &policy, no_car, start, Timestamp(2)),
                Ok(expected)
            );
            assert_eq!(repository.snapshot(account).unwrap(), fresh);
        }
        // The first drive started (first objective durable), then the session ended.
        let nfs_services::story::Prepared::Mutation(batch) = policy
            .complete(
                &fresh,
                fresh.generation,
                policy.next_operation(&fresh).unwrap(),
                10,
                vec![],
            )
            .unwrap()
        else {
            panic!()
        };
        repository.apply(account, &batch, Timestamp(3)).unwrap();
        let interrupted = repository.snapshot(account).unwrap();
        assert_eq!(policy.inspect(&interrupted).unwrap().position, 1);
        assert_eq!(
            resume_intro(
                &repository,
                account,
                &policy,
                no_car,
                Some(10),
                Timestamp(4)
            ),
            Ok(Resume::Restarted)
        );
        let restarted = repository.snapshot(account).unwrap();
        let view = policy.inspect(&restarted).unwrap();
        assert_eq!(view.position, 0);
        assert!(view.milestones[0].active && !view.milestones[0].completed);
        assert_eq!(restarted.items, interrupted.items);
        assert_eq!(restarted.garage, interrupted.garage);
        let tables = crate::progression::tests::tables();
        let restored = progression
            .catalog
            .restore(&tables.view(&restarted).unwrap(), progression.settings)
            .unwrap();
        assert!(
            restored
                .branch(crate::progression::Branch::Intro)
                .objectives()[&10]
                == crate::progression::Objective {
                    active: true,
                    completed: false
                }
        );
        // The next start finds an intro that has not begun.
        assert_eq!(
            resume_intro(
                &repository,
                account,
                &policy,
                no_car,
                Some(10),
                Timestamp(5)
            ),
            Ok(Resume::Unchanged)
        );
        assert_eq!(repository.snapshot(account).unwrap(), restarted);
        // Without the prologue the restarted account is reported, not changed.
        assert_eq!(
            resume_intro(&repository, account, &policy, no_car, None, Timestamp(6)),
            Ok(Resume::Stranded)
        );
        assert_eq!(repository.snapshot(account).unwrap(), restarted);
    }
    #[tokio::test]
    async fn interrupted_intro_after_the_starter_car_finishes_without_rewards() {
        let (repository, account, _) = crate::progression::tests::local();
        finishes_without_rewards(Arc::new(repository), account).await;
    }

    /// E1247: a restart on the real E1192 save failed in this jump with a
    /// storage error the memory repository never raises. Same flow, SQLite.
    #[tokio::test]
    async fn interrupted_intro_after_the_starter_car_finishes_on_a_sqlite_save() {
        let root = std::env::temp_dir().join(format!(
            "afterhours-resume-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (repository, account, _) = crate::progression::tests::local_sqlite(&root);
        finishes_without_rewards(Arc::new(repository), account).await;
        // A fresh repository on the same directory sees the committed result.
        let reopened = nfs_storage::SqliteRepository::open_owned_directory(&root).unwrap();
        let progression = progression();
        let policy = progression.catalog.story_policy(progression.settings);
        let view = policy
            .inspect(&reopened.snapshot(account).unwrap())
            .unwrap();
        assert_eq!(view.position, view.milestones.len());
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    async fn finishes_without_rewards<R: InventoryRepository + 'static>(
        repository: Arc<R>,
        account: AccountId,
    ) {
        let service = Service::new(repository.clone(), car_catalog())
            .with_persistent(Arc::new(crate::progression::tests::tables()));
        let profile = service.bind(account);
        let progression = progression();
        let chosen = Request {
            participant: 19,
            player_index: 4,
            progression: progression.clone(),
            action: Action::Choose {
                sequence: 50,
                node: 7,
                objective: 10,
                definition: [9; 16],
            },
        };
        let committed = profile.story(&chosen, Timestamp(8)).await.unwrap();
        let car = committed.reward.unwrap().item;
        let before = repository.snapshot(account).unwrap();
        assert_eq!(
            profile
                .resume_intro(progression.clone(), Some(10), Timestamp(9))
                .await,
            Ok(Resume::Finished)
        );
        let finished = repository.snapshot(account).unwrap();
        let policy = progression.catalog.story_policy(progression.settings);
        let view = policy.inspect(&finished).unwrap();
        assert_eq!(view.position, view.milestones.len());
        assert_eq!(finished.items, before.items);
        assert!(finished.items.contains_key(&car));
        assert_eq!(finished.garage, before.garage);
        for start in [Some(10), None] {
            assert_eq!(
                profile
                    .resume_intro(progression.clone(), start, Timestamp(10))
                    .await,
                Ok(Resume::Unchanged)
            );
        }
        assert_eq!(repository.snapshot(account).unwrap(), finished);
    }
}
