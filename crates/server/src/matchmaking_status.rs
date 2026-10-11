// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::matchmaking_status::{Accepted, Config, Failure, Session};
pub struct Profile {
    inner: Config,
}
impl std::ops::Deref for Profile {
    type Target = Config;
    fn deref(&self) -> &Config {
        &self.inner
    }
}
impl Profile {
    pub fn owned(inner: Config) -> Self {
        Self { inner }
    }
}
