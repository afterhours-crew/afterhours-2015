// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Server-issued account session epochs separate reconnect sequence spaces.
//! High-water marks survive response eviction, so an old purchase cannot become
//! a new charge. Response bytes are generated owned state, never captured data.
use super::{Error, Reason, apply};
use nfs_storage::{
    AccountId, InventoryRepository, Op, Snapshot, Timestamp,
    tables::{Row, Table, Value},
};
use nfs_world_core::items::{Catalog, Collection};
use std::collections::BTreeMap;
const MAX_SESSIONS: usize = 64;
const MAX_REPLY: usize = 32 * 1024;
fn key() -> u32 {
    crate::persistent::key("AfterhoursGarageSessions")
}
fn replies_key() -> u32 {
    crate::persistent::key("AfterhoursGarageReplies")
}
fn num(n: u64) -> Value {
    Value::String(n.to_string())
}
fn number(row: &Row, column: u32) -> Result<u64, Error> {
    let Some(Value::String(s)) = row.get(&column) else {
        return Err(nfs_storage::Error::Config.into());
    };
    let n = s.parse::<u64>().map_err(|_| nfs_storage::Error::Config)?;
    if n.to_string() != *s {
        return Err(nfs_storage::Error::Config.into());
    }
    Ok(n)
}
fn sessions(snapshot: &Snapshot) -> Result<Table, Error> {
    let table = snapshot
        .tables
        .get(&key())
        .cloned()
        .unwrap_or_else(|| Table {
            rows: BTreeMap::from([(0, Row::from([(0, Value::Int(1)), (1, num(1))]))]),
        });
    let header = table.rows.get(&0).ok_or(nfs_storage::Error::Config)?;
    let next = number(header, 1)?;
    if header.len() != 2
        || header.get(&0) != Some(&Value::Int(1))
        || next == 0
        || table.rows.len() > MAX_SESSIONS + 1
    {
        return Err(nfs_storage::Error::Config.into());
    }
    for (&epoch, row) in table.rows.iter().filter(|(k, _)| **k != 0) {
        if epoch >= next || row.len() != 1 || number(row, 0)? > u64::from(u32::MAX) {
            return Err(nfs_storage::Error::Config.into());
        }
    }
    table.validate()?;
    Ok(table)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandId {
    pub session: u64,
    pub sequence: u32,
}

/// Issue a fresh account epoch on the blocking storage edge. Never use a wire
/// persona, client sequence or process-local counter as a durable session ID.
pub fn open_session(
    repository: &dyn InventoryRepository,
    account: AccountId,
    now: Timestamp,
) -> Result<u64, Error> {
    for _ in 0..8 {
        let snapshot = repository.snapshot(account)?;
        let mut table = sessions(&snapshot)?;
        let epoch = number(&table.rows[&0], 1)?;
        table.rows.get_mut(&0).unwrap().insert(
            1,
            num(epoch.checked_add(1).ok_or(nfs_storage::Error::Bounds)?),
        );
        table.rows.insert(epoch, Row::from([(0, num(0))]));
        while table.rows.len() > MAX_SESSIONS + 1 {
            let oldest = *table.rows.keys().find(|k| **k != 0).unwrap();
            table.rows.remove(&oldest);
        }
        let mut replies = snapshot
            .tables
            .get(&replies_key())
            .cloned()
            .unwrap_or_default();
        replies
            .rows
            .retain(|epoch, _| table.rows.contains_key(epoch));
        if apply(
            repository,
            account,
            &snapshot,
            vec![
                Op::SetTable(key(), table),
                Op::SetTable(replies_key(), replies),
            ],
            now,
        )? {
            return Ok(epoch);
        }
    }
    Err(nfs_storage::Error::Conflict.into())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(text: &str) -> Result<Vec<u8>, Error> {
    if !text.len().is_multiple_of(2)
        || text.len() > MAX_REPLY * 2
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(nfs_storage::Error::Config.into());
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            u8::from_str_radix(
                std::str::from_utf8(p).map_err(|_| nfs_storage::Error::Config)?,
                16,
            )
            .map_err(|_| nfs_storage::Error::Config.into())
        })
        .collect()
}
pub(super) fn replay(
    snapshot: &Snapshot,
    id: CommandId,
    request: &[u8],
    catalog: &Catalog,
) -> Result<Option<Collection>, Error> {
    if id.sequence < 2 || id.session == 0 {
        return Err(Error::Rejected(Reason::Shape));
    }
    let table = sessions(snapshot)?;
    let row = table
        .rows
        .get(&id.session)
        .ok_or(Error::Rejected(Reason::Expired))?;
    let sequence = number(row, 0)?;
    if u64::from(id.sequence) > sequence {
        return Ok(None);
    }
    if u64::from(id.sequence) < sequence {
        return Err(Error::Rejected(Reason::Expired));
    }
    let receipt = snapshot
        .tables
        .get(&replies_key())
        .and_then(|t| t.rows.get(&id.session))
        .ok_or(Error::Rejected(Reason::Expired))?;
    if receipt.get(&0) != Some(&Value::String(hex(request))) {
        return Err(Error::Rejected(Reason::Shape));
    }
    let mut encoded = String::new();
    for (&column, value) in receipt.iter().filter(|(k, _)| **k != 0) {
        if column as usize != encoded.len() / 4096 + 1 {
            return Err(nfs_storage::Error::Config.into());
        }
        let Value::String(chunk) = value else {
            return Err(nfs_storage::Error::Config.into());
        };
        if chunk.len() > 4096 || encoded.len() + chunk.len() > MAX_REPLY * 2 {
            return Err(nfs_storage::Error::Config.into());
        }
        encoded.push_str(chunk);
    }
    Collection::decode(&unhex(&encoded)?, catalog)
        .map(Some)
        .map_err(crate::inventory::Error::from)
        .map_err(Into::into)
}
pub(super) fn record(
    snapshot: &Snapshot,
    id: CommandId,
    request: &[u8],
    reply: &Collection,
    catalog: &Catalog,
) -> Result<Vec<Op>, Error> {
    let mut table = sessions(snapshot)?;
    let row = table
        .rows
        .get_mut(&id.session)
        .ok_or(Error::Rejected(Reason::Expired))?;
    if u64::from(id.sequence) <= number(row, 0)? {
        return Err(Error::Rejected(Reason::Expired));
    }
    row.insert(0, num(id.sequence.into()));
    let bytes = reply
        .encode(catalog)
        .map_err(crate::inventory::Error::from)?;
    if bytes.len() > MAX_REPLY || request.len() > 2048 {
        return Err(nfs_storage::Error::Bounds.into());
    }
    let mut receipt = Row::from([(0, Value::String(hex(request)))]);
    for (i, chunk) in hex(&bytes).as_bytes().chunks(4096).enumerate() {
        receipt.insert(
            i as u32 + 1,
            Value::String(
                String::from_utf8(chunk.to_vec()).map_err(|_| nfs_storage::Error::Config)?,
            ),
        );
    }
    let mut replies = snapshot
        .tables
        .get(&replies_key())
        .cloned()
        .unwrap_or_default();
    replies.rows.insert(id.session, receipt);
    while replies.validate().is_err() {
        let oldest = *replies
            .rows
            .keys()
            .find(|k| **k != id.session)
            .ok_or(nfs_storage::Error::Bounds)?;
        replies.rows.remove(&oldest);
    }
    Ok(vec![
        Op::SetTable(key(), table),
        Op::SetTable(replies_key(), replies),
    ])
}
