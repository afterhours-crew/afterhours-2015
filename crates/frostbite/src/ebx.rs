// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Self-describing EBX partitions (magic `CE D1 B2 0F`).
//!
//! A 64-byte header is followed by imports (partition GUID, instance GUID),
//! NUL-separated type and field names keyed by [`crate::name_hash`], 16-byte
//! field descriptors, 16-byte class descriptors, the instance table (padded to
//! 16 bytes), 12-byte array descriptors (padded), then strings, instance data
//! and array data. Exported instances are preceded by their 16-byte GUID; the
//! eight-byte object header before the first field is not stored.
use crate::{Error, Limits, name_hash};
use std::collections::HashMap;

const MAGIC: [u8; 4] = [0xCE, 0xD1, 0xB2, 0x0F];

/// Field type codes (`(descriptor type >> 4) & 0x1F`).
mod code {
    pub const INHERITED: u8 = 0x00;
    pub const STRUCT: u8 = 0x02;
    pub const POINTER: u8 = 0x03;
    pub const ARRAY: u8 = 0x04;
    pub const STRING: u8 = 0x06;
    pub const CSTRING: u8 = 0x07;
    pub const ENUM: u8 = 0x08;
    pub const FILE_REF: u8 = 0x09;
    pub const BOOLEAN: u8 = 0x0A;
    pub const INT8: u8 = 0x0B;
    pub const UINT8: u8 = 0x0C;
    pub const INT16: u8 = 0x0D;
    pub const UINT16: u8 = 0x0E;
    pub const INT32: u8 = 0x0F;
    pub const UINT32: u8 = 0x10;
    pub const UINT64: u8 = 0x11;
    pub const INT64: u8 = 0x12;
    pub const FLOAT32: u8 = 0x13;
    pub const FLOAT64: u8 = 0x14;
    pub const GUID: u8 = 0x15;
    pub const SHA1: u8 = 0x16;
    pub const RESOURCE_REF: u8 = 0x17;
    pub const TYPE_REF: u8 = 0x19;
    pub const BOXED_VALUE_REF: u8 = 0x1A;
}

/// A pointer field: null, another instance in this partition (by index), or
/// an import naming a partition GUID and an instance GUID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pointer {
    Null,
    Internal(usize),
    Import {
        partition: [u8; 16],
        instance: [u8; 16],
    },
}

/// A decoded field value. Numeric values keep their stored representation.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int8(i8),
    UInt8(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Float32(f32),
    Float64(f64),
    String(String),
    Enum {
        value: i32,
        name: Option<String>,
    },
    Guid([u8; 16]),
    Sha1([u8; 20]),
    ResourceRef(u64),
    TypeRef(String),
    BoxedValueRef([u8; 16]),
    Pointer(Pointer),
    Struct(Fields),
    Array(Vec<Value>),
    /// A type code this reader does not decode.
    Unsupported(u8),
}

