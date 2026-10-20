// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only inspection of one account database for local tools, such as a
//! launcher showing the player's profile while the server runs.
//!
//! The connection is opened read-only with `query_only` and never
//! checkpoints or deletes a writer's write-ahead log when it closes. Every
//! domain is read inside one read transaction, so the result is one
//! committed state even while a server commits concurrently, and a lock wait
//! is bounded by the adapter's busy timeout. Nothing is created, migrated or
//! repaired: an absent store, another account's store or another schema
//! version is refused. Like any WAL reader, SQLite may create the empty log
//! and shared-memory files beside the database when they are absent; the
//! database file itself is never written.
use crate::{
    AccountId, Error, Snapshot, SqliteRepository,
    account::{Document, Kind},
    records,
};
use rusqlite::TransactionBehavior;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    /// Schema version of the store; only [`crate::sqlite::VERSION`] is read.
    pub schema_version: i64,
    /// Main database file size when the connection opened. Recently
    /// committed pages may still be in the write-ahead log.
    pub file_bytes: u64,
    pub snapshot: Snapshot,
    /// Present account domains only; an absent domain has no entry.
    pub documents: BTreeMap<Kind, Document>,
    pub livery: records::Summary,
}

impl SqliteRepository {
    /// One consistent, read-only view of the account's committed state.
    pub fn inspect(&self, account: AccountId) -> Result<Inspection, Error> {
        let (mut conn, file_bytes) = self.connect_read_only(account)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let snapshot = Self::snapshot_in(&tx, account)?;
        let mut documents = BTreeMap::new();
        for kind in Kind::ALL {
            if let Some(document) = crate::account::read_in(&tx, kind)? {
                documents.insert(kind, document);
            }
        }
        let livery = records::summary(&tx)?;
        tx.commit()?;
        Ok(Inspection {
            schema_version: crate::sqlite::VERSION,
            file_bytes,
            snapshot,
            documents,
            livery,
        })
    }
}
