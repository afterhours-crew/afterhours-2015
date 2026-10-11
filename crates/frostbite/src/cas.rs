// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `cas.cat` catalogues and `cas_NN.cas` block records.
//!
//! A catalogue is `NyanNyanNyanNyan` followed by 32-byte entries: SHA-1, then
//! little-endian offset, size and archive index. A record is a sequence of
//! blocks: big-endian decompressed size (24 bits), codec byte, flags byte
//! (low nibble extends the size) and big-endian 16-bit compressed size.
//! Codec 0 is stored, 2 zlib and 9 an LZ4 block. A delta record rewrites the
//! blocks of a base record with a command stream (`casPatchType` 2).
use crate::{Error, Limits};
use std::collections::HashMap;

const CATALOG_MAGIC: &[u8; 16] = b"NyanNyanNyanNyan";
const MAX_BLOCK_BYTES: usize = 1 << 24;

/// Location of one record in an archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub offset: u32,
    pub size: u32,
    pub archive: u8,
}

/// Parse a catalogue. The first entry for a SHA-1 wins.
pub fn parse_catalog(data: &[u8], limits: &Limits) -> Result<HashMap<[u8; 20], Record>, Error> {
    if data.len() > limits.max_container_bytes {
        return Err(Error::Bound("catalogue size"));
    }
    let body = data
        .strip_prefix(CATALOG_MAGIC)
        .ok_or(Error::Malformed("catalogue magic"))?;
    let (rows, rest) = body.as_chunks::<32>();
    if !rest.is_empty() {
        return Err(Error::Malformed("catalogue entry size"));
    }
    let mut entries = HashMap::with_capacity(rows.len());
    for entry in rows {
        let mut sha1 = [0u8; 20];
        sha1.copy_from_slice(&entry[..20]);
        let word = |i: usize| u32::from_le_bytes(entry[20 + i * 4..24 + i * 4].try_into().unwrap());
        entries.entry(sha1).or_insert(Record {
            offset: word(0),
            size: word(1),
            archive: (word(2) & 0xFF) as u8,
        });
    }
    Ok(entries)
}

/// Decode one block at `pos`; returns the bytes, next position and codec.
pub fn read_block(raw: &[u8], pos: usize) -> Result<(Vec<u8>, usize, u8), Error> {
    let header = raw
        .get(pos..pos + 8)
        .ok_or(Error::Malformed("block header truncated"))?;
    let size = (u32::from_be_bytes(header[..4].try_into().unwrap()) & 0x00FF_FFFF) as usize;
    let codec = header[4] & 0x7F;
    let flags = header[5];
    let compressed =
        usize::from(u16::from_be_bytes([header[6], header[7]])) | (usize::from(flags & 0x0F) << 16);
    let end = pos + 8 + compressed;
    if compressed > MAX_BLOCK_BYTES || end > raw.len() {
        return Err(Error::Malformed("block body exceeds record"));
    }
    let body = &raw[pos + 8..end];
    let out = match codec {
        0 => body.to_vec(),
        2 => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(body, size)
            .map_err(|_| Error::Malformed("zlib block"))?,
        9 => lz4_flex::block::decompress(body, size).map_err(|_| Error::Malformed("LZ4 block"))?,
        _ => return Err(Error::Unsupported("block codec")),
    };
    if out.len() != size {
        return Err(Error::Malformed("block size mismatch"));
    }
    Ok((out, end, codec))
}

/// Concatenate every block of a record.
pub fn read_blocks(raw: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < raw.len() {
        let (block, next, _) = read_block(raw, pos)?;
        out.extend_from_slice(&block);
        pos = next;
        if out.len() > limits.max_asset_bytes {
            return Err(Error::Bound("decompressed asset size"));
        }
    }
    Ok(out)
}

fn be16(data: &[u8], at: usize) -> Result<usize, Error> {
    data.get(at..at + 2)
        .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
        .ok_or(Error::Malformed("delta command truncated"))
}

