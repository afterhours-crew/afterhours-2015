// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
pub use nfs_services::world_connection::{Current, Session};
pub(crate) fn failure(e: nfs_services::world_connection::Error) -> Failure {
    use nfs_services::world_connection::Error;
    match e {
        Error::Context => Failure::ProfileConfig,
        Error::Encode => Failure::Reply,
        _ => Failure::IneligibleRequest,
    }
}
#[derive(Clone)]
pub(crate) struct Config {
    inner: nfs_services::world_connection::Config,
}
impl std::ops::Deref for Config {
    type Target = nfs_services::world_connection::Config;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Config {
    pub(crate) fn owned() -> Self {
        Self {
            inner: nfs_services::world_connection::Config,
        }
    }
}
