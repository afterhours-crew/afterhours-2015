// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bit-oriented world codecs and bounded application history primitives.
//! These do not share the backend's Heat2 schema.
//! Only independently bounded wire portions belong here; object lifetimes and
//! asset-dependent serializer selection belong outside the byte codec.

pub mod admission;
pub mod challenge;
pub mod disconnect;
pub mod envelope;
pub mod fragment;
pub mod ghost;
pub mod history;
pub mod payload;
pub mod rpc;
pub mod timing;

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidBitRange,
    InvalidWidth,
    Truncated { offset: usize, width: u8 },
    InputLimit,
    RecordLimit,
    DeletionLimit,
    ReferenceLimit,
    PayloadLimit,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "world codec {self:?}")
    }
}
impl std::error::Error for Error {}

/// A validated MSB-first range within borrowed bytes. Bit zero is byte 0's MSB.
/// Debug formatting deliberately omits payload bytes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BitSpan<'a> {
    bytes: &'a [u8],
    start: usize,
    len: usize,
}

impl fmt::Debug for BitSpan<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BitSpan")
            .field("start", &self.start)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl<'a> BitSpan<'a> {
    pub fn new(bytes: &'a [u8], start: usize, len: usize) -> Result<Self, Error> {
        let end = start.checked_add(len).ok_or(Error::InvalidBitRange)?;
        // Avoid multiplying bytes.len by 8, which can overflow for large slices.
        if end / 8 > bytes.len() || (end / 8 == bytes.len() && !end.is_multiple_of(8)) {
            return Err(Error::InvalidBitRange);
        }
        Ok(Self { bytes, start, len })
    }

    /// The original backing bytes, including bytes outside this bit range.
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub fn start(self) -> usize {
        self.start
    }

    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Read 0..=32 bits at a relative offset without advancing or allocating.
    pub fn read_u32(self, offset: usize, width: u8) -> Result<u32, Error> {
        if width > 32 {
            return Err(Error::InvalidWidth);
        }
        if offset > self.len || usize::from(width) > self.len - offset {
            return Err(Error::Truncated { offset, width });
        }
        let mut value = 0;
        for bit in self.start + offset..self.start + offset + usize::from(width) {
            value = (value << 1) | u32::from((self.bytes[bit / 8] >> (7 - bit % 8)) & 1);
        }
        Ok(value)
    }

    /// Preserve the exact suffix, including any partial first/last bytes.
    pub fn after(self, offset: usize) -> Result<Self, Error> {
        if offset > self.len {
            return Err(Error::InvalidBitRange);
        }
        Ok(Self {
            bytes: self.bytes,
            start: self.start + offset,
            len: self.len - offset,
        })
    }

    /// Borrow a bounded subrange. Neither following fields nor byte padding can
    /// be read through the returned view, even when the byte slice includes them.
    pub fn slice(self, offset: usize, len: usize) -> Result<Self, Error> {
        let suffix = self.after(offset)?;
        if len > suffix.len {
            return Err(Error::InvalidBitRange);
        }
        Ok(Self { len, ..suffix })
    }
}
