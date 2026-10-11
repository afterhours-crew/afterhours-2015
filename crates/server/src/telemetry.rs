// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::telemetry::*;
pub fn reply(state: &mut Telemetry, wire: &[u8]) -> Result<Vec<u8>, crate::Failure> {
    state.reply(wire).map_err(|error| match error {
        Error::IneligibleFrame | Error::Body | Error::Identity => crate::Failure::IneligibleRequest,
        Error::Encode => crate::Failure::Reply,
    })
}
