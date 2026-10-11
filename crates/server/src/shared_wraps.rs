// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub use nfs_services::shared_wraps::*;
pub fn reply(state: &mut Service, persona: i64, wire: &[u8]) -> Result<Vec<u8>, crate::Failure> {
    state.reply(wire, persona).map_err(|error| match error {
        Error::IneligibleFrame | Error::Body | Error::Scope => crate::Failure::IneligibleRequest,
        Error::Unsupported => crate::Failure::ProfileConfig,
        Error::Encode => crate::Failure::Reply,
    })
}
