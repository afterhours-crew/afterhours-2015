// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::players::PlayerListener;
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
    workers: Arc<Semaphore>,
}
impl Service {
    pub fn new(repository: Arc<dyn InventoryRepository>, catalog: Arc<InventoryCatalog>) -> Self {
        Self {
            repository,
            catalog,
            persistent: None,
            workers: Arc::new(Semaphore::new(WORKERS)),
        }
    }
    pub fn with_persistent(mut self, catalog: Arc<crate::persistent::Catalog>) -> Self {
        self.persistent = Some(catalog);
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
}
#[derive(Debug)]
struct LoadedReply {
    reply: inventory::Reply,
    items: Arc<nfs_world::items::Collection>,
}
impl Profile {
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
        let decoded =
            nfs_world::items::Request::decode(&request).map_err(|e| Error::Inventory(e.into()))?;
        if decoded.operation != nfs_world::items::LOAD_INVENTORY {
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
    pending: [Option<Completed>; 2],
    started: [Option<u64>; 2],
    connection: u32,
}
impl Exchanges {
    pub(super) fn new(profile: Option<Profile>, connection: u32) -> Self {
        Self {
            profile,
            jobs: JoinSet::new(),
            pending: [None, None],
            started: [None, None],
            connection,
        }
    }
    pub(super) fn working(&self) -> bool {
        !self.jobs.is_empty()
    }
    pub(super) fn start(&mut self, listener: &mut PlayerListener, now: u64, utc: Timestamp) {
        if self.working() {
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
            match nfs_world::items::Request::decode(&request.bytes) {
                Ok(request) if request.operation != nfs_world::items::LOAD_INVENTORY => {
                    warn!(
                        connection = self.connection,
                        handler,
                        operation = request.operation,
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
            self.started[index] = Some(now);
            self.jobs
                .spawn(async move { (index, profile.answer(request.bytes, utc).await) });
            break;
        }
    }
    pub(super) async fn completed(&mut self, listener: &mut PlayerListener) -> bool {
        let Some(Ok((index, result))) = self.jobs.join_next().await else {
            return false;
        };
        match result {
            Ok(Some(LoadedReply { reply, items })) => {
                if let Err(error) = listener.inventory_with_items(
                    reply.garage,
                    reply.persistent.clone(),
                    Some(items),
                ) {
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
