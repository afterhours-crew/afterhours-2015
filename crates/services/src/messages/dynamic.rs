// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Attachment-bearing inbox rows. The authored capacity includes one Asset=0
//! cursor, leaving capacity-1 retained messages. Attachments are stored as
//! canonical padded base64 in Data; this module does not interpret their bytes.
//! Callers join the returned table operation to their delivery receipt and
//! commit before publication. Asset hashes are not unique delivery identities.
use crate::persistent::key;
use nfs_storage::{
    Error,
    tables::{Table, Value},
};

pub const TABLE: &str = "DynamicMessage";
/// Local delivery identities prevent an old live message from marking a new
/// identical delivery read after the bounded ring reuses its slot.
pub const RECEIPTS: &str = "AfterhoursDynamicMessages";
/// Native persistence accepts encoded strings shorter than 34 bytes. Base64
/// expands whole groups of four, so at most 24 attachment bytes fit. Oversized
/// data is rejected rather than persisting the native empty-data fallback.
pub const MAX_DATA: usize = 24;
const MAX_CAPACITY: u16 = 512;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub index: u64,
    pub hash: u32,
    pub read: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    pub row: Row,
    /// None identifies a legacy row written before local receipt tracking.
    pub generation: Option<u64>,
}

fn receipts(table: Option<&Table>) -> Result<std::collections::BTreeMap<u64, u64>, Error> {
    let Some(table) = table else {
        return Ok(Default::default());
    };
    table.validate()?;
    if table.rows.len() > usize::from(MAX_CAPACITY) {
        return Err(Error::Bounds);
    }
    table
        .rows
        .iter()
        .map(|(&index, row)| {
            let (Some(Value::Int(1)), Some(Value::String(text))) =
                (row.get(&key("Version")), row.get(&key("Generation")))
            else {
                return Err(Error::Invalid);
            };
            let generation: u64 = text.parse().map_err(|_| Error::Invalid)?;
            if index >= u64::from(MAX_CAPACITY)
                || row.len() != 2
                || generation == 0
                || generation.to_string() != *text
            {
                return Err(Error::Invalid);
            }
            Ok((index, generation))
        })
        .collect()
}

pub fn identities(
    table: Option<&Table>,
    receipt_table: Option<&Table>,
    capacity: u16,
) -> Result<Vec<Identity>, Error> {
    let rows = rows(table, capacity)?;
    let receipts = receipts(receipt_table)?;
    if receipts.keys().any(|i| !rows.iter().any(|r| r.index == *i)) {
        return Err(Error::Invalid);
    }
    Ok(rows
        .into_iter()
        .map(|row| Identity {
            generation: receipts.get(&row.index).copied(),
            row,
        })
        .collect())
}

/// Plan the ring and its private receipt together. The caller commits both with
/// the pending-intent removal; neither table is independently authoritative.
pub fn append_owned(
    snapshot: &nfs_storage::Snapshot,
    capacity: u16,
    hash: u32,
    data: &[u8],
) -> Result<(Vec<nfs_storage::Op>, Identity), Error> {
    let old = snapshot.tables.get(&key(TABLE));
    let receipt_table = snapshot.tables.get(&key(RECEIPTS));
    identities(old, receipt_table, capacity)?;
    let (table, row) = append(old, capacity, hash, data)?;
    let generation = snapshot.generation.checked_add(1).ok_or(Error::Bounds)?;
    let mut next = receipt_table.cloned().unwrap_or_default();
    next.rows.remove(&((row.index + 1) % u64::from(capacity)));
    next.rows.insert(
        row.index,
        [
            (key("Version"), Value::Int(1)),
            (key("Generation"), Value::String(generation.to_string())),
        ]
        .into(),
    );
    identities(Some(&table), Some(&next), capacity)?;
    Ok((
        vec![
            nfs_storage::Op::SetTable(key(TABLE), table),
            nfs_storage::Op::SetTable(key(RECEIPTS), next),
        ],
        Identity {
            row,
            generation: Some(generation),
        },
    ))
}

pub fn set_read_owned(
    snapshot: &nfs_storage::Snapshot,
    capacity: u16,
    expected: &Identity,
    read: bool,
) -> Result<Option<nfs_storage::Batch>, Error> {
    let table = snapshot.tables.get(&key(TABLE));
    let current = identities(table, snapshot.tables.get(&key(RECEIPTS)), capacity)?;
    if !current
        .iter()
        .any(|r| r.row.index == expected.row.index && r.generation == expected.generation)
    {
        return Ok(None);
    }
    set_read(table, capacity, &expected.row, read)?
        .map(|table| super::batch(snapshot, TABLE, table))
        .transpose()
}

struct Ring {
    next: u64,
    rows: Vec<Row>,
}

/// Retained messages in oldest-to-newest ring order, not numeric slot order.
/// Empty data is preserved for existing rows, never interpreted as an attachment.
pub fn rows(table: Option<&Table>, capacity: u16) -> Result<Vec<Row>, Error> {
    Ok(load(table, capacity)?.rows)
}

