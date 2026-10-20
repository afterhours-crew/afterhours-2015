// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Localized strings of a `UITextDatabase`, keyed by a hash of their string
//! ID. The layouts were recovered from the supported installation's text
//! databases (`localization/textdbs/<locale>`).
//!
//! The binary chunk is `magic 0x39000, body length` and a body whose offsets
//! are relative to the body: `count, table offset, text offset`, the
//! database name, `count` pairs of (key, text offset) sorted by key, and
//! NUL-terminated byte strings. The histogram chunk is `magic 0x39001, body
//! length, count` followed by `count` UTF-16 code units that give each byte
//! its character: a byte below 0x80 with a zero entry is itself. A table of
//! 256 entries is a single-byte encoding. Entries below 0x20 mark multi-byte
//! sequences, which the Japanese and Chinese databases use and this reader
//! does not decode: their strings are reported as undecodable, never guessed.
use crate::{Error, Limits};

pub const BINARY_MAGIC: u32 = 0x39000;
pub const HISTOGRAM_MAGIC: u32 = 0x39001;
/// Longest string a lookup decodes.
pub const MAX_STRING_BYTES: usize = 64 * 1024;
const SINGLE_BYTE_ENTRIES: u32 = 256;

/// The key of a string ID: `h = h * 33 + byte` from `0xFFFF_FFFF`, over the
/// ID exactly as written (IDs are case-sensitive).
pub fn string_id_hash(id: &str) -> u32 {
    id.bytes().fold(u32::MAX, |hash, byte| {
        hash.wrapping_mul(33).wrapping_add(u32::from(byte))
    })
}

/// The outcome of one lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Lookup {
    Found(String),
    /// No string has this ID's key.
    Missing,
    /// The string uses bytes this reader does not decode.
    Undecodable,
}

/// A parsed text database.
#[derive(Clone, Debug)]
pub struct TextDatabase {
    body: Vec<u8>,
    name: String,
    keys: Vec<(u32, u32)>,
    text: usize,
    /// Code point of each byte; `None` for a multi-byte histogram.
    table: Option<[u16; 256]>,
}

fn word(data: &[u8], at: usize) -> Result<u32, Error> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
        .ok_or(Error::Malformed("text database truncated"))
}

fn index(value: u32) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::Bound("text database offset"))
}

/// The body of a chunk after its `magic, body length` header.
fn body(data: &[u8], magic: u32) -> Result<&[u8], Error> {
    if word(data, 0)? != magic {
        return Err(Error::Malformed("text database magic"));
    }
    if index(word(data, 4)?)? != data.len() - 8 {
        return Err(Error::Malformed("text database length"));
    }
    Ok(&data[8..])
}

impl TextDatabase {
    pub fn parse(binary: &[u8], histogram: &[u8], limits: &Limits) -> Result<Self, Error> {
        if binary.len() > limits.max_asset_bytes || histogram.len() > limits.max_asset_bytes {
            return Err(Error::Bound("text database size"));
        }
        let body = body(binary, BINARY_MAGIC)?;
        let count = index(word(body, 0)?)?;
        let table_at = index(word(body, 4)?)?;
        let text = index(word(body, 8)?)?;
        if count > limits.max_elements {
            return Err(Error::Bound("text database strings"));
        }
        let table_end = count
            .checked_mul(8)
            .and_then(|bytes| table_at.checked_add(bytes))
            .ok_or(Error::Bound("text database table"))?;
        if table_at < 12 || table_end != text || text > body.len() {
            return Err(Error::Malformed("text database layout"));
        }
        let name = &body[12..table_at];
        let name = String::from_utf8_lossy(
            &name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())],
        )
        .into_owned();
        let region = body.len() - text;
        let mut keys = Vec::with_capacity(count);
        for i in 0..count {
            let key = word(body, table_at + 8 * i)?;
            let at = word(body, table_at + 8 * i + 4)?;
            if index(at)? >= region {
                return Err(Error::Malformed("text database string offset"));
            }
            if keys.last().is_some_and(|(last, _)| *last >= key) {
                return Err(Error::Malformed("text database keys unsorted"));
            }
            keys.push((key, at));
        }
        let histogram = self::body(histogram, HISTOGRAM_MAGIC)?;
        let entries = word(histogram, 0)?;
        let table = if entries == SINGLE_BYTE_ENTRIES {
            let bytes = histogram
                .get(4..4 + 512)
                .ok_or(Error::Malformed("histogram truncated"))?;
            let mut table = [0u16; 256];
            for (slot, pair) in table.iter_mut().zip(bytes.as_chunks::<2>().0) {
                *slot = u16::from_le_bytes(*pair);
            }
            Some(table)
        } else {
            let needed = index(entries)?
                .checked_mul(2)
                .and_then(|bytes| bytes.checked_add(4))
                .ok_or(Error::Bound("histogram entries"))?;
            if needed > histogram.len() {
                return Err(Error::Malformed("histogram truncated"));
            }
            None
        };
        Ok(Self {
            body: body.to_vec(),
            name,
            keys,
            text,
            table,
        })
    }

    /// The database's own name, such as `Global`.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether strings use the single-byte encoding this reader decodes.
    pub fn is_single_byte(&self) -> bool {
        self.table.is_some()
    }

    pub fn get(&self, id: &str) -> Lookup {
        self.get_key(string_id_hash(id))
    }

    pub fn get_key(&self, key: u32) -> Lookup {
        let Ok(found) = self.keys.binary_search_by_key(&key, |(k, _)| *k) else {
            return Lookup::Missing;
        };
        let Some(table) = &self.table else {
            return Lookup::Undecodable;
        };
        let start = self.text + self.keys[found].1 as usize;
        let tail = &self.body[start..self.body.len().min(start + MAX_STRING_BYTES + 1)];
        let Some(end) = tail.iter().position(|b| *b == 0) else {
            return Lookup::Undecodable;
        };
        let mut out = String::with_capacity(end);
        for &byte in &tail[..end] {
            let entry = table[usize::from(byte)];
            let unit = if entry == 0 && byte < 0x80 {
                u16::from(byte)
            } else {
                entry
            };
            let control = unit < 0x20 && !matches!(unit, 0x09 | 0x0A | 0x0D);
            match char::from_u32(u32::from(unit)) {
                Some(c) if !control => out.push(c),
                _ => return Lookup::Undecodable,
            }
        }
        Lookup::Found(out)
    }
}

#[cfg(test)]
mod tests;
