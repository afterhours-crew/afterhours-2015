// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Data embedded in the executable image, located through its PE section table.
use crate::{BuildIdentity, Error, sha256_hex};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const HEADER_BYTES: u64 = 64 << 10;
const TEMPLATE_BYTES: usize = 64;

fn word16(head: &[u8], at: usize) -> Result<u16, Error> {
    head.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| Error::Content("executable header truncated".into()))
}
fn word32(head: &[u8], at: usize) -> Result<u32, Error> {
    head.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| Error::Content("executable header truncated".into()))
}

/// File offset of `len` bytes at `rva`, which must lie in a section's file data.
pub(crate) fn file_offset(head: &[u8], rva: u32, len: usize) -> Result<u64, Error> {
    if head.get(..2) != Some(b"MZ") {
        return Err(Error::Content("executable is not a PE image".into()));
    }
    let pe = word32(head, 0x3C)? as usize;
    if head.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(Error::Content("executable is not a PE image".into()));
    }
    let sections = usize::from(word16(head, pe + 6)?);
    let optional = usize::from(word16(head, pe + 20)?);
    let table = pe + 24 + optional;
    for i in 0..sections {
        let at = table + i * 40;
        let virtual_size = word32(head, at + 8)?;
        let virtual_address = word32(head, at + 12)?;
        let raw_size = word32(head, at + 16)?;
        let raw_offset = word32(head, at + 20)?;
        let end = u64::from(rva) + len as u64;
        if rva >= virtual_address
            && u64::from(rva) < u64::from(virtual_address) + u64::from(virtual_size)
        {
            if end > u64::from(virtual_address) + u64::from(raw_size) {
                return Err(Error::Content("embedded data has no file backing".into()));
            }
            return Ok(u64::from(raw_offset) + u64::from(rva - virtual_address));
        }
    }
    Err(Error::Content(
        "embedded data is outside every section".into(),
    ))
}

/// Read the world MAC template and check its digest.
pub fn mac_template(path: &Path, identity: &BuildIdentity) -> Result<Vec<u8>, Error> {
    let mut file = std::fs::File::open(path)?;
    let mut head = Vec::new();
    (&mut file).take(HEADER_BYTES).read_to_end(&mut head)?;
    let offset = file_offset(&head, identity.mac_template_rva, TEMPLATE_BYTES)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut template = vec![0u8; TEMPLATE_BYTES];
    file.read_exact(&mut template)?;
    if sha256_hex(&template) != identity.mac_template_sha256 {
        return Err(Error::Content("world MAC template digest differs".into()));
    }
    Ok(template)
}