fn load(table: Option<&Table>, capacity: u16) -> Result<Ring, Error> {
    if !(2..=MAX_CAPACITY).contains(&capacity) {
        return Err(Error::Bounds);
    }
    let Some(table) = table.filter(|t| !t.rows.is_empty()) else {
        return Ok(Ring {
            next: 0,
            rows: vec![],
        });
    };
    table.validate()?;
    if table.rows.len() > usize::from(capacity) {
        return Err(Error::Bounds);
    }
    let mut next = None;
    let mut rows = vec![];
    for (&index, cells) in &table.rows {
        if index >= u64::from(capacity) {
            return Err(Error::Bounds);
        }
        let Some(Value::Int(hash)) = cells.get(&key("Asset")) else {
            return Err(Error::Invalid);
        };
        if *hash == 0 {
            if next.replace(index).is_some() {
                return Err(Error::Invalid);
            }
            // The cursor overwrites only Asset; old Read/Data cells are inert.
            continue;
        }
        let read = match cells.get(&key("Read")) {
            Some(Value::Int(0)) => false,
            Some(Value::Int(1)) => true,
            _ => return Err(Error::Invalid),
        };
        let Some(Value::String(data)) = cells.get(&key("Data")) else {
            return Err(Error::Invalid);
        };
        rows.push(Row {
            index,
            hash: *hash as u32,
            read,
            data: decode(data)?,
        });
    }
    let next = next.ok_or(Error::Invalid)?;
    rows.sort_by_key(|row| (row.index + u64::from(capacity) - next - 1) % u64::from(capacity));
    // Before the first wrap, slots beyond the cursor do not exist. After wrap,
    // every slot exists. Holes must not silently change chronology/eviction.
    if table.rows.len() < usize::from(capacity)
        && (next != rows.len() as u64 || table.rows.keys().copied().ne(0..=next))
    {
        return Err(Error::Invalid);
    }
    Ok(Ring { next, rows })
}

/// Append one delivery, retaining repeated hashes with distinct attachments.
/// The caller supplies the installed ring capacity and transaction retry key;
/// this pure planner deliberately performs no hash-based deduplication.
pub fn append(
    table: Option<&Table>,
    capacity: u16,
    hash: u32,
    data: &[u8],
) -> Result<(Table, Row), Error> {
    let ring = load(table, capacity)?;
    if hash == 0 || data.is_empty() {
        return Err(Error::Invalid);
    }
    let encoded = encode(data)?;
    let row = Row {
        index: ring.next,
        hash,
        read: false,
        data: data.to_vec(),
    };
    let mut next = table.cloned().unwrap_or_default();
    let cells = next.rows.entry(row.index).or_default();
    cells.insert(key("Asset"), Value::Int(hash as i32));
    cells.insert(key("Read"), Value::Int(0));
    cells.insert(key("Data"), Value::String(encoded));
    let cursor = (row.index + 1) % u64::from(capacity);
    next.rows
        .entry(cursor)
        .or_default()
        .insert(key("Asset"), Value::Int(0));
    next.validate()?;
    Ok((next, row))
}

/// Update the row selected by the manager's current delivery association.
/// A removed/replaced row is inert. The caller must resolve the live message
/// identity first; choosing the latest row with the same hash is insufficient.
pub fn set_read(
    table: Option<&Table>,
    capacity: u16,
    expected: &Row,
    read: bool,
) -> Result<Option<Table>, Error> {
    let ring = load(table, capacity)?;
    let Some(row) = ring
        .rows
        .iter()
        .find(|r| r.index == expected.index && r.hash == expected.hash && r.data == expected.data)
    else {
        return Ok(None);
    };
    if row.read == read {
        return Ok(None);
    }
    let mut next = table.ok_or(Error::Invalid)?.clone();
    next.rows
        .get_mut(&row.index)
        .ok_or(Error::Invalid)?
        .insert(key("Read"), Value::Int(i32::from(read)));
    next.validate()?;
    Ok(Some(next))
}

fn encode(data: &[u8]) -> Result<String, Error> {
    if data.len() > MAX_DATA {
        return Err(Error::Bounds);
    }
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for part in data.chunks(3) {
        let value = u32::from(part[0]) << 16
            | u32::from(*part.get(1).unwrap_or(&0)) << 8
            | u32::from(*part.get(2).unwrap_or(&0));
        out.push(ALPHABET[(value >> 18) as usize] as char);
        out.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        out.push(if part.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if part.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    Ok(out)
}
fn decode(text: &str) -> Result<Vec<u8>, Error> {
    if text.len() > MAX_DATA.div_ceil(3) * 4 || !text.len().is_multiple_of(4) {
        return Err(Error::Invalid);
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for (index, group) in text.as_bytes().as_chunks::<4>().0.iter().enumerate() {
        let last = (index + 1) * 4 == text.len();
        let mut value = 0u32;
        let mut padding = 0;
        for (i, &byte) in group.iter().enumerate() {
            value <<= 6;
            if byte == b'=' {
                if i < 2 || !last {
                    return Err(Error::Invalid);
                }
                padding += 1;
            } else {
                if padding != 0 {
                    return Err(Error::Invalid);
                }
                value |= ALPHABET
                    .iter()
                    .position(|&c| c == byte)
                    .ok_or(Error::Invalid)? as u32;
            }
        }
        out.push((value >> 16) as u8);
        if padding < 2 {
            out.push((value >> 8) as u8);
        }
        if padding == 0 {
            out.push(value as u8);
        }
    }
    if encode(&out)? != text {
        return Err(Error::Invalid);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
