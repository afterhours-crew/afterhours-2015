// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local persistence port for account-owned server state.
//!
//! One durable store per stable local account holds the player's owned item
//! inventory: reference id, 16-byte definition GUID, owner, default items,
//! sub-items, state, buy and sell price, plus an opaque class-specific body.
//! Account identity comes from local configuration, never from a
//! wire value.
//!
//! The port is synchronous and free of Tokio: the server runs it off its async
//! workers. Two adapters implement the same [`InventoryRepository`] contract
//! and the same pure transition ([`port::transition`], [`port::decide`]):
//! [`memory::MemoryRepository`] for tests and [`sqlite::SqliteRepository`] for
//! durable state (one versioned SQLite file per account, transactional batches,
//! commit before acknowledgement, batch ids for retry safety).
//!
//! Bounds are local policy, not recovered native maxima. Nothing here claims
//! game persistence: a controlled change surviving a server and game restart
//! remains a separate live acceptance test.
#![forbid(unsafe_code)]

pub mod account;
pub mod memory;
pub mod model;
pub mod port;
pub mod settings;
pub mod sqlite;
pub mod tables;

pub use memory::MemoryRepository;
pub use model::{
    AccountId, Applied, Batch, DefinitionGuid, Error, GarageSlots, ItemId, ItemRecord,
    MAX_BATCH_HISTORY, MAX_DERIVED, MAX_ITEMS, MAX_NESTED, MAX_OPS, Op, Snapshot, Timestamp,
};
pub use port::InventoryRepository;
pub use sqlite::SqliteRepository;
