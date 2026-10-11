// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! DbObject containers: the tagged binary format of `layout.toc`, superbundle
//! `.toc` files, `.sb` bundle manifests and `initfs`.
//!
//! A record is a tag byte (type in the low five bits, bit 7 set when the record
//! is unnamed), a NUL-terminated name for named records and a typed payload.
//! Lists and objects carry a 7-bit-encoded byte length and end with a zero tag.
//! `.toc` files may start with a 0x22C-byte header (`00 D1 CE 0x`) whose 260-byte
//! key at 0x128 XORs the body for variants 00 and 01.
use crate::{Error, Limits};
use std::borrow::Cow;

const HEADER_BYTES: usize = 0x22C;
const KEY_AT: usize = 0x128;
const KEY_BYTES: usize = 260;
const KEY_MODULUS: usize = 257;

/// One decoded DbObject value. Object members keep their stored order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
    Bool(bool),
    String(String),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Guid([u8; 16]),
    Sha1([u8; 20]),
    Blob(Vec<u8>),
}

impl Value {
    /// First member with this name, when the value is an object.
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Self::Object(members) => members.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
    /// Integer value of an `Int` or `Long`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(i64::from(*value)),
            Self::Long(value) => Some(*value),
            _ => None,
        }
    }
    pub fn as_sha1(&self) -> Option<[u8; 20]> {
        match self {
            Self::Sha1(value) => Some(*value),
            _ => None,
        }
    }
}

/// Header variant of an obfuscated container, if any.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Obfuscation {
    None,
    /// `00 D1 CE 03`: body stored in clear after the header.
    Clear,
    /// `00 D1 CE 00` or `00 D1 CE 01`: body XORed with the header key.
    Keyed,
}

/// Strip a `00 D1 CE 0x` header and undo its XOR, if present.
pub fn deobfuscate(data: &[u8]) -> Result<(Cow<'_, [u8]>, Obfuscation), Error> {
    let variant = match data.get(..4) {
        Some([0x00, 0xD1, 0xCE, v @ (0x00 | 0x01 | 0x03)]) => *v,
        _ => return Ok((Cow::Borrowed(data), Obfuscation::None)),
    };
    if data.len() < HEADER_BYTES {
        return Err(Error::Malformed("obfuscation header truncated"));
    }
    let body = &data[HEADER_BYTES..];
    if variant == 0x03 {
        return Ok((Cow::Borrowed(body), Obfuscation::Clear));
    }
    let mask = if variant == 0x01 { 0x7B } else { 0x00 };
    let key: Vec<u8> = data[KEY_AT..KEY_AT + KEY_BYTES]
        .iter()
        .map(|k| k ^ mask)
        .collect();
    let clear = body
        .iter()
        .enumerate()
        .map(|(i, byte)| byte ^ key[i % KEY_MODULUS])
        .collect();
    Ok((Cow::Owned(clear), Obfuscation::Keyed))
}

/// Parse a whole container (after [`deobfuscate`]): returns its root value and
/// the bytes consumed. An empty root (a leading zero tag) yields `None`.
pub fn parse(data: &[u8], limits: &Limits) -> Result<(Option<Value>, usize), Error> {
    if data.len() > limits.max_container_bytes {
        return Err(Error::Bound("DbObject container size"));
    }
    let mut reader = Reader {
        data,
        pos: 0,
        limits,
    };
    let root = reader.record(0)?.map(|(_, value)| value);
    Ok((root, reader.pos))
}

/// [`deobfuscate`] then [`parse`], requiring a root value.
pub fn load(data: &[u8], limits: &Limits) -> Result<Value, Error> {
    let (body, _) = deobfuscate(data)?;
    parse(&body, limits)?
        .0
        .ok_or(Error::Malformed("empty DbObject container"))
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    limits: &'a Limits,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, Error> {
        let byte = *self
            .data
            .get(self.pos)
            .ok_or(Error::Malformed("DbObject truncated"))?;
        self.pos += 1;
        Ok(byte)
    }
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or(Error::Malformed("DbObject truncated"))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    fn leb(&mut self) -> Result<usize, Error> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return usize::try_from(value).map_err(|_| Error::Bound("DbObject length"));
            }
            shift += 7;
            if shift > 63 {
                return Err(Error::Malformed("DbObject length encoding too long"));
            }
        }
    }
    fn name(&mut self) -> Result<String, Error> {
        let rest = &self.data[self.pos..];
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(Error::Malformed("unterminated DbObject name"))?;
        // Names are ASCII in practice; Latin-1 keeps every byte representable.
        let name = rest[..end].iter().map(|b| char::from(*b)).collect();
        self.pos += end + 1;
        Ok(name)
    }
    fn record(&mut self, depth: usize) -> Result<Option<(String, Value)>, Error> {
        let tag = self.u8()?;
        let kind = tag & 0x1F;
        if kind == 0 {
            return Ok(None);
        }
        let name = if tag & 0x80 != 0 {
            String::new()
        } else {
            self.name()?
        };
        Ok(Some((name, self.value(kind, depth)?)))
    }
    fn value(&mut self, kind: u8, depth: usize) -> Result<Value, Error> {
        if depth > self.limits.max_depth {
            return Err(Error::Bound("DbObject nesting depth"));
        }
        Ok(match kind {
            1 | 2 => {
                let size = self.leb()?;
                let end = self
                    .pos
                    .checked_add(size)
                    .filter(|end| *end <= self.data.len())
                    .ok_or(Error::Malformed("DbObject container exceeds data"))?;
                let mut members = Vec::new();
                while self.pos < end {
                    let Some(member) = self.record(depth + 1)? else {
                        break;
                    };
                    if members.len() == self.limits.max_elements {
                        return Err(Error::Bound("DbObject container elements"));
                    }
                    members.push(member);
                }
                if self.pos != end {
                    return Err(Error::Malformed("DbObject container end mismatch"));
                }
                if kind == 1 {
                    Value::List(members.into_iter().map(|(_, v)| v).collect())
                } else {
                    Value::Object(members)
                }
            }
            6 => Value::Bool(self.u8()? == 1),
            7 => {
                let size = self.leb()?;
                let bytes = self.take(size)?;
                let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
                Value::String(String::from_utf8_lossy(&bytes[..end]).into_owned())
            }
            8 => Value::Int(i32::from_le_bytes(self.array()?)),
            9 => Value::Long(i64::from_le_bytes(self.array()?)),
            11 => Value::Float(f32::from_le_bytes(self.array()?)),
            12 => Value::Double(f64::from_le_bytes(self.array()?)),
            15 => Value::Guid(self.array()?),
            16 => Value::Sha1(self.array()?),
            19 => {
                let size = self.leb()?;
                Value::Blob(self.take(size)?.to_vec())
            }
            _ => return Err(Error::Unsupported("DbObject value type")),
        })
    }
}

#[cfg(test)]
mod tests;
