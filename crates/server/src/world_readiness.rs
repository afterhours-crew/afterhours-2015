// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::world_readiness::{
    Binding, ContinuationPermit, Decision, Error, InitialReliableSyncPrerequisite,
    MAX_HOST_REQUESTS, PreparedBatch, Session, frame, wire,
};
#[derive(Clone)]
pub(crate) struct Config {
    inner: nfs_services::world_readiness::Config,
}
impl std::ops::Deref for Config {
    type Target = nfs_services::world_readiness::Config;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Config {
    pub(crate) fn owned(policy_id: [u8; 32]) -> Result<Self, Error> {
        if policy_id == [0; 32] {
            return Err(Error::Source);
        }
        Ok(Self {
            inner: nfs_services::world_readiness::Config::new(policy_id),
        })
    }
}
