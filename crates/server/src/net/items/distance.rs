// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One writer per world connection, shared by retries of its cumulative total.
use super::*;
use nfs_services::driving_distance::Claim;
use std::sync::Mutex;

#[derive(Clone, Default)]
pub(in crate::net) struct Writer(Arc<Mutex<Option<Claim>>>);
impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DistanceWriter").finish_non_exhaustive()
    }
}
impl PartialEq for Writer {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Writer {}

impl Writer {
    pub(super) fn checkpoint(
        &self,
        service: &Service,
        account: AccountId,
        cumulative: i32,
        now: Timestamp,
    ) -> Result<nfs_storage::Snapshot, Error> {
        let tables = service.persistent.as_deref().ok_or(Error::Worker)?;
        let mut state = self.0.lock().map_err(|_| Error::Worker)?;
        if state.is_none() {
            let identity =
                crate::seeds::fresh(&mut crate::seeds::OsSeeds).map_err(|_| Error::Worker)?;
            *state = Some(Claim::new(account, identity).map_err(|e| Error::Inventory(e.into()))?);
        }
        let claim = state.as_mut().ok_or(Error::Worker)?;
        if claim.account() != account {
            return Err(Error::Worker);
        }
        claim
            .checkpoint(service.repository.as_ref(), tables, cumulative, now)
            .map_err(|e| Error::Inventory(e.into()))
    }
}
