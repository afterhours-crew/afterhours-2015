// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::world_attributes::{Error, Session};
#[derive(Clone)]
pub(crate) struct Config {
    inner: nfs_services::world_attributes::Config,
}
impl std::ops::Deref for Config {
    type Target = nfs_services::world_attributes::Config;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Config {
    pub(crate) fn owned(policy: [u8; 32]) -> Result<Self, Error> {
        if policy == [0; 32] {
            return Err(Error::Source);
        }
        Ok(Self {
            inner: nfs_services::world_attributes::Config::new(policy),
        })
    }
}
