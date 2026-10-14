// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::players::{EquipmentRequest, PlayerListener};
use crate::inventory;
use nfs_storage::{AccountId, InventoryRepository, Timestamp};
use nfs_world::{
    files::Completed,
    items::InventoryCatalog,
    session::{Event, Host},
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet, time::timeout};
use tracing::{info, warn};

const WORKERS: usize = 4;
const JOB_TIMEOUT: Duration = Duration::from_secs(5);
const EXCHANGE_MS: u64 = 60_000;

#[derive(Clone)]
pub struct Service {
    repository: Arc<dyn InventoryRepository>,
    catalog: Arc<InventoryCatalog>,
    persistent: Option<Arc<crate::persistent::Catalog>>,
    offers: Option<Arc<nfs_services::purchase::Offers>>,
    workers: Arc<Semaphore>,
}
impl Service {
    pub fn new(repository: Arc<dyn InventoryRepository>, catalog: Arc<InventoryCatalog>) -> Self {
        Self {
            repository,
            catalog,
            persistent: None,
            offers: None,
            workers: Arc::new(Semaphore::new(WORKERS)),
        }
    }
    pub fn with_persistent(mut self, catalog: Arc<crate::persistent::Catalog>) -> Self {
        self.persistent = Some(catalog);
        self
    }
    pub fn with_purchase_offers(mut self, offers: nfs_services::purchase::Offers) -> Self {
        self.offers = Some(Arc::new(offers));
        self
    }
    pub fn bind(&self, account: AccountId) -> Profile {
        Profile {
            service: self.clone(),
            account,
        }
    }
}

#[derive(Clone)]
pub struct Profile {
    service: Service,
    account: AccountId,
}
impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InventoryProfile").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Error {
    Inventory(inventory::Error),
    Worker,
    Deadline,
    Purchase(nfs_services::purchase::Error),
    Equipment(nfs_services::equipment::Error),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplyKind {
    Load,
    Purchase,
    Rejected,
}
#[derive(Debug)]
struct LoadedReply {
    reply: inventory::Reply,
    items: Arc<nfs_world::items::Collection>,
    kind: ReplyKind,
}
struct Equipped {
    loaded: Arc<crate::persistent::Loaded>,
    items: Arc<nfs_world::items::Collection>,
    notification: Vec<u8>,
}

