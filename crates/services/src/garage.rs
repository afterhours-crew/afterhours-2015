// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned vehicle acquisition, resale and five-slot selection. Installed
//! offers are explicit; client prices and arbitrary catalog GUIDs grant nothing.
mod requests;
pub use requests::{Binding, Call, Error as RpcError, Operation};
mod ledger;
mod ownership;
#[cfg(test)]
mod tests;
pub use ledger::{CommandId, open_session};
pub use ownership::{Changed, Offers, buy, place, select, sell};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    Shape,
    Offer,
    Funds,
    Full,
    Owner,
    Primary,
    Expired,
    Quantity,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Rejected(Reason),
    Storage(nfs_storage::Error),
    Inventory(crate::inventory::Error),
}
impl From<nfs_storage::Error> for Error {
    fn from(e: nfs_storage::Error) -> Self {
        Self::Storage(e)
    }
}
impl From<crate::inventory::Error> for Error {
    fn from(e: crate::inventory::Error) -> Self {
        Self::Inventory(e)
    }
}

const BATCH_NAMESPACE: u64 = 1 << 55;
fn apply(
    repository: &dyn nfs_storage::InventoryRepository,
    account: nfs_storage::AccountId,
    snapshot: &nfs_storage::Snapshot,
    ops: Vec<nfs_storage::Op>,
    now: nfs_storage::Timestamp,
) -> Result<bool, Error> {
    if snapshot.generation >= BATCH_NAMESPACE {
        return Err(nfs_storage::Error::Bounds.into());
    }
    match repository.apply(
        account,
        &nfs_storage::Batch {
            id: BATCH_NAMESPACE | snapshot.generation,
            expected_generation: snapshot.generation,
            ops,
        },
        now,
    ) {
        Ok(result) => Ok(!result.replayed),
        Err(nfs_storage::Error::Conflict) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
