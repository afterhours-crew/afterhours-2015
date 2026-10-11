// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Canonical encoding of the observed startup scalar/container shapes.
//! Unknown validated fields can be copied exactly. A failed writer stays failed.
use crate::{Error, ErrorKind, Field, Kind, Limits};

pub struct Encoder {
    bytes: Vec<u8>,
    limits: Limits,
    depth: usize,
    values: usize,
    failed: Option<Error>,
}

impl Encoder {
    pub fn new(limits: Limits) -> Self {
        Self {
            bytes: Vec::new(),
            limits,
            depth: 0,
            values: 0,
            failed: None,
        }
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }
    fn error(&self, kind: ErrorKind) -> Error {
        Error {
            offset: self.bytes.len(),
            kind,
        }
    }
    fn guard(&mut self, action: impl FnOnce(&mut Self) -> Result<(), Error>) -> Result<(), Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let result = action(self);
        if let Err(error) = result {
            self.failed = Some(error);
        }
        result
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > self.limits.max_bytes.saturating_sub(self.bytes.len()) {
            return Err(self.error(ErrorKind::InputLimit));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| self.error(ErrorKind::AllocationFailed))?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn value(&mut self) -> Result<(), Error> {
        if self.depth > self.limits.max_depth.min(32) {
            return Err(self.error(ErrorKind::DepthLimit));
        }
        if self.values >= self.limits.max_values {
            return Err(self.error(ErrorKind::ValueLimit));
        }
        self.values += 1;
        Ok(())
    }
    fn header(&mut self, tag: [u8; 3], kind: Kind) -> Result<(), Error> {
        if tag[0] == 0 {
            return Err(self.error(ErrorKind::InvalidTag));
        }
        self.value()?;
        self.append(&[tag[0], tag[1], tag[2], kind as u8])
    }
    fn integer_bytes(&mut self, value: i64) -> Result<(), Error> {
        if value == i64::MIN {
            return self.append(&[0x40]);
        }
        let mut magnitude = value.unsigned_abs();
        let mut byte = (magnitude & 0x3f) as u8 | if value < 0 { 0x40 } else { 0 };
        magnitude >>= 6;
        loop {
            self.append(&[byte | if magnitude != 0 { 0x80 } else { 0 }])?;
            if magnitude == 0 {
                return Ok(());
            }
            byte = (magnitude & 0x7f) as u8;
            magnitude >>= 7;
        }
    }
    fn length(&mut self, length: usize) -> Result<(), Error> {
        self.integer_bytes(
            i64::try_from(length).map_err(|_| self.error(ErrorKind::IntegerOverflow))?,
        )
    }
    fn string_bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let length = bytes
            .len()
            .checked_add(1)
            .ok_or_else(|| self.error(ErrorKind::ByteStringLimit))?;
        if length > self.limits.max_byte_string {
            return Err(self.error(ErrorKind::ByteStringLimit));
        }
        self.length(length)?;
        self.append(bytes)?;
        self.append(&[0])
    }
    fn collection(&mut self, count: usize, multiplier: usize) -> Result<(), Error> {
        if count > self.limits.max_collection {
            return Err(self.error(ErrorKind::CollectionLimit));
        }
        if count > self.limits.max_values.saturating_sub(self.values) / multiplier {
            return Err(self.error(ErrorKind::ValueLimit));
        }
        self.length(count)
    }
    pub fn integer(&mut self, tag: [u8; 3], value: i64) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Integer)?;
            w.integer_bytes(value)
        })
    }
    /// tagged type 8: two consecutive integers, counted as one value.
    /// Native field widths belong to typed adapters (ObjectType).
    pub fn integer_pair(&mut self, tag: [u8; 3], values: [i64; 2]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::IntegerPair)?;
            for value in values {
                w.integer_bytes(value)?;
            }
            Ok(())
        })
    }
    /// tagged type 9: three consecutive integers, counted as one value.
    /// Native field widths and object identity semantics belong to typed adapters.
    pub fn integer_triple(&mut self, tag: [u8; 3], values: [i64; 3]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::IntegerTriple)?;
            for value in values {
                w.integer_bytes(value)?;
            }
            Ok(())
        })
    }
    /// The observed unset tagged union selector, with no selected member.
    pub fn unset_union(&mut self, tag: [u8; 3]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Union)?;
            w.append(&[0x7f])
        })
    }
    /// A tagged union with one tagged struct member. Selector meanings
    /// and the member tag belong to the typed adapter; 127 denotes no member.
    pub fn struct_union(
        &mut self,
        tag: [u8; 3],
        selector: u8,
        member_tag: [u8; 3],
        fields: impl FnOnce(&mut Self) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            if selector == 127 {
                return Err(w.error(ErrorKind::InvalidSelector));
            }
            w.header(tag, Kind::Union)?;
            w.append(&[selector])?;
            w.depth += 1;
            let result = w.structure(member_tag, fields);
            w.depth -= 1;
            result
        })
    }
    /// The observed absent tagged dynamic value, with no payload.
    pub fn absent_variable(&mut self, tag: [u8; 3]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Variable)?;
            w.append(&[0])
        })
    }
    pub fn string(&mut self, tag: [u8; 3], value: &[u8]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::String)?;
            w.string_bytes(value)
        })
    }
    /// Opaque bytes with a length prefix and no string terminator.
    pub fn blob(&mut self, tag: [u8; 3], value: &[u8]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Blob)?;
            if value.len() > w.limits.max_byte_string {
                return Err(w.error(ErrorKind::ByteStringLimit));
            }
            w.length(value.len())?;
            w.append(value)
        })
    }
    pub fn structure(
        &mut self,
        tag: [u8; 3],
        fields: impl FnOnce(&mut Self) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Struct)?;
            w.depth += 1;
            let result = fields(w);
            w.depth -= 1;
            result?;
            w.append(&[0])
        })
    }
    pub fn integer_list(
        &mut self,
        tag: [u8; 3],
        values: impl ExactSizeIterator<Item = i64>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::List)?;
            w.append(&[Kind::Integer as u8])?;
            let count = values.len();
            w.collection(count, 1)?;
            w.depth += 1;
            let result = (|| {
                let mut written = 0;
                for value in values {
                    if written >= count {
                        return Err(w.error(ErrorKind::CollectionCountMismatch));
                    }
                    w.value()?;
                    w.integer_bytes(value)?;
                    written += 1;
                }
                if written != count {
                    return Err(w.error(ErrorKind::CollectionCountMismatch));
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// GNLS: ordered bare strings, each with a length and NUL terminator.
    pub fn string_list<'a>(
        &mut self,
        tag: [u8; 3],
        values: impl ExactSizeIterator<Item = &'a [u8]>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::List)?;
            w.append(&[Kind::String as u8])?;
            let count = values.len();
            w.collection(count, 1)?;
            w.depth += 1;
            let result = (|| {
                let mut written = 0;
                for value in values {
                    if written >= count {
                        return Err(w.error(ErrorKind::CollectionCountMismatch));
                    }
                    w.value()?;
                    w.string_bytes(value)?;
                    written += 1;
                }
                if written != count {
                    return Err(w.error(ErrorKind::CollectionCountMismatch));
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    pub fn string_map(&mut self, tag: [u8; 3], entries: &[(&[u8], &[u8])]) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[1, 1])?;
            w.collection(entries.len(), 2)?;
            w.depth += 1;
            let result = (|| {
                for (key, value) in entries {
                    w.value()?;
                    w.string_bytes(key)?;
                    w.value()?;
                    w.string_bytes(value)?;
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// String keys are arbitrary bytes, including their wire NUL terminator.
    /// Values retain signed integer patterns; adapters constrain native widths.
    pub fn string_integer_map<'a>(
        &mut self,
        tag: [u8; 3],
        entries: impl ExactSizeIterator<Item = (&'a [u8], i64)>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[Kind::String as u8, Kind::Integer as u8])?;
            let count = entries.len();
            w.collection(count, 2)?;
            w.depth += 1;
            let result = (|| {
                let mut written = 0;
                for (key, value) in entries {
                    if written >= count {
                        return Err(w.error(ErrorKind::CollectionCountMismatch));
                    }
                    w.value()?;
                    w.string_bytes(key)?;
                    w.value()?;
                    w.integer_bytes(value)?;
                    written += 1;
                }
                if written != count {
                    return Err(w.error(ErrorKind::CollectionCountMismatch));
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// Preserve IEEE-754 binary32 bits exactly, including NaNs and signed zero.
    /// settingsFlt map uses string keys and four network-order bytes.
    pub fn string_float_bits_map<'a>(
        &mut self,
        tag: [u8; 3],
        entries: impl ExactSizeIterator<Item = (&'a [u8], u32)>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[Kind::String as u8, Kind::FloatBits as u8])?;
            let count = entries.len();
            w.collection(count, 2)?;
            w.depth += 1;
            let result = (|| {
                let mut written = 0;
                for (key, bits) in entries {
                    if written >= count {
                        return Err(w.error(ErrorKind::CollectionCountMismatch));
                    }
                    w.value()?;
                    w.string_bytes(key)?;
                    w.value()?;
                    w.append(&bits.to_be_bytes())?;
                    written += 1;
                }
                if written != count {
                    return Err(w.error(ErrorKind::CollectionCountMismatch));
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// Integer map order is retained; typed adapters enforce key uniqueness.
    pub fn integer_map(
        &mut self,
        tag: [u8; 3],
        entries: impl ExactSizeIterator<Item = (i64, i64)>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[Kind::Integer as u8, Kind::Integer as u8])?;
            let count = entries.len();
            w.collection(count, 2)?;
            w.depth += 1;
            let result = (|| {
                let mut written = 0;
                for (key, value) in entries {
                    if written >= count {
                        return Err(w.error(ErrorKind::CollectionCountMismatch));
                    }
                    w.value()?;
                    w.integer_bytes(key)?;
                    w.value()?;
                    w.integer_bytes(value)?;
                    written += 1;
                }
                if written != count {
                    return Err(w.error(ErrorKind::CollectionCountMismatch));
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// ObjectId-list shape: each bare triple is one list value.
    pub fn integer_triple_list(
        &mut self,
        tag: [u8; 3],
        values: impl ExactSizeIterator<Item = [i64; 3]>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::List)?;
            w.append(&[Kind::IntegerTriple as u8])?;
            let count = values.len();
            w.collection(count, 1)?;
            w.depth += 1;
            let result = (|| {
                let mut written = 0;
                for triple in values {
                    if written >= count {
                        return Err(w.error(ErrorKind::CollectionCountMismatch));
                    }
                    w.value()?;
                    for value in triple {
                        w.integer_bytes(value)?;
                    }
                    written += 1;
                }
                if written != count {
                    return Err(w.error(ErrorKind::CollectionCountMismatch));
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// list of bare structs. Collection byte 3 denotes Struct here;
    /// each element has tagged fields and its own zero terminator.
    pub fn struct_list<T>(
        &mut self,
        tag: [u8; 3],
        values: &[T],
        mut fields: impl FnMut(&mut Self, &T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::List)?;
            w.append(&[3])?;
            w.collection(values.len(), 1)?;
            w.depth += 1;
            let result = (|| {
                for value in values {
                    w.value()?;
                    w.depth += 1;
                    let result = fields(w, value);
                    w.depth -= 1;
                    result?;
                    w.append(&[0])?;
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// The IEEE-754 representation is retained, including NaN payload bits.
    pub fn float_bits(&mut self, tag: [u8; 3], bits: u32) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::FloatBits)?;
            w.append(&bits.to_be_bytes())
        })
    }
    /// : list header 3 with bare union elements. A selected struct
    /// has no member tag here; None emits the explicit unset selector 127.
    pub fn struct_union_list<T>(
        &mut self,
        tag: [u8; 3],
        values: &[T],
        mut selector: impl FnMut(&T) -> Option<u8>,
        mut fields: impl FnMut(&mut Self, &T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::List)?;
            w.append(&[3])?;
            w.collection(values.len(), 1)?;
            w.depth += 1;
            let result = (|| {
                for value in values {
                    w.value()?;
                    if let Some(selected) = selector(value) {
                        if selected == 127 {
                            return Err(w.error(ErrorKind::InvalidSelector));
                        }
                        w.append(&[selected])?;
                        w.depth += 1;
                        w.value()?;
                        w.depth += 1;
                        let result = fields(w, value);
                        w.depth -= 2;
                        result?;
                        w.append(&[0])?;
                    } else {
                        w.append(&[127])?;
                    }
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    pub fn string_struct_map<T>(
        &mut self,
        tag: [u8; 3],
        entries: &[(&[u8], T)],
        mut fields: impl FnMut(&mut Self, &T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[1, 3])?;
            w.collection(entries.len(), 2)?;
            w.depth += 1;
            let result = (|| {
                for (key, value) in entries {
                    w.value()?;
                    w.string_bytes(key)?;
                    w.value()?;
                    w.depth += 1;
                    let result = fields(w, value);
                    w.depth -= 1;
                    result?;
                    w.append(&[0])?;
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// Integer keys with bare struct values, observed in the SpeedList catalog.
    pub fn integer_struct_map<T>(
        &mut self,
        tag: [u8; 3],
        entries: &[(u32, T)],
        mut fields: impl FnMut(&mut Self, &T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[0, 3])?;
            w.collection(entries.len(), 2)?;
            w.depth += 1;
            let result = (|| {
                for (key, value) in entries {
                    w.value()?;
                    w.integer_bytes(i64::from(*key))?;
                    w.value()?;
                    w.depth += 1;
                    let result = fields(w, value);
                    w.depth -= 1;
                    result?;
                    w.append(&[0])?;
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// Integer keys mapped to lists of bare structs. Both the map's list value
    /// and each list's struct element use collection header 3. The caller must
    /// finish with the corresponding schema to disambiguate the nested layout.
    pub fn integer_struct_list_map<T>(
        &mut self,
        tag: [u8; 3],
        entries: &[(u32, Vec<T>)],
        mut fields: impl FnMut(&mut Self, &T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.guard(|w| {
            w.header(tag, Kind::Map)?;
            w.append(&[0, 3])?;
            w.collection(entries.len(), 2)?;
            w.depth += 1;
            let result = (|| {
                for (key, values) in entries {
                    w.value()?;
                    w.integer_bytes(i64::from(*key))?;
                    w.value()?;
                    w.append(&[3])?;
                    w.collection(values.len(), 1)?;
                    w.depth += 1;
                    let result = (|| {
                        for value in values {
                            w.value()?;
                            w.depth += 1;
                            let result = fields(w, value);
                            w.depth -= 1;
                            result?;
                            w.append(&[0])?;
                        }
                        Ok(())
                    })();
                    w.depth -= 1;
                    result?;
                }
                Ok(())
            })();
            w.depth -= 1;
            result
        })
    }
    /// Retain an unknown field exactly, but recheck it under this writer's limits.
    pub fn raw_field(&mut self, field: Field<'_>) -> Result<(), Error> {
        self.guard(|w| {
            let remaining = Limits {
                max_depth: w.limits.max_depth.min(32).saturating_sub(w.depth),
                max_values: w.limits.max_values.saturating_sub(w.values),
                ..w.limits
            };
            if w.depth > w.limits.max_depth.min(32) {
                return Err(w.error(ErrorKind::DepthLimit));
            }
            w.values += field.validate(remaining)?.values;
            w.append(field.as_bytes())
        })
    }
    /// No partial output is exposed after any error, even if a caller ignored it.
    pub fn finish(self) -> Result<Vec<u8>, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        crate::decode(&self.bytes, self.limits)?;
        Ok(self.bytes)
    }
    /// Validate output against a supplied root, including copied schema-backed
    /// fields. This does not synthesize responses or assert a route binding.
    pub fn finish_with_schema(
        self,
        schema: crate::Schema<'_>,
        root: crate::TypeId,
    ) -> Result<Vec<u8>, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        crate::decode_with_schema(&self.bytes, self.limits, schema, root)?;
        Ok(self.bytes)
    }
}