impl Value {
    /// The exact 32-bit pattern of a 32-bit scalar (floats as their bits).
    pub fn bits32(&self) -> Option<u32> {
        match self {
            Self::Int32(v) => Some(*v as u32),
            Self::UInt32(v) => Some(*v),
            Self::Float32(v) => Some(v.to_bits()),
            Self::Enum { value, .. } => Some(*value as u32),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(v) | Self::TypeRef(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_enum_name(&self) -> Option<&str> {
        match self {
            Self::Enum { name, .. } => name.as_deref(),
            _ => None,
        }
    }
    pub fn as_fields(&self) -> Option<&Fields> {
        match self {
            Self::Struct(fields) => Some(fields),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }
    pub fn as_pointer(&self) -> Option<Pointer> {
        match self {
            Self::Pointer(pointer) => Some(*pointer),
            _ => None,
        }
    }
}

/// Named field values in declaration order, including inherited fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields(pub Vec<(String, Value)>);

impl Fields {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// One decoded instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub class: String,
    /// GUID of an exported instance.
    pub guid: Option<[u8; 16]>,
    pub fields: Fields,
}

#[derive(Clone, Debug)]
struct FieldDesc {
    name: String,
    code: u8,
    class_ref: usize,
    offset: usize,
}

#[derive(Clone, Debug)]
struct ClassDesc {
    name: String,
    first_field: usize,
    field_count: usize,
    alignment: usize,
    size: usize,
}

/// A parsed partition header; [`Partition::objects`] decodes instances.
#[derive(Clone, Debug)]
pub struct Partition<'a> {
    data: &'a [u8],
    /// Partition GUID.
    pub guid: [u8; 16],
    /// `(partition GUID, instance GUID)` pairs referenced by import pointers.
    pub imports: Vec<([u8; 16], [u8; 16])>,
    fields: Vec<FieldDesc>,
    classes: Vec<ClassDesc>,
    instances: Vec<(usize, usize)>,
    arrays: Vec<(usize, usize, usize)>,
    exported: usize,
    strings_at: usize,
    data_at: usize,
    data_len: usize,
    arrays_at: usize,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or(Error::Malformed("EBX header tables truncated"))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn guid(&mut self) -> Result<[u8; 16], Error> {
        Ok(self.bytes(16)?.try_into().unwrap())
    }
}

fn index(value: u32, limit: usize, what: &'static str) -> Result<usize, Error> {
    let value = value as usize;
    if value < limit {
        Ok(value)
    } else {
        Err(Error::Malformed(what))
    }
}

impl<'a> Partition<'a> {
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self, Error> {
        if data.len() > limits.max_asset_bytes {
            return Err(Error::Bound("EBX partition size"));
        }
        if data.len() < 64 || data[..4] != MAGIC {
            return Err(Error::Malformed("EBX magic"));
        }
        let mut c = Cursor { data, pos: 4 };
        let strings_at = c.u32()? as usize;
        let _strings_and_data = c.u32()?;
        let import_count = c.u32()? as usize;
        let instance_count = usize::from(c.u16()?);
        let exported = usize::from(c.u16()?);
        let _unique_classes = c.u16()?;
        let class_count = usize::from(c.u16()?);
        let field_count = usize::from(c.u16()?);
        let names_len = usize::from(c.u16()?);
        let strings_len = c.u32()? as usize;
        let array_count = c.u32()? as usize;
        let data_len = c.u32()? as usize;
        let guid = c.guid()?;
        if import_count > limits.max_elements || array_count > limits.max_elements {
            return Err(Error::Bound("EBX table size"));
        }
        c.pos = 64;
        let mut imports = Vec::with_capacity(import_count);
        for _ in 0..import_count {
            imports.push((c.guid()?, c.guid()?));
        }
        let mut names = HashMap::new();
        for name in c
            .bytes(names_len)?
            .split(|b| *b == 0)
            .filter(|n| !n.is_empty())
        {
            names.insert(
                name_hash(name),
                name.iter().map(|b| char::from(*b)).collect::<String>(),
            );
        }
        let name_of = |hash: u32| names.get(&hash).cloned().unwrap_or_else(|| "?".into());
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let hash = c.u32()?;
            let kind = c.u16()?;
            let class_ref = usize::from(c.u16()?);
            let offset = c.u32()? as usize;
            let _second = c.u32()?;
            fields.push(FieldDesc {
                name: name_of(hash),
                code: ((kind >> 4) & 0x1F) as u8,
                class_ref,
                offset,
            });
        }
        let mut classes = Vec::with_capacity(class_count);
        for _ in 0..class_count {
            let hash = c.u32()?;
            let first_field = c.u32()? as usize;
            let field_count = usize::from(c.bytes(1)?[0]);
            let alignment = usize::from(c.bytes(1)?[0]);
            let _kind = c.u16()?;
            let size = usize::from(c.u16()?);
            let _second = c.u16()?;
            if first_field + field_count > fields.len() {
                return Err(Error::Malformed("EBX class field range"));
            }
            classes.push(ClassDesc {
                name: name_of(hash),
                first_field,
                field_count,
                alignment: if alignment == 0 { 4 } else { alignment },
                size,
            });
        }
        let mut instances = Vec::with_capacity(instance_count);
        for _ in 0..instance_count {
            let class = index(u32::from(c.u16()?), classes.len(), "EBX instance class")?;
            instances.push((class, usize::from(c.u16()?)));
        }
        c.pos = c.pos.next_multiple_of(16);
        let mut arrays = Vec::with_capacity(array_count);
        for _ in 0..array_count {
            let offset = c.u32()? as usize;
            let count = c.u32()? as usize;
            let class = c.u32()? as usize;
            arrays.push((offset, count, class));
        }
        let data_at = strings_at
            .checked_add(strings_len)
            .ok_or(Error::Malformed("EBX section offsets"))?;
        let arrays_at = data_at
            .checked_add(data_len)
            .filter(|end| *end <= data.len())
            .ok_or(Error::Malformed("EBX data exceeds partition"))?;
        Ok(Self {
            data,
            guid,
            imports,
            fields,
            classes,
            instances,
            arrays,
            exported,
            strings_at,
            data_at,
            data_len,
            arrays_at,
        })
    }

    /// Class name of the first instance (the asset's primary object).
    pub fn primary_class(&self) -> Option<&str> {
        self.instances
            .first()
            .map(|(class, _)| self.classes[*class].name.as_str())
    }

    /// Decode every instance. The instance cursor must end within 16 bytes of
    /// the declared data length.
    pub fn objects(&self, limits: &Limits) -> Result<Vec<Object>, Error> {
        let mut decoder = Decoder {
            part: self,
            limits,
            values: 0,
        };
        let mut pos = self.data_at;
        let mut out = Vec::new();
        for (entry, (class, count)) in self.instances.iter().enumerate() {
            let cls = &self.classes[*class];
            let exported = entry < self.exported;
            for _ in 0..*count {
                pos = pos.next_multiple_of(cls.alignment);
                let mut guid = None;
                if exported {
                    let bytes = self
                        .data
                        .get(pos..pos + 16)
                        .ok_or(Error::Malformed("EBX instance GUID"))?;
                    guid = Some(bytes.try_into().unwrap());
                    pos += 16;
                }
                if cls.alignment != 4 {
                    pos += 8;
                }
                let start = pos
                    .checked_sub(8)
                    .ok_or(Error::Malformed("EBX instance start"))?;
                let fields = decoder.class(cls, start, 0)?;
                out.push(Object {
                    class: cls.name.clone(),
                    guid,
                    fields,
                });
                pos = start + cls.size;
            }
        }
        let expected = self.data_at + self.data_len;
        if pos > expected || expected - pos > 16 {
            return Err(Error::Malformed("EBX instance layout"));
        }
        Ok(out)
    }

    /// Resolve an import pointer value.
    fn import(&self, index: usize) -> Result<Pointer, Error> {
        let (partition, instance) = *self
            .imports
            .get(index)
            .ok_or(Error::Malformed("EBX import index"))?;
        Ok(Pointer::Import {
            partition,
            instance,
        })
    }
}

struct Decoder<'p, 'a> {
    part: &'p Partition<'a>,
    limits: &'p Limits,
    values: usize,
}

impl Decoder<'_, '_> {
    fn bytes(&self, pos: usize, n: usize) -> Result<&[u8], Error> {
        self.part
            .data
            .get(
                pos..pos
                    .checked_add(n)
                    .ok_or(Error::Malformed("EBX field offset"))?,
            )
            .ok_or(Error::Malformed("EBX field exceeds partition"))
    }
    fn word(&self, pos: usize) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(pos, 4)?.try_into().unwrap()))
    }
    fn cstring(&self, offset: u32) -> Result<String, Error> {
        if offset == u32::MAX {
            return Ok(String::new());
        }
        let start = self.part.strings_at + offset as usize;
        let rest = self
            .part
            .data
            .get(start..)
            .ok_or(Error::Malformed("EBX string offset"))?;
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
    fn size_of(&self, code: u8, class_ref: usize) -> Result<usize, Error> {
        Ok(match code {
            code::STRUCT => {
                self.part
                    .classes
                    .get(class_ref)
                    .ok_or(Error::Malformed("EBX struct class"))?
                    .size
            }
            code::BOOLEAN | code::INT8 | code::UINT8 => 1,
            code::INT16 | code::UINT16 => 2,
            code::UINT64 | code::INT64 | code::FLOAT64 | code::RESOURCE_REF => 8,
            code::STRING => 32,
            code::GUID | code::BOXED_VALUE_REF => 16,
            code::SHA1 => 20,
            _ => 4,
        })
    }
    fn class(&mut self, cls: &ClassDesc, start: usize, depth: usize) -> Result<Fields, Error> {
        if depth > self.limits.max_depth {
            return Err(Error::Bound("EBX nesting depth"));
        }
        let mut out = Fields::default();
        for field in &self.part.fields[cls.first_field..cls.first_field + cls.field_count] {
            if field.code == code::INHERITED {
                let base = self
                    .part
                    .classes
                    .get(field.class_ref)
                    .ok_or(Error::Malformed("EBX inherited class"))?;
                out.0.extend(self.class(base, start, depth + 1)?.0);
            } else {
                let value =
                    self.value(field.code, field.class_ref, start + field.offset, depth + 1)?;
                out.0.push((field.name.clone(), value));
            }
        }
        Ok(out)
    }
    fn value(
        &mut self,
        code: u8,
        class_ref: usize,
        pos: usize,
        depth: usize,
    ) -> Result<Value, Error> {
        if depth > self.limits.max_depth {
            return Err(Error::Bound("EBX nesting depth"));
        }
        self.values += 1;
        if self.values > self.limits.max_values {
            return Err(Error::Bound("EBX decoded values"));
        }
        self.bytes(pos, self.size_of(code, class_ref)?)?;
        let b = |n: usize| self.bytes(pos, n);
        Ok(match code {
            code::STRUCT => {
                let cls = self
                    .part
                    .classes
                    .get(class_ref)
                    .ok_or(Error::Malformed("EBX struct class"))?
                    .clone();
                Value::Struct(self.class(&cls, pos, depth)?)
            }
            code::POINTER => {
                let v = self.word(pos)?;
                Value::Pointer(if v == 0 {
                    Pointer::Null
                } else if v & 0x8000_0000 != 0 {
                    self.part.import((v & 0x7FFF_FFFF) as usize)?
                } else {
                    Pointer::Internal(v as usize - 1)
                })
            }
            code::ARRAY => {
                let index = self.word(pos)? as usize;
                let (offset, count, array_class) = *self
                    .part
                    .arrays
                    .get(index)
                    .ok_or(Error::Malformed("EBX array index"))?;
                let cls = self
                    .part
                    .classes
                    .get(array_class)
                    .ok_or(Error::Malformed("EBX array class"))?;
                let member = self
                    .part
                    .fields
                    .get(cls.first_field)
                    .ok_or(Error::Malformed("EBX array member"))?
                    .clone();
                if count > self.limits.max_elements {
                    return Err(Error::Bound("EBX array elements"));
                }
                let stride = self.size_of(member.code, member.class_ref)?;
                let base = self.part.arrays_at + offset;
                let mut items = Vec::with_capacity(count.min(4096));
                for i in 0..count {
                    items.push(self.value(
                        member.code,
                        member.class_ref,
                        base + i * stride,
                        depth + 1,
                    )?);
                }
                Value::Array(items)
            }
            code::STRING => {
                let bytes = b(32)?;
                let end = bytes.iter().position(|c| *c == 0).unwrap_or(32);
                Value::String(String::from_utf8_lossy(&bytes[..end]).into_owned())
            }
            code::CSTRING | code::FILE_REF => Value::String(self.cstring(self.word(pos)?)?),
            code::ENUM => {
                let value = self.word(pos)? as i32;
                let name = self.part.classes.get(class_ref).and_then(|cls| {
                    self.part.fields[cls.first_field..cls.first_field + cls.field_count]
                        .iter()
                        .find(|f| value >= 0 && f.offset == value as usize)
                        .map(|f| f.name.clone())
                });
                Value::Enum { value, name }
            }
            code::BOOLEAN => Value::Bool(b(1)?[0] != 0),
            code::INT8 => Value::Int8(b(1)?[0] as i8),
            code::UINT8 => Value::UInt8(b(1)?[0]),
            code::INT16 => Value::Int16(i16::from_le_bytes(b(2)?.try_into().unwrap())),
            code::UINT16 => Value::UInt16(u16::from_le_bytes(b(2)?.try_into().unwrap())),
            code::INT32 => Value::Int32(i32::from_le_bytes(b(4)?.try_into().unwrap())),
            code::UINT32 => Value::UInt32(self.word(pos)?),
            code::UINT64 => Value::UInt64(u64::from_le_bytes(b(8)?.try_into().unwrap())),
            code::INT64 => Value::Int64(i64::from_le_bytes(b(8)?.try_into().unwrap())),
            code::FLOAT32 => Value::Float32(f32::from_le_bytes(b(4)?.try_into().unwrap())),
            code::FLOAT64 => Value::Float64(f64::from_le_bytes(b(8)?.try_into().unwrap())),
            code::GUID => Value::Guid(b(16)?.try_into().unwrap()),
            code::SHA1 => Value::Sha1(b(20)?.try_into().unwrap()),
            code::RESOURCE_REF => Value::ResourceRef(u64::from_le_bytes(b(8)?.try_into().unwrap())),
            code::TYPE_REF => Value::TypeRef(self.cstring(self.word(pos)?)?),
            code::BOXED_VALUE_REF => Value::BoxedValueRef(b(16)?.try_into().unwrap()),
            other => Value::Unsupported(other),
        })
    }
}

#[cfg(test)]
mod tests;
