// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Session-owned map eligibility. Immutable authored definitions do not store
//! account or scene membership, and candidate state is committed atomically.
pub mod channels;
mod content;
pub mod copies;
pub use content::Content;
mod saved;
mod state;
#[cfg(test)]
pub(crate) use content::tests::gated_fixture;
pub use state::{Participant, State};
