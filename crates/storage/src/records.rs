// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Account-owned opaque livery records. Names are database keys, never paths.
//! PUT replaces one complete record atomically; an identical retry preserves
//! timestamps. HTTP framing and game payload interpretation belong elsewhere.
use crate::{AccountId, Error, SqliteRepository};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

pub const MAX_RECORD_BYTES: usize = 512 * 1024;
pub const MAX_RECORDS: usize = 256;
pub const MAX_ACCOUNT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 128;

/// Microseconds since the Unix epoch, as used by livery record HTTP metadata.
/// Kept distinct from the inventory's millisecond timestamp type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnixMicros(pub u64);

#[derive(Clone, PartialEq, Eq)]
pub struct Record {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub created: UnixMicros,
    pub updated: UnixMicros,
}
impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveryRecord")
            .field("bytes", &self.bytes.len())
            .field("created", &self.created)
            .field("updated", &self.updated)
            .finish_non_exhaustive()
    }
}

pub fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn validate_payload(content_type: &str, bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::Bounds);
    }
    if content_type.is_empty()
        || content_type.len() > 128
        || !content_type
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/.-_+".contains(&c))
        || !content_type.contains('/')
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(crate) fn create_table(tx: &Transaction<'_>) -> Result<(), Error> {
    tx.execute_batch(&format!(
        "CREATE TABLE livery_record(\
         name TEXT PRIMARY KEY CHECK(length(name) BETWEEN 1 AND {MAX_NAME_BYTES}),\
         content_type TEXT NOT NULL CHECK(length(content_type) BETWEEN 1 AND 128),\
         value BLOB NOT NULL CHECK(length(value)<={MAX_RECORD_BYTES}),\
         created INTEGER NOT NULL CHECK(created>0),\
         updated INTEGER NOT NULL CHECK(updated>=created)) STRICT;"
    ))?;
    Ok(())
}
/// Record count and total value bytes, without loading any value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub records: usize,
    pub bytes: usize,
}
/// A stored total above the account bounds is a damaged store, not a count.
pub(crate) fn summary(conn: &Connection) -> Result<Summary, Error> {
    let (count, total): (i64, i64) = conn.query_row(
        "SELECT count(*),coalesce(sum(length(value)),0) FROM livery_record",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let records = usize::try_from(count).map_err(|_| Error::Config)?;
    let bytes = usize::try_from(total).map_err(|_| Error::Config)?;
    if records > MAX_RECORDS || bytes > MAX_ACCOUNT_BYTES {
        return Err(Error::Config);
    }
    Ok(Summary { records, bytes })
}
fn read(tx: &Transaction<'_>, name: &str) -> Result<Option<Record>, Error> {
    let row = tx
        .query_row(
            "SELECT content_type,value,created,updated FROM livery_record WHERE name=?1",
            [name],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(content_type, bytes, created, updated)| {
        validate_payload(&content_type, &bytes).map_err(|_| Error::Config)?;
        if created <= 0 || updated < created {
            return Err(Error::Config);
        }
        Ok(Record {
            content_type,
            bytes,
            created: UnixMicros(created as u64),
            updated: UnixMicros(updated as u64),
        })
    })
    .transpose()
}

/// A synchronous port: socket adapters must use a bounded blocking worker.
pub trait Repository: Send + Sync {
    fn get(&self, account: AccountId, name: &str) -> Result<Option<Record>, Error>;
    fn put(
        &self,
        account: AccountId,
        name: &str,
        content_type: &str,
        bytes: &[u8],
        now: UnixMicros,
    ) -> Result<Record, Error>;
    fn delete(&self, account: AccountId, name: &str) -> Result<(), Error>;
}

/// Uses the account's existing inventory database and ownership/version guards.
#[derive(Clone, Debug)]
pub struct SqliteRecords(pub SqliteRepository);
impl Repository for SqliteRecords {
    fn get(&self, account: AccountId, name: &str) -> Result<Option<Record>, Error> {
        validate_name(name)?;
        let mut conn = self.0.connect(account, false)?;
        let tx = conn.transaction()?;
        SqliteRepository::check(&tx, account)?;
        read(&tx, name)
    }
    fn put(
        &self,
        account: AccountId,
        name: &str,
        content_type: &str,
        bytes: &[u8],
        now: UnixMicros,
    ) -> Result<Record, Error> {
        validate_name(name)?;
        validate_payload(content_type, bytes)?;
        if now.0 == 0 || now.0 > i64::MAX as u64 {
            return Err(Error::Bounds);
        }
        let mut conn = self.0.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        SqliteRepository::check(&tx, account)?;
        let prior = read(&tx, name)?;
        if let Some(prior) = &prior
            && prior.content_type == content_type
            && prior.bytes == bytes
        {
            return Ok(prior.clone());
        }
        let (count, total): (i64, i64) = tx.query_row(
            "SELECT count(*),coalesce(sum(length(value)),0) FROM livery_record",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count > MAX_RECORDS as i64 || total > MAX_ACCOUNT_BYTES as i64 || count < 0 || total < 0
        {
            return Err(Error::Config);
        }
        let next_count = count as usize + usize::from(prior.is_none());
        let next_bytes = (total as usize)
            .checked_sub(prior.as_ref().map_or(0, |r| r.bytes.len()))
            .and_then(|v| v.checked_add(bytes.len()))
            .ok_or(Error::Config)?;
        if next_count > MAX_RECORDS || next_bytes > MAX_ACCOUNT_BYTES {
            return Err(Error::Bounds);
        }
        let created = prior.as_ref().map_or(now, |r| r.created);
        let updated = prior
            .as_ref()
            .map_or(now, |r| UnixMicros(now.0.max(r.updated.0)));
        tx.execute("INSERT INTO livery_record VALUES(?1,?2,?3,?4,?5) ON CONFLICT(name) DO UPDATE SET content_type=excluded.content_type,value=excluded.value,updated=excluded.updated",
            params![name, content_type, bytes, created.0 as i64, updated.0 as i64])?;
        tx.commit()?;
        Ok(Record {
            content_type: content_type.into(),
            bytes: bytes.to_vec(),
            created,
            updated,
        })
    }
    fn delete(&self, account: AccountId, name: &str) -> Result<(), Error> {
        validate_name(name)?;
        let mut conn = self.0.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        SqliteRepository::check(&tx, account)?;
        tx.execute("DELETE FROM livery_record WHERE name=?1", [name])?;
        tx.commit()?;
        Ok(())
    }
}
