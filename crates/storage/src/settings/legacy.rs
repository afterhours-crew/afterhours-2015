// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only importer for the former standalone settings database.
use super::{Document, MAX_BYTES};
use crate::{AccountId, Error};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{path::Path, time::Duration};

pub(super) fn read(root: &Path, account: AccountId) -> Result<Option<Document>, Error> {
    let path = root.join(format!("settings-{}.sqlite", account.hex()));
    match std::fs::symlink_metadata(&path) {
        Ok(m) if !m.is_file() || m.file_type().is_symlink() || m.len() > 4 * 1024 * 1024 => {
            return Err(Error::Config);
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Storage),
    }
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(250))?;
    conn.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        (MAX_BYTES + 1024) as i32,
    )?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, 8192)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")?;
    let tx = conn.transaction()?;
    let app: i64 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let objects: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    if app != 0x4e465350 || version != 1 || objects != 1 {
        return Err(Error::Version);
    }
    let rows: i64 = tx.query_row("SELECT count(*) FROM settings", [], |r| r.get(0))?;
    if rows > 1 {
        return Err(Error::Config);
    }
    let row = tx
        .query_row(
            "SELECT account,revision,value FROM settings WHERE singleton=1",
            [],
            |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(|(owner, revision, bytes)| {
        if owner != account.bytes() {
            return Err(Error::Identity);
        }
        if revision < 1 || bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Error::Config);
        }
        Ok(Document {
            revision: revision as u64,
            bytes,
        })
    })
    .transpose()
}
