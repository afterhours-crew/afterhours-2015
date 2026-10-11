// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded service state in the same account database as inventory and tables.
//! Domain codecs validate/serialize these values; storage never contains wire
//! replies. Revisions prevent stale sessions from overwriting committed state.
use crate::{AccountId, Applied, Batch, Error, InventoryRepository, SqliteRepository, Timestamp};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Identity,
    Entitlements,
    Kickback,
    Speedwall,
    Settings,
}
impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Identity,
        Self::Entitlements,
        Self::Kickback,
        Self::Speedwall,
        Self::Settings,
    ];
    const fn table(self) -> &'static str {
        match self {
            Self::Identity => "identity_state",
            Self::Entitlements => "entitlement_state",
            Self::Kickback => "kickback_state",
            Self::Speedwall => "speedwall_state",
            Self::Settings => "user_settings",
        }
    }
    pub const fn max_bytes(self) -> usize {
        match self {
            Self::Identity => 4096,
            Self::Entitlements | Self::Settings => 512 * 1024,
            Self::Kickback | Self::Speedwall => 64 * 1024,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Document {
    pub revision: u64,
    pub bytes: Vec<u8>,
}
impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountDocument")
            .field("revision", &self.revision)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
fn validate(kind: Kind, revision: u64, bytes: &[u8]) -> Result<(), Error> {
    if revision == 0
        || revision > i64::MAX as u64
        || bytes.is_empty()
        || bytes.len() > kind.max_bytes()
    {
        return Err(Error::Bounds);
    }
    Ok(())
}
pub(crate) fn create_tables(tx: &Transaction<'_>) -> Result<(), Error> {
    for kind in Kind::ALL {
        tx.execute_batch(&format!("CREATE TABLE {}(singleton INTEGER PRIMARY KEY CHECK(singleton=1), revision INTEGER NOT NULL CHECK(revision>0), value BLOB NOT NULL CHECK(length(value)>0 AND length(value)<={})) STRICT;", kind.table(), kind.max_bytes()))?;
    }
    Ok(())
}
fn read_in(conn: &Connection, kind: Kind) -> Result<Option<Document>, Error> {
    let count: i64 =
        conn.query_row(&format!("SELECT count(*) FROM {}", kind.table()), [], |r| {
            r.get(0)
        })?;
    if count > 1 {
        return Err(Error::Config);
    }
    let row = conn
        .query_row(
            &format!(
                "SELECT revision,value FROM {} WHERE singleton=1",
                kind.table()
            ),
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    row.map(|(revision, bytes)| {
        let revision = u64::try_from(revision).map_err(|_| Error::Config)?;
        validate(kind, revision, &bytes).map_err(|_| Error::Config)?;
        Ok(Document { revision, bytes })
    })
    .transpose()
}

#[derive(Clone, Debug)]
pub struct Repository {
    inventory: SqliteRepository,
}
impl Repository {
    /// Publish all account domains and initial inventory/tables together. Only
    /// an untouched store is eligible; this cannot reset or reseed an account.
    pub fn initialize_fresh(
        &self,
        account: AccountId,
        documents: &[(Kind, Document)],
        batch: &Batch,
        now: Timestamp,
    ) -> Result<Applied, Error> {
        let mut kinds = BTreeSet::new();
        for (kind, document) in documents {
            if !kinds.insert(*kind) || document.revision != 1 {
                return Err(Error::Invalid);
            }
            validate(*kind, document.revision, &document.bytes)?;
        }
        if kinds != Kind::ALL.into_iter().collect() || batch.expected_generation != 0 {
            return Err(Error::Invalid);
        }
        batch.validate()?;
        self.prepare(account)?;
        let mut conn = self.inventory.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = SqliteRepository::snapshot_in(&tx, account)?;
        let history: i64 = tx.query_row("SELECT count(*) FROM batch", [], |r| r.get(0))?;
        if history != 0 || snapshot.updated_at != Timestamp(0) {
            return Err(Error::Conflict);
        }
        if snapshot.generation != 0
            || snapshot.garage.is_some()
            || !snapshot.items.is_empty()
            || !snapshot.tables.is_empty()
        {
            return Err(Error::Conflict);
        }
        for (kind, document) in documents {
            if read_in(&tx, *kind)?.is_some() {
                return Err(Error::Conflict);
            }
            tx.execute(
                &format!("INSERT INTO {} VALUES(1,?1,?2)", kind.table()),
                params![1, document.bytes],
            )?;
        }
        let applied = SqliteRepository::apply_in(&tx, account, batch, now)?;
        tx.commit()?;
        Ok(applied)
    }
    pub fn open_owned_directory(root: &Path) -> Result<Self, Error> {
        Ok(Self {
            inventory: SqliteRepository::open_owned_directory(root)?,
        })
    }
    pub fn path(&self, account: AccountId) -> PathBuf {
        self.inventory.path(account)
    }
    /// Accept ownership and migrate the database before reading service state.
    /// This does not seed inventory or invent any domain defaults.
    pub fn prepare(&self, account: AccountId) -> Result<(), Error> {
        self.inventory.open(account).map(|_| ())
    }
    pub fn read(&self, account: AccountId, kind: Kind) -> Result<Option<Document>, Error> {
        let mut conn = match self.inventory.connect(account, false) {
            Err(Error::Absent) => return Ok(None),
            other => other?,
        };
        let tx = conn.transaction()?;
        SqliteRepository::check(&tx, account)?;
        read_in(&tx, kind)
    }
    /// Atomically import missing domains. Existing state always wins, including
    /// on retry after a successful commit whose result was lost. Imported
    /// revisions survive migration from the old standalone settings store.
    /// Callers must validate all domain values before calling this method.
    pub fn initialize(
        &self,
        account: AccountId,
        documents: &[(Kind, Document)],
    ) -> Result<(), Error> {
        let mut kinds = BTreeSet::new();
        for (kind, document) in documents {
            if !kinds.insert(*kind) {
                return Err(Error::Invalid);
            }
            validate(*kind, document.revision, &document.bytes)?;
        }
        self.prepare(account)?;
        let mut conn = self.inventory.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        SqliteRepository::check(&tx, account)?;
        for (kind, document) in documents {
            if read_in(&tx, *kind)?.is_none() {
                tx.execute(
                    &format!("INSERT INTO {} VALUES(1,?1,?2)", kind.table()),
                    params![document.revision as i64, document.bytes],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// A stale revision returns false without changing data. Successful writes
    /// have committed before returning; callers must acknowledge afterwards.
    pub fn compare_exchange(
        &self,
        account: AccountId,
        kind: Kind,
        expected: Option<u64>,
        bytes: &[u8],
    ) -> Result<bool, Error> {
        let revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Bounds)?;
        validate(kind, revision, bytes)?;
        let mut conn = self.inventory.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        SqliteRepository::check(&tx, account)?;
        if read_in(&tx, kind)?.as_ref().map(|d| d.revision) != expected {
            return Ok(false);
        }
        tx.execute(&format!("INSERT INTO {} VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,value=excluded.value", kind.table()), params![revision as i64, bytes])?;
        tx.commit()?;
        Ok(true)
    }
}