/// Python-style slice: indices clamp to the data and an inverted range is empty.
fn clamped(data: &[u8], start: usize, end: usize) -> &[u8] {
    let start = start.min(data.len());
    let end = end.min(data.len()).max(start);
    &data[start..end]
}

/// Apply a delta record to its base record.
///
/// Commands are big-endian words: type in the top nibble, count in the low 28
/// bits. 0 copies `count` base blocks; 1 merges one base block with `count`
/// (offset, skip, delta block) edits; 2 rewrites one base block with `count`
/// bytes of inline (offset, skip, add, bytes) edits to a stated size; 3 copies
/// `count` delta blocks; 4 skips `count` base blocks. Remaining base blocks are
/// appended.
pub fn read_patched(base: &[u8], delta: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let (mut bp, mut dp) = (0usize, 0usize);
    while dp < delta.len() {
        let word = delta
            .get(dp..dp + 4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
            .ok_or(Error::Malformed("delta command truncated"))?;
        dp += 4;
        let (kind, count) = (word >> 28, (word & 0x0FFF_FFFF) as usize);
        match kind {
            0 => {
                for _ in 0..count {
                    let (block, next, _) = read_block(base, bp)?;
                    out.extend_from_slice(&block);
                    bp = next;
                    bound(&out, limits)?;
                }
            }
            1 => {
                let (block, next, _) = read_block(base, bp)?;
                bp = next;
                let mut cursor = 0;
                for _ in 0..count {
                    let offset = be16(delta, dp)?;
                    let skip = be16(delta, dp + 2)?;
                    dp += 4;
                    out.extend_from_slice(clamped(&block, cursor, offset));
                    let (piece, next, _) = read_block(delta, dp)?;
                    dp = next;
                    out.extend_from_slice(&piece);
                    cursor = offset + skip;
                    bound(&out, limits)?;
                }
                out.extend_from_slice(clamped(&block, cursor, block.len()));
            }
            2 => {
                let (block, next, _) = read_block(base, bp)?;
                bp = next;
                let new_size = be16(delta, dp)? + 1;
                dp += 2;
                let start = dp;
                let mut cursor = 0;
                let mut buf = Vec::with_capacity(new_size);
                while dp - start < count {
                    let offset = be16(delta, dp)?;
                    let (skip, add) = match delta.get(dp + 2..dp + 4) {
                        Some(b) => (usize::from(b[0]), usize::from(b[1])),
                        None => return Err(Error::Malformed("delta command truncated")),
                    };
                    dp += 4;
                    buf.extend_from_slice(clamped(&block, cursor, offset));
                    cursor = offset + skip;
                    let bytes = delta
                        .get(dp..dp + add)
                        .ok_or(Error::Malformed("delta inline bytes truncated"))?;
                    buf.extend_from_slice(bytes);
                    dp += add;
                    if buf.len() > MAX_BLOCK_BYTES {
                        return Err(Error::Bound("delta block size"));
                    }
                }
                let remaining = new_size.saturating_sub(buf.len());
                buf.extend_from_slice(clamped(&block, cursor, cursor + remaining));
                out.extend_from_slice(&buf);
            }
            3 => {
                for _ in 0..count {
                    let (piece, next, _) = read_block(delta, dp)?;
                    dp = next;
                    out.extend_from_slice(&piece);
                    bound(&out, limits)?;
                }
            }
            4 => {
                for _ in 0..count {
                    let (_, next, _) = read_block(base, bp)?;
                    bp = next;
                }
            }
            _ => return Err(Error::Unsupported("delta command")),
        }
        bound(&out, limits)?;
    }
    while bp < base.len() {
        let (block, next, _) = read_block(base, bp)?;
        out.extend_from_slice(&block);
        bp = next;
        bound(&out, limits)?;
    }
    Ok(out)
}

fn bound(out: &[u8], limits: &Limits) -> Result<(), Error> {
    if out.len() > limits.max_asset_bytes {
        return Err(Error::Bound("patched asset size"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
