// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::user_lookup::*;

pub(crate) fn failure(error: Error) -> crate::Failure {
    match error {
        Error::IneligibleRequest => crate::Failure::IneligibleRequest,
        Error::ProfileShape => crate::Failure::ProfileShape,
        Error::ProfileConfig => crate::Failure::ProfileConfig,
        Error::Reply => crate::Failure::Reply,
    }
}
