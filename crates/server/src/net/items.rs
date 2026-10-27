// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::players::{
    AwardRequest, Credited, EquipmentRequest, GarageOperation, GarageRequest, PlayerListener,
};
mod garage;
pub(super) mod story;
use crate::inventory;
use nfs_storage::{AccountId, InventoryRepository, Timestamp};
use nfs_world::{
    files::Completed,
    items::InventoryCatalog,
    session::{Event, Host},
};
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet, time::timeout};
use tracing::{info, warn};

const WORKERS: usize = 4;
const JOB_TIMEOUT: Duration = Duration::from_secs(5);
const EXCHANGE_MS: u64 = 60_000;
/// Separate notifications per kind and credit; more changes share them.
const MAX_SEPARATE: usize = 16;

/// Notifications for what a committed credit changed, in the official order
/// (E926): every new record in its own change notification (one per part a
/// level-up unlocked), then every changed root (the category unlock
/// controller) in its own update notification. Also returns the changed
/// roots no notification describes: removals, which a credit never makes,
/// and updates beyond the bound.
fn credit_notifications(
    player_index: u32,
    before: &nfs_world::items::Collection,
    after: &nfs_world::items::Collection,
    catalog: &nfs_world::items::Catalog,
) -> Result<(Vec<Vec<u8>>, usize), nfs_world::items::Error> {
    let added = after.changes_since(before)?.added.len();
    notifications_with(
        player_index,
        before,
        after,
        catalog,
        added.div_ceil(MAX_SEPARATE).max(1),
    )
}

/// Notifications for a story award: every new record in one change
/// notification. The official career start sent the starter car's complete
/// item tree as a single 3,926-byte Items change notification, together with
/// the car's creation and before the selection callback (E1026 497.299 s,
/// E1249); changed roots keep their own update notifications.
fn award_notifications(
    player_index: u32,
    before: &nfs_world::items::Collection,
    after: &nfs_world::items::Collection,
    catalog: &nfs_world::items::Catalog,
) -> Result<(Vec<Vec<u8>>, usize), nfs_world::items::Error> {
    notifications_with(player_index, before, after, catalog, usize::MAX)
}

fn notifications_with(
    player_index: u32,
    before: &nfs_world::items::Collection,
    after: &nfs_world::items::Collection,
    catalog: &nfs_world::items::Catalog,
    per: usize,
) -> Result<(Vec<Vec<u8>>, usize), nfs_world::items::Error> {
    let changes = after.changes_since(before)?;
    let mut notifications = Vec::new();
    for roots in changes.added.chunks(per) {
        notifications.push(
            nfs_world::items::ChangeNotification {
                player_index,
                added: after.for_roots(roots.to_vec())?,
                removed: Default::default(),
            }
            .encode(catalog)?,
        );
    }
    for &root in changes.changed.iter().take(MAX_SEPARATE) {
        notifications.push(
            nfs_world::items::UpdateNotification {
                player_index,
                items: after.for_roots(vec![root])?,
            }
            .encode(catalog)?,
        );
    }
    let unannounced = changes.removed.len() + changes.changed.len().saturating_sub(MAX_SEPARATE);
    Ok((notifications, unannounced))
}

