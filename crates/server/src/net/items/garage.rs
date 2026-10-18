// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::vehicle_content::population::Inventory;
use nfs_services::garage::{self, CommandId};
use nfs_world::items::{PurchaseReply, PurchaseRequest, SellReply, SellRequest};

pub(super) struct Updated {
    pub previous: Inventory,
    pub loaded: Arc<crate::persistent::Loaded>,
    pub items: Arc<nfs_world::items::Collection>,
}
fn inventory(service: &Service, snapshot: &nfs_storage::Snapshot) -> Result<Inventory, Error> {
    Ok(Inventory {
        slots: nfs_world::garage::Slots::new(
            snapshot
                .garage
                .ok_or(Error::Worker)?
                .0
                .map(|v| v.map(nfs_storage::ItemId::get)),
        )
        .map_err(|_| Error::Worker)?,
        items: Arc::new(
            crate::inventory::collection(snapshot, service.catalog.bindings())
                .map_err(Error::Inventory)?,
        ),
    })
}
fn loaded(
    service: &Service,
    snapshot: &nfs_storage::Snapshot,
) -> Result<Arc<crate::persistent::Loaded>, Error> {
    service
        .persistent
        .as_ref()
        .ok_or(Error::Worker)?
        .view(snapshot)
        .map(Arc::new)
        .map_err(|e| Error::Inventory(e.into()))
}
impl Profile {
    pub(super) async fn answer_garage(
        &self,
        bytes: Vec<u8>,
        now: Timestamp,
    ) -> Result<Option<LoadedReply>, Error> {
        if self.service.car_offers.is_none() {
            return Ok(None);
        }
        let service = self.service.clone();
        let account = self.account;
        let session = self.garage_session.clone();
        timeout(JOB_TIMEOUT, async move {
            let permit = service
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Error::Worker)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let epoch = {
                    let mut guard = session.lock().map_err(|_| Error::Worker)?;
                    match *guard {
                        Some(epoch) => epoch,
                        None => {
                            let epoch =
                                garage::open_session(service.repository.as_ref(), account, now)
                                    .map_err(Error::Garage)?;
                            *guard = Some(epoch);
                            epoch
                        }
                    }
                };
                let snapshot = service
                    .repository
                    .snapshot(account)
                    .map_err(|e| Error::Inventory(e.into()))?;
                let previous = inventory(&service, &snapshot)?;
                let (sequence, result, selling) = if bytes[4] == nfs_world::items::SELL_ITEMS {
                    let request =
                        SellRequest::decode(&bytes).map_err(|e| Error::Inventory(e.into()))?;
                    (
                        request.sequence,
                        garage::sell(
                            service.repository.as_ref(),
                            account,
                            &service.catalog,
                            CommandId {
                                session: epoch,
                                sequence: request.sequence,
                            },
                            &request,
                            now,
                        ),
                        true,
                    )
                } else {
                    let request =
                        PurchaseRequest::decode(&bytes).map_err(|e| Error::Inventory(e.into()))?;
                    (
                        request.sequence,
                        garage::buy(
                            service.repository.as_ref(),
                            account,
                            &service.catalog,
                            service.car_offers.as_deref().ok_or(Error::Worker)?,
                            CommandId {
                                session: epoch,
                                sequence: request.sequence,
                            },
                            &request,
                            now,
                        ),
                        false,
                    )
                };
                let (snapshot, records, kind) = match result {
                    Ok(changed) => (changed.snapshot, Some(changed.items), ReplyKind::Garage),
                    Err(garage::Error::Rejected(reason)) => {
                        info!(?reason, selling, "owned car transaction rejected");
                        (snapshot, None, ReplyKind::Rejected)
                    }
                    Err(error) => return Err(Error::Garage(error)),
                };
                let next = inventory(&service, &snapshot)?;
                let persistent = loaded(&service, &snapshot)?;
                let bytes = if selling {
                    SellReply {
                        sequence,
                        sold: records,
                    }
                    .encode(service.catalog.bindings())
                } else {
                    PurchaseReply {
                        sequence,
                        purchased: records,
                    }
                    .encode(service.catalog.bindings())
                }
                .map_err(|e| Error::Inventory(e.into()))?;
                Ok(Some(LoadedReply {
                    reply: crate::inventory::Reply {
                        generation: snapshot.generation,
                        bytes,
                        garage: next.slots.values(),
                        persistent: Some(persistent),
                    },
                    items: next.items,
                    kind,
                    previous: Some(previous),
                }))
            })
            .await
            .map_err(|_| Error::Worker)?
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    pub(super) async fn garage_command(
        &self,
        request: &GarageRequest,
        now: Timestamp,
    ) -> Result<Updated, Error> {
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
                let before = service
                    .repository
                    .snapshot(account)
                    .map_err(|e| Error::Inventory(e.into()))?;
                let previous = inventory(&service, &before)?;
                let snapshot = match request.operation {
                    GarageOperation::Select {
                        item,
                        previous_primary,
                    } => garage::select(
                        service.repository.as_ref(),
                        account,
                        &service.catalog,
                        item,
                        previous_primary,
                        now,
                    )
                    .map_err(Error::Garage)?,
                    GarageOperation::Place { item, secondary } => garage::place(
                        service.repository.as_ref(),
                        account,
                        &service.catalog,
                        item,
                        secondary,
                        now,
                    )
                    .map_err(Error::Garage)?,
                    GarageOperation::Sold { secondary } => {
                        if previous.slots.values()[usize::from(secondary) + 1].is_some() {
                            return Err(Error::Garage(garage::Error::Rejected(
                                garage::Reason::Owner,
                            )));
                        }
                        before
                    }
                };
                Ok(Updated {
                    previous,
                    loaded: loaded(&service, &snapshot)?,
                    items: inventory(&service, &snapshot)?.items,
                })
            })
            .await
            .map_err(|_| Error::Worker)?
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
}
