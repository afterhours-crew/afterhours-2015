// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::user_settings::*;
pub fn failure(error: Error) -> crate::Failure {
    match error {
        Error::Config => crate::Failure::ProfileConfig,
        Error::Bounds => crate::Failure::BodyLimit,
        Error::Ineligible => crate::Failure::IneligibleRequest,
        Error::Encode | Error::Phase => crate::Failure::Reply,
        Error::Storage | Error::Conflict => crate::Failure::Output,
    }
}