#[derive(Clone)]
pub struct Service {
    repository: Arc<dyn InventoryRepository>,
    catalog: Arc<InventoryCatalog>,
    persistent: Option<Arc<crate::persistent::Catalog>>,
    offers: Option<Arc<nfs_services::purchase::Offers>>,
    car_offers: Option<Arc<nfs_services::garage::Offers>>,
    category_unlocks: Option<Arc<nfs_services::category_unlocks::Policy>>,
    records: Option<Arc<dyn nfs_storage::records::Repository>>,
    workers: Arc<Semaphore>,
}
impl Service {
    pub fn new(repository: Arc<dyn InventoryRepository>, catalog: Arc<InventoryCatalog>) -> Self {
        Self {
            repository,
            catalog,
            persistent: None,
            offers: None,
            car_offers: None,
            category_unlocks: None,
            records: None,
            workers: Arc::new(Semaphore::new(WORKERS)),
        }
    }
    pub fn with_persistent(mut self, catalog: Arc<crate::persistent::Catalog>) -> Self {
        self.persistent = Some(catalog);
        self
    }
    pub fn with_records(mut self, records: Arc<dyn nfs_storage::records::Repository>) -> Self {
        self.records = Some(records);
        self
    }
    pub fn with_purchase_offers(mut self, offers: nfs_services::purchase::Offers) -> Self {
        self.offers = Some(Arc::new(offers));
        self
    }
    pub fn with_car_offers(mut self, offers: nfs_services::garage::Offers) -> Self {
        self.car_offers = Some(Arc::new(offers));
        self
    }
    pub fn with_category_unlocks(mut self, policy: nfs_services::category_unlocks::Policy) -> Self {
        self.category_unlocks = Some(Arc::new(policy));
        self
    }
    pub fn bind(&self, account: AccountId) -> Profile {
        Profile {
            service: self.clone(),
            account,
            garage_session: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct Profile {
    service: Service,
    account: AccountId,
    garage_session: Arc<std::sync::Mutex<Option<u64>>>,
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
    Garage(nfs_services::garage::Error),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplyKind {
    Load,
    Purchase,
    Garage,
    Rejected,
}
#[derive(Debug)]
struct LoadedReply {
    reply: inventory::Reply,
    items: Arc<nfs_world::items::Collection>,
    kind: ReplyKind,
    previous: Option<crate::vehicle_content::population::Inventory>,
}
struct Equipped {
    loaded: Arc<crate::persistent::Loaded>,
    items: Arc<nfs_world::items::Collection>,
    notification: Vec<u8>,
    edit: nfs_services::equipment::Edit,
    presentation: crate::vehicle_content::EquipmentPresentation,
}

fn supported_request(bytes: &[u8]) -> Result<bool, nfs_world::items::Error> {
    use nfs_world::items;
    match bytes.get(4) {
        Some(&items::PURCHASE_ITEMS) => items::PurchaseRequest::decode(bytes).map(|_| true),
        Some(&items::SELL_ITEMS) => items::SellRequest::decode(bytes).map(|_| true),
        _ => items::Request::decode(bytes).map(|r| r.operation == items::LOAD_INVENTORY),
    }
}
impl Profile {
    pub(super) fn bind_records(
        &self,
        records: crate::auxiliary::LocalRecords,
    ) -> crate::auxiliary::LocalRecords {
        match &self.service.records {
            Some(repository) => records.with_storage(repository.clone(), self.account),
            None => records,
        }
    }
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
                let tables = service.persistent.as_ref().ok_or(Error::Worker)?;
                let mut edit = nfs_services::equipment::prepare_vehicle(
                    account,
                    request.generation,
                    &request.items,
                    request.car,
                    &service.catalog,
                )
                .map_err(Error::Equipment)?;
                for operation in &request.part_edits {
                    use nfs_services::item_builder::Operation;
                    edit = match *operation {
                        Operation::StockPerformance { .. } => {
                            edit.with_stock_performance(&request.items, &service.catalog)
                        }
                        Operation::StockVisual { keep_wheels, .. } => {
                            edit.with_stock_visual(&request.items, &service.catalog, keep_wheels)
                        }
                        Operation::StockPaint => edit.with_stock_paint(&service.catalog),
                        Operation::LicensePlate { plate } => edit.with_license_plate(plate),
                        Operation::WindowTint { tint } => edit.with_window_tint(tint),
                        Operation::Livery {
                            item,
                            data_id,
                            preset,
                        } => edit.with_livery(
                            &request.items,
                            &service.catalog,
                            item,
                            data_id,
                            preset,
                        ),
                        Operation::Equip { item, .. } => edit.with_part(
                            &request.items,
                            item,
                            &service.catalog,
                            service.offers.as_deref().ok_or(Error::Worker)?,
                        ),
                        Operation::RemovePart { item, .. } => edit.without_part(
                            &request.items,
                            item,
                            &service.catalog,
                            service.offers.as_deref().ok_or(Error::Worker)?,
                        ),
                        Operation::Paint { paint } => edit.with_paint(
                            &request.items,
                            &service.catalog,
                            service.offers.as_deref().ok_or(Error::Worker)?,
                            paint,
                        ),
                        Operation::RimSize { item, size } => {
                            request
                                .content
                                .vehicles
                                .validate_rim_size(
                                    &edit.preview(&request.items).map_err(Error::Equipment)?,
                                    request.car,
                                    item,
                                    size,
                                )
                                .map_err(|_| Error::Worker)?;
                            edit.with_rim_size(&request.items, &service.catalog, item, size)
                        }
                        _ => return Err(Error::Worker),
                    }
                    .map_err(Error::Equipment)?;
                }
                let transmission = if request.handling.transmission().is_some() {
                    Some(
                        request
                            .content
                            .vehicles
                            .transmission_values(
                                &edit.preview(&request.items).map_err(Error::Equipment)?,
                                request.car,
                            )
                            .map_err(|_| Error::Worker)?,
                    )
                } else {
                    None
                };
                let edit = edit
                    .with_handling(
                        &request.items,
                        &service.catalog,
                        request.handling,
                        transmission,
                    )
                    .map_err(Error::Equipment)?;
                let edit = if let Some(stats) = request.dyno {
                    edit.with_dyno(stats).map_err(Error::Equipment)?
                } else {
                    edit
                };
                // Validate the exact outbound update before the durable write.
                let preview = edit.preview(&request.items).map_err(Error::Equipment)?;
                let presentation = request
                    .content
                    .vehicles
                    .equipment(
                        &preview,
                        request.car,
                        request.owner.clone(),
                        &request.registrations,
                    )
                    .map_err(|_| Error::Worker)?;
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
                    edit,
                    presentation,
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
    /// The item definitions (stock vehicle figures for free-roam scoring).
    pub fn item_catalog(&self) -> Arc<InventoryCatalog> {
        self.service.catalog.clone()
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
    pub async fn current_speedwall(
        &self,
        now: Timestamp,
    ) -> Result<nfs_services::speedwall::Current, crate::Failure> {
        let (snapshot, _) = self.current_account(now, false).await?;
        nfs_services::speedwall::Current::from_snapshot(self.account, &snapshot)
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
    /// Apply one banked driving credit as a single retry-safe batch: the
    /// score columns, the derived level and every newly eligible category
    /// and item grant commit together. The refreshed account view and item
    /// collection come back for the listener to adopt.
    async fn credit(&self, request: &AwardRequest, now: Timestamp) -> Result<Credited, Error> {
        let service = self.service.clone();
        let account = self.account;
        let credit = request.credit;
        let batch_id = request.batch_id;
        let thresholds = request.thresholds.clone();
        let player_index = request.player_index;
        let event = request.event.clone();
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .acquire_owned()
                .await
                .map_err(|_| Error::Worker)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let tables = service.persistent.as_deref().ok_or(Error::Worker)?;
                let snapshot = inventory::load(
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    1,
                    now,
                )
                .map_err(Error::Inventory)?;
                let policy = nfs_services::reputation::earned::Policy {
                    tables,
                    inventory: &service.catalog,
                    thresholds: &thresholds,
                    categories: service.category_unlocks.as_deref(),
                    items: service
                        .offers
                        .as_ref()
                        .and_then(|offers| offers.item_unlocks()),
                };
                let before = inventory::collection(&snapshot, service.catalog.bindings())
                    .map_err(Error::Inventory)?;
                let batch = match &event {
                    // An event completion's rewards commit as one transaction.
                    Some(event) => {
                        let snapshot = nfs_services::awards::ensure_empty(
                            service.repository.as_ref(),
                            account,
                            snapshot,
                            now,
                        )
                        .map_err(|_| Error::Inventory(nfs_storage::Error::Config.into()))?;
                        nfs_services::event_rewards::prepare(&policy, &snapshot, event)
                            .map_err(Error::Inventory)?
                    }
                    None => policy
                        .prepare(&snapshot, credit, batch_id)
                        .map_err(Error::Inventory)?,
                };
                let applied = service
                    .repository
                    .apply(account, &batch, now)
                    .map_err(|e| Error::Inventory(e.into()))?;
                let snapshot = service
                    .repository
                    .snapshot(account)
                    .map_err(|e| Error::Inventory(e.into()))?;
                let loaded = tables
                    .view(&snapshot)
                    .map_err(|e| Error::Inventory(e.into()))?;
                let items = inventory::collection(&snapshot, service.catalog.bindings())
                    .map_err(Error::Inventory)?;
                // The commit stands whatever happens here; an unannounced
                // change still reaches the client at its next load.
                let (notifications, unannounced) = player_index
                    .and_then(|index| {
                        credit_notifications(index, &before, &items, service.catalog.bindings())
                            .ok()
                    })
                    .unwrap_or_else(|| (Vec::new(), usize::from(before != items)));
                Ok(Credited {
                    loaded: Arc::new(loaded),
                    items: Arc::new(items),
                    replayed: applied.replayed,
                    notifications,
                    unannounced,
                })
            })
            .await
            .map_err(|_| Error::Worker)?
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    async fn answer(&self, request: Vec<u8>, now: Timestamp) -> Result<Option<LoadedReply>, Error> {
        if !supported_request(&request).map_err(|e| Error::Inventory(e.into()))? {
            return Ok(None);
        }
        let purchase = (request[4] == nfs_world::items::PURCHASE_ITEMS)
            .then(|| nfs_world::items::PurchaseRequest::decode(&request))
            .transpose()
            .map_err(|e| Error::Inventory(e.into()))?;
        if request[4] == nfs_world::items::SELL_ITEMS
            || purchase.as_ref().is_some_and(|p| p.owner == 0)
        {
            return self.answer_garage(request, now).await;
        }
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
                        previous: None,
                    }));
                }
                let reply = inventory::answer_load_with_unlocks(
                    &request,
                    service.repository.as_ref(),
                    account,
                    &service.catalog,
                    1,
                    now,
                    service.persistent.as_deref(),
                    service.category_unlocks.as_deref(),
                    service
                        .offers
                        .as_ref()
                        .and_then(|offers| offers.item_unlocks()),
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
                            previous: None,
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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum UnsupportedRoute {
    File { handler: usize, name_bytes: usize },
    Items { handler: usize, operation: u8 },
}

pub(super) struct Exchanges {
    profile: Option<Profile>,
    challenges: JoinSet<Result<nfs_services::challenges::Current, crate::Failure>>,
    jobs: JoinSet<(usize, u8, Result<Option<LoadedReply>, Error>)>,
    equipment: JoinSet<(EquipmentRequest, Result<Equipped, Error>)>,
    garage: JoinSet<(GarageRequest, Result<garage::Updated, Error>)>,
    awards: JoinSet<(AwardRequest, Result<Credited, Error>)>,
    story: JoinSet<(story::Request, Result<story::Committed, Error>)>,
    story_delivery: Option<(story::Request, story::Committed)>,
    pending: [Option<Completed>; 2],
    started: [Option<u64>; 2],
    /// Unsolicited notifications awaiting the request handler, in order.
    notifications: VecDeque<Completed>,
    /// Display objects may refer to newly purchased item IDs. Publish them only
    /// after the client has acknowledged delivery of the Items result.
    garage_delivery: Option<garage::Updated>,
    failed: bool,
    connection: u32,
    unsupported: crate::diagnostics::Unsupported<UnsupportedRoute>,
}
impl Exchanges {
    pub(super) fn new(profile: Option<Profile>, connection: u32) -> Self {
        Self {
            profile,
            challenges: JoinSet::new(),
            jobs: JoinSet::new(),
            equipment: JoinSet::new(),
            garage: JoinSet::new(),
            awards: JoinSet::new(),
            story: JoinSet::new(),
            story_delivery: None,
            pending: [None, None],
            started: [None, None],
            notifications: VecDeque::new(),
            garage_delivery: None,
            failed: false,
            connection,
            unsupported: crate::diagnostics::Unsupported::new("world-file", connection),
        }
    }
    pub(super) fn working(&self) -> bool {
        !self.challenges.is_empty()
            || !self.jobs.is_empty()
            || !self.equipment.is_empty()
            || !self.awards.is_empty()
            || !self.garage.is_empty()
            || !self.story.is_empty()
    }
    pub(super) fn start(&mut self, listener: &mut PlayerListener, now: u64, utc: Timestamp) {
        if self.pending.iter().all(Option::is_none)
            && self.started.iter().all(Option::is_none)
            && self.notifications.is_empty()
            && let Some((request, committed)) = &self.story_delivery
        {
            match listener.story_committed(request, committed.clone()) {
                Ok(()) => self.story_delivery = None,
                Err(nfs_world::replication::Error::Bound) => return,
                Err(error) => {
                    warn!(
                        connection = self.connection,
                        ?error,
                        "committed story snapshot rejected"
                    );
                    self.failed = true;
                }
            }
        }
        if self.pending.iter().all(Option::is_none)
            && self.started.iter().all(Option::is_none)
            && let Some(updated) = self.garage_delivery.take()
            && let Err(error) = listener.garage_inventory(
                &updated.previous,
                updated.loaded.garage(),
                updated.loaded,
                updated.items,
            )
        {
            warn!(
                connection = self.connection,
                ?error,
                "delivered garage snapshot rejected"
            );
            self.failed = true;
        }
        if self.failed {
            return;
        }
        if self.working()
            || self.story_delivery.is_some()
            || self.pending.iter().any(Option::is_some)
            || self.started.iter().any(Option::is_some)
            || !self.notifications.is_empty()
        {
            return;
        }
        if let Some(request) = listener.take_story() {
            if let Some(profile) = self.profile.clone() {
                self.story.spawn(async move {
                    let result = profile.story(&request, utc).await;
                    (request, result)
                });
            } else {
                warn!(
                    connection = self.connection,
                    "story persistence service not configured"
                );
                self.failed = true;
            }
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
        if let Some(request) = listener.take_garage() {
            if let Some(profile) = self.profile.clone() {
                self.garage.spawn(async move {
                    let result = profile.garage_command(&request, utc).await;
                    (request, result)
                });
            } else {
                warn!(
                    connection = self.connection,
                    "garage service not configured"
                );
            }
            return;
        }
        if let Some(request) = listener.take_award() {
            if let Some(profile) = self.profile.clone() {
                self.awards.spawn(async move {
                    let result = profile.credit(&request, utc).await;
                    (request, result)
                });
            } else {
                warn!(
                    connection = self.connection,
                    "reputation credit dropped: inventory service not configured"
                );
            }
            return;
        }
        if listener.has_challenge_request() {
            if let Some(profile) = self.profile.clone() {
                self.challenges
                    .spawn(async move { profile.current_challenges(utc).await });
            } else {
                warn!(
                    connection = self.connection,
                    "challenge account service not configured"
                );
                self.failed = true;
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
                self.unsupported.record(UnsupportedRoute::File {
                    handler,
                    name_bytes: request.name.len(),
                });
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
                    self.unsupported.record(UnsupportedRoute::Items {
                        handler,
                        operation: request.bytes[4],
                    });
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
            let car_request = request.bytes[4] == nfs_world::items::SELL_ITEMS
                || (request.bytes[4] == nfs_world::items::PURCHASE_ITEMS
                    && nfs_world::items::PurchaseRequest::decode(&request.bytes)
                        .is_ok_and(|p| p.owner == 0));
            if car_request && !listener.garage_ready() {
                // A loading garage is a normal rejected transaction, not an
                // unsupported request that leaves the client's File waiting.
                let mut bytes = request.bytes[..4].to_vec();
                bytes.push(3);
                self.started[index] = Some(now);
                self.pending[index] = Some(Completed {
                    name: b"Items".to_vec(),
                    bytes,
                });
                return;
            }
            if request.bytes[4] == nfs_world::items::PURCHASE_ITEMS && !listener.purchase_ready() {
                warn!(
                    connection = self.connection,
                    handler, "purchase before owned inventory is ready"
                );
                continue;
            }
            self.started[index] = Some(now);
            let operation = request.bytes[4];
            self.jobs
                .spawn(async move { (index, operation, profile.answer(request.bytes, utc).await) });
            break;
        }
    }
    pub(super) async fn completed(&mut self, listener: &mut PlayerListener) -> bool {
        if !self.challenges.is_empty() {
            return match self.challenges.join_next().await {
                Some(Ok(Ok(current))) => match listener.challenges_loaded(current) {
                    Ok(()) => true,
                    Err(error) => {
                        warn!(
                            connection = self.connection,
                            ?error,
                            "challenge cache rejected"
                        );
                        false
                    }
                },
                failure => {
                    warn!(
                        connection = self.connection,
                        ?failure,
                        "challenge account read failed"
                    );
                    false
                }
            };
        }
        if !self.story.is_empty() {
            let Some(Ok((request, result))) = self.story.join_next().await else {
                return false;
            };
            return match result {
                Ok(mut committed) => {
                    self.notifications.extend(
                        std::mem::take(&mut committed.notifications)
                            .into_iter()
                            .map(|bytes| Completed {
                                name: b"Items".to_vec(),
                                bytes,
                            }),
                    );
                    self.story_delivery = Some((request, committed));
                    true
                }
                Err(error) => {
                    warn!(
                        connection = self.connection,
                        ?error,
                        "story persistence failed"
                    );
                    false
                }
            };
        }
        if !self.garage.is_empty() {
            let Some(Ok((request, result))) = self.garage.join_next().await else {
                return false;
            };
            return match result {
                Ok(updated) => match listener.garage_inventory(
                    &updated.previous,
                    updated.loaded.garage(),
                    updated.loaded,
                    updated.items,
                ) {
                    Ok(()) => {
                        info!(connection = self.connection, ?request.operation, "owned garage command committed");
                        true
                    }
                    Err(error) => {
                        warn!(
                            connection = self.connection,
                            ?error,
                            "garage snapshot rejected"
                        );
                        false
                    }
                },
                Err(error) => {
                    warn!(
                        connection = self.connection,
                        ?error,
                        "garage command failed"
                    );
                    false
                }
            };
        }
        // start() serializes account writes across Files, equipment and credits.
        if !self.awards.is_empty() {
            let Some(Ok((request, result))) = self.awards.join_next().await else {
                return false;
            };
            return match result {
                Ok(mut credited) => {
                    let notifications = std::mem::take(&mut credited.notifications);
                    let unannounced = credited.unannounced;
                    match listener.awarded(&request, credited) {
                        Ok(()) => {
                            if unannounced > 0 {
                                warn!(
                                    connection = self.connection,
                                    batch_id = request.batch_id,
                                    unannounced,
                                    "credited inventory changes left for the next load"
                                );
                            }
                            if !notifications.is_empty() {
                                info!(
                                    connection = self.connection,
                                    batch_id = request.batch_id,
                                    count = notifications.len(),
                                    "credited inventory changes queued"
                                );
                            }
                            self.notifications
                                .extend(notifications.into_iter().map(|bytes| Completed {
                                    name: b"Items".to_vec(),
                                    bytes,
                                }));
                            true
                        }
                        Err(error) => {
                            warn!(
                                connection = self.connection,
                                ?error,
                                batch_id = request.batch_id,
                                "reputation credit snapshot rejected"
                            );
                            false
                        }
                    }
                }
                Err(error) => {
                    warn!(
                        connection = self.connection,
                        ?error,
                        batch_id = request.batch_id,
                        "reputation credit failed"
                    );
                    false
                }
            };
        }
        if !self.equipment.is_empty() {
            let Some(Ok((request, result))) = self.equipment.join_next().await else {
                return false;
            };
            return match result {
                Ok(Equipped {
                    loaded,
                    items,
                    notification,
                    edit,
                    presentation,
                }) => {
                    let generation = loaded.generation();
                    match listener.equipped_inventory(&request, loaded, items, &edit, presentation)
                    {
                        Ok(()) => {
                            self.pending[0] = Some(Completed {
                                name: b"Items".to_vec(),
                                bytes: notification,
                            });
                            info!(
                                connection = self.connection,
                                generation,
                                car = request.car,
                                items = ?request.selections,
                                dyno = request.dyno.is_some(),
                                handling = request.handling.iter().count(),
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
        let Some(Ok((index, operation, result))) = self.jobs.join_next().await else {
            return false;
        };
        match result {
            Ok(Some(LoadedReply {
                reply,
                items,
                kind,
                previous,
            })) => {
                let notifications = if kind == ReplyKind::Purchase {
                    let Some(profile) = &self.profile else {
                        return false;
                    };
                    match listener
                        .purchase_notifications(&items, profile.service.catalog.bindings())
                    {
                        Ok(notifications) => notifications,
                        Err(error) => {
                            warn!(
                                connection = self.connection,
                                ?error,
                                "purchase notification rejected"
                            );
                            return false;
                        }
                    }
                } else if kind == ReplyKind::Garage {
                    let Some(profile) = &self.profile else {
                        return false;
                    };
                    match listener
                        .garage_offer_notifications(&items, profile.service.catalog.bindings())
                    {
                        Ok(notifications) => notifications,
                        Err(_) => return false,
                    }
                } else {
                    Vec::new()
                };
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
                    ReplyKind::Garage => match (previous, reply.persistent.clone()) {
                        (Some(previous), Some(loaded)) if self.garage_delivery.is_none() => {
                            self.garage_delivery = Some(garage::Updated {
                                previous,
                                loaded,
                                items,
                            });
                            Ok(())
                        }
                        _ => Err(nfs_world::replication::Error::Shape),
                    },
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
                self.notifications
                    .extend(notifications.into_iter().map(|bytes| Completed {
                        name: b"Items".to_vec(),
                        bytes,
                    }));
                true
            }
            Ok(None) => {
                self.unsupported.record(UnsupportedRoute::Items {
                    handler: index + 4,
                    operation,
                });
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
    /// Unsolicited notifications use the request handler one at a time, each
    /// after the previous transfer completed (E926: an official level-up sent
    /// its notifications about 0.3 s apart).
    fn promote(&mut self) {
        if self.pending[0].is_none() && self.started[0].is_none() {
            self.pending[0] = self.notifications.pop_front();
        }
    }
    pub(super) fn advance(&mut self, host: &mut Host, events: &[Event], now: u64) -> bool {
        if self.failed {
            return false;
        }
        for event in events {
            match event {
                Event::FileComplete { handler } => self.started[usize::from(handler - 4)] = None,
                Event::FileError { .. } => return false,
                _ => {}
            }
        }
        self.promote();
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