fn supported_request(bytes: &[u8]) -> Result<bool, nfs_world::items::Error> {
    use nfs_world::items;
    match bytes.get(4) {
        Some(&items::PURCHASE_ITEMS) => items::PurchaseRequest::decode(bytes).map(|_| true),
        _ => items::Request::decode(bytes).map(|r| r.operation == items::LOAD_INVENTORY),
    }
}
impl Profile {
    async fn equip(&self, request: &EquipmentRequest, now: Timestamp) -> Result<Equipped, Error> {
        let service = self.service.clone();
        let account = self.account;
        let request = request.clone();
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Error::Worker)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let offers = service.offers.as_deref().ok_or(Error::Worker)?;
                let tables = service.persistent.as_ref().ok_or(Error::Worker)?;
                let edit = if let Some(item) = request.item {
                    let edit = nfs_services::equipment::prepare(
                        account,
                        request.generation,
                        &request.items,
                        request.car,
                        item,
                        &service.catalog,
                        offers,
                    )
                    .map_err(Error::Equipment)?;
                    if let Some(stats) = request.dyno {
                        edit.with_dyno(stats).map_err(Error::Equipment)?
                    } else {
                        edit
                    }
                } else {
                    nfs_services::equipment::prepare_dyno(
                        account,
                        request.generation,
                        &request.items,
                        request.car,
                        request.dyno.ok_or(Error::Worker)?,
                        &service.catalog,
                    )
                    .map_err(Error::Equipment)?
                };
                // Validate the exact outbound update before the durable write.
                let notification = nfs_world::items::UpdateNotification {
                    player_index: request.player_index,
                    items: edit
                        .preview(&request.items)
                        .map_err(Error::Equipment)?
                        .for_roots(vec![request.car])
                        .map_err(|e| Error::Inventory(e.into()))?,
                }
                .encode(service.catalog.bindings())
                .map_err(|e| Error::Inventory(e.into()))?;
                let snapshot = nfs_services::equipment::commit(
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    &edit,
                    now,
                )
                .map_err(Error::Equipment)?;
                let items = inventory::collection(&snapshot, service.catalog.bindings())
                    .map_err(Error::Inventory)?;
                let loaded = tables
                    .view(&snapshot)
                    .map_err(|e| Error::Inventory(e.into()))?;
                Ok(Equipped {
                    loaded: Arc::new(loaded),
                    items: Arc::new(items),
                    notification,
                })
            })
            .await
            .map_err(|_| Error::Worker)?
        })
        .await
        .map_err(|_| Error::Deadline)?
    }

    pub fn bindings(&self) -> &nfs_world::items::Catalog {
        self.service.catalog.bindings()
    }
    pub async fn current_tables(
        &self,
        now: Timestamp,
    ) -> Result<crate::persistent::Loaded, crate::Failure> {
        self.current_account(now, false)
            .await
            .map(|(_, loaded)| loaded)
    }
    pub async fn current_awards(
        &self,
        now: Timestamp,
        thresholds: &nfs_services::reputation::Thresholds,
    ) -> Result<nfs_services::awards::Current, crate::Failure> {
        let (snapshot, loaded) = self.current_account(now, true).await?;
        nfs_services::awards::Current::from_snapshot(&snapshot, loaded, thresholds)
            .map_err(|_| crate::Failure::ProfileConfig)
    }
    pub async fn current_challenges(
        &self,
        now: Timestamp,
    ) -> Result<nfs_services::challenges::Current, crate::Failure> {
        let service = self.service.clone();
        let account = self.account;
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| crate::Failure::Io)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let snapshot = inventory::load(
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    1,
                    now,
                )
                .map_err(|_| crate::Failure::ProfileConfig)?;
                let snapshot = nfs_services::challenges::ensure_empty(
                    service.repository.as_ref(),
                    account,
                    snapshot,
                    now,
                )
                .map_err(|_| crate::Failure::ProfileConfig)?;
                nfs_services::challenges::Current::from_snapshot(&snapshot)
                    .map_err(|_| crate::Failure::ProfileConfig)
            })
            .await
            .map_err(|_| crate::Failure::Io)?
        })
        .await
        .map_err(|_| crate::Failure::Io)?
    }
    async fn current_account(
        &self,
        now: Timestamp,
        awards: bool,
    ) -> Result<(nfs_storage::Snapshot, crate::persistent::Loaded), crate::Failure> {
        let service = self.service.clone();
        let account = self.account;
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| crate::Failure::Io)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let tables = service
                    .persistent
                    .as_ref()
                    .ok_or(crate::Failure::ProfileConfig)?;
                let snapshot = inventory::load(
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    1,
                    now,
                )
                .map_err(|_| crate::Failure::ProfileConfig)?;
                let snapshot = if awards {
                    nfs_services::awards::ensure_empty(
                        service.repository.as_ref(),
                        account,
                        snapshot,
                        now,
                    )
                    .map_err(|_| crate::Failure::ProfileConfig)?
                } else {
                    snapshot
                };
                tables
                    .ensure_loaded(service.repository.as_ref(), account, snapshot, now)
                    .map_err(|_| crate::Failure::ProfileConfig)
            })
            .await
            .map_err(|_| crate::Failure::Io)?
        })
        .await
        .map_err(|_| crate::Failure::Io)?
    }
    async fn answer(&self, request: Vec<u8>, now: Timestamp) -> Result<Option<LoadedReply>, Error> {
        if !supported_request(&request).map_err(|e| Error::Inventory(e.into()))? {
            return Ok(None);
        }
        let purchase = (request[4] == nfs_world::items::PURCHASE_ITEMS)
            .then(|| nfs_world::items::PurchaseRequest::decode(&request))
            .transpose()
            .map_err(|e| Error::Inventory(e.into()))?;
        if purchase.is_some() && self.service.offers.is_none() {
            return Ok(None);
        }
        let service = self.service.clone();
        let account = self.account;
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .acquire_owned()
                .await
                .map_err(|_| Error::Worker)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                if let Some(request) = purchase {
                    let offers = service.offers.as_deref().ok_or(Error::Worker)?;
                    let (snapshot, purchased, kind) = match nfs_services::purchase::buy(
                        service.repository.as_ref(),
                        account,
                        &service.catalog,
                        offers,
                        &request,
                        now,
                    ) {
                        Ok(result) => (result.snapshot, Some(result.items), ReplyKind::Purchase),
                        Err(nfs_services::purchase::Error::Rejected(reason)) => {
                            info!(?reason, "owned purchase rejected without mutation");
                            (
                                service
                                    .repository
                                    .snapshot(account)
                                    .map_err(|e| Error::Inventory(e.into()))?,
                                None,
                                ReplyKind::Rejected,
                            )
                        }
                        Err(error) => return Err(Error::Purchase(error)),
                    };
                    let items = inventory::collection(&snapshot, service.catalog.bindings())
                        .map_err(Error::Inventory)?;
                    let persistent = service
                        .persistent
                        .as_ref()
                        .map(|tables| {
                            tables
                                .view(&snapshot)
                                .map(Arc::new)
                                .map_err(|e| Error::Inventory(e.into()))
                        })
                        .transpose()?;
                    let bytes = nfs_world::items::PurchaseReply {
                        sequence: request.sequence,
                        purchased,
                    }
                    .encode(service.catalog.bindings())
                    .map_err(|e| Error::Inventory(e.into()))?;
                    return Ok(Some(LoadedReply {
                        reply: inventory::Reply {
                            generation: snapshot.generation,
                            bytes,
                            persistent,
                            garage: snapshot
                                .garage
                                .ok_or(Error::Worker)?
                                .0
                                .map(|id| id.map(nfs_storage::ItemId::get)),
                        },
                        items: Arc::new(items),
                        kind,
                    }));
                }
                let reply = inventory::answer_load_with_tables(
                    &request,
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    1,
                    now,
                    service.persistent.as_deref(),
                )
                .map_err(Error::Inventory)?;
                reply
                    .map(|reply| {
                        let items = nfs_world::items::Envelope::decode(&reply.bytes)
                            .and_then(|e| e.collection(service.catalog.bindings()))
                            .map_err(|e| Error::Inventory(e.into()))?;
                        Ok(LoadedReply {
                            reply,
                            items: Arc::new(items),
                            kind: ReplyKind::Load,
                        })
                    })
                    .transpose()
            })
            .await
            .map_err(|_| Error::Worker)?
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
}
pub(super) struct Exchanges {
    profile: Option<Profile>,
    jobs: JoinSet<(usize, Result<Option<LoadedReply>, Error>)>,
    equipment: JoinSet<(EquipmentRequest, Result<Equipped, Error>)>,
    pending: [Option<Completed>; 2],
    started: [Option<u64>; 2],
    connection: u32,
}
impl Exchanges {
    pub(super) fn new(profile: Option<Profile>, connection: u32) -> Self {
        Self {
            profile,
            jobs: JoinSet::new(),
            equipment: JoinSet::new(),
            pending: [None, None],
            started: [None, None],
            connection,
        }
    }
    pub(super) fn working(&self) -> bool {
        !self.jobs.is_empty() || !self.equipment.is_empty()
    }
    pub(super) fn start(&mut self, listener: &mut PlayerListener, now: u64, utc: Timestamp) {
        if self.working()
            || self.pending.iter().any(Option::is_some)
            || self.started.iter().any(Option::is_some)
        {
            return;
        }
        if let Some(request) = listener.take_equipment() {
            if let Some(profile) = self.profile.clone() {
                self.equipment.spawn(async move {
                    let result = profile.equip(&request, utc).await;
                    (request, result)
                });
            } else {
                warn!(
                    connection = self.connection,
                    "equipment service not configured"
                );
            }
            return;
        }
        for index in 0..2 {
            if self.started[index].is_some() {
                continue;
            }
            let Some(request) = listener.take_file(index) else {
                continue;
            };
            let handler = index + 4;
            if request.name != b"Items" {
                warn!(
                    connection = self.connection,
                    handler,
                    name_bytes = request.name.len(),
                    bytes = request.bytes.len(),
                    "unsupported File name"
                );
                continue;
            }
            let Some(profile) = self.profile.clone() else {
                warn!(
                    connection = self.connection,
                    handler, "Items service not configured"
                );
                continue;
            };
            match supported_request(&request.bytes) {
                Ok(false) => {
                    warn!(
                        connection = self.connection,
                        handler,
                        operation = request.bytes[4],
                        "unsupported Items operation"
                    );
                    continue;
                }
                Err(error) => {
                    warn!(
                        connection = self.connection,
                        handler,
                        ?error,
                        "invalid Items request"
                    );
                    continue;
                }
                _ => {}
            }
            if request.bytes[4] == nfs_world::items::PURCHASE_ITEMS && !listener.purchase_ready() {
                warn!(
                    connection = self.connection,
                    handler, "purchase before owned inventory is ready"
                );
                continue;
            }
            self.started[index] = Some(now);
            self.jobs
                .spawn(async move { (index, profile.answer(request.bytes, utc).await) });
            break;
        }
    }
    pub(super) async fn completed(&mut self, listener: &mut PlayerListener) -> bool {
        // start() serializes account writes across Files and equipment RPCs.
        if !self.equipment.is_empty() {
            let Some(Ok((request, result))) = self.equipment.join_next().await else {
                return false;
            };
            return match result {
                Ok(Equipped {
                    loaded,
                    items,
                    notification,
                }) => {
                    let generation = loaded.generation();
                    match listener.equipped_inventory(&request, loaded, items) {
                        Ok(()) => {
                            self.pending[0] = Some(Completed {
                                name: b"Items".to_vec(),
                                bytes: notification,
                            });
                            info!(
                                connection = self.connection,
                                generation,
                                car = request.car,
                                item = ?request.item,
                                dyno = request.dyno.is_some(),
                                "owned equipment committed"
                            );
                            true
                        }
                        Err(error) => {
                            warn!(
                                connection = self.connection,
                                ?error,
                                "equipment snapshot rejected"
                            );
                            false
                        }
                    }
                }
                Err(error) => {
                    warn!(
                        connection = self.connection,
                        ?error,
                        "equipment commit failed"
                    );
                    false
                }
            };
        }
        let Some(Ok((index, result))) = self.jobs.join_next().await else {
            return false;
        };
        match result {
            Ok(Some(LoadedReply { reply, items, kind })) => {
                let applied = match kind {
                    ReplyKind::Load => listener.inventory_with_items(
                        reply.garage,
                        reply.persistent.clone(),
                        Some(items),
                    ),
                    ReplyKind::Purchase => {
                        listener.purchased_inventory(reply.garage, reply.persistent.clone(), items)
                    }
                    ReplyKind::Rejected => Ok(()),
                };
                if let Err(error) = applied {
                    warn!(
                        connection = self.connection,
                        ?error,
                        "player snapshot rejected"
                    );
                    return false;
                }
                info!(
                    connection = self.connection,
                    handler = index + 4,
                    generation = reply.generation,
                    ?kind,
                    bytes = reply.bytes.len(),
                    persistent_tables = reply.persistent.as_ref().map_or(0, |p| p.tables().len()),
                    persistent_stores = reply.persistent.as_ref().map_or(0, |p| p.stores().len()),
                    "owned Items reply committed"
                );
                self.pending[index] = Some(Completed {
                    name: b"Items".to_vec(),
                    bytes: reply.bytes,
                });
                true
            }
            Ok(None) => {
                warn!(
                    connection = self.connection,
                    handler = index + 4,
                    "unsupported Items operation"
                );
                self.started[index] = None;
                true
            }
            Err(error) => {
                warn!(
                    connection = self.connection,
                    handler = index + 4,
                    ?error,
                    "Items request failed"
                );
                false
            }
        }
    }
    pub(super) fn advance(&mut self, host: &mut Host, events: &[Event], now: u64) -> bool {
        for event in events {
            match event {
                Event::FileComplete { handler } => self.started[usize::from(handler - 4)] = None,
                Event::FileError { .. } => return false,
                _ => {}
            }
        }
        for index in 0..2 {
            if self.started[index].is_some_and(|start| now.saturating_sub(start) >= EXCHANGE_MS) {
                warn!(
                    connection = self.connection,
                    handler = index + 4,
                    "Items exchange deadline"
                );
                return false;
            }
            if let Some(reply) = &self.pending[index] {
                // Unsolicited updates have no incoming request to start the clock.
                self.started[index].get_or_insert(now);
                match host.queue_file(index as u8 + 4, reply, now) {
                    Ok(()) => {
                        self.pending[index] = None;
                    }
                    Err(nfs_world::files::sender::Error::State) => {}
                    Err(error) => {
                        warn!(
                            connection = self.connection,
                            ?error,
                            "Items File queue failed"
                        );
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
pub(crate) mod tests;
