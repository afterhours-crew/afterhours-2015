// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Requests to the owning event's teleport manager. The location is a provider,
//! not a world transform; the manager resolves it at execution time.
use super::participant_properties::Property;
pub const SET: u32 = 0x0819_20ba;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub node: u16,
    pub participant: u64,
    pub location: Property,
    pub manager: Property,
}
