// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;

pub fn reply(bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    nfs_services::client_state::reply(bytes).map_err(|error| match error {
        nfs_services::client_state::Error::IneligibleRequest => Failure::IneligibleRequest,
        nfs_services::client_state::Error::Encode => Failure::Reply,
    })
}
