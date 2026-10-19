// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Writers for constructed containers, partitions and installations used by
//! tests. They produce the formats the readers accept; they contain no game data.
use crate::name_hash;
use std::path::Path;

/// 7-bit length encoding.
pub fn leb(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// A DbObject value to encode.
#[derive(Clone, Debug)]
pub enum Db {
    List(Vec<Db>),
    Object(Vec<(&'static str, Db)>),
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

impl Db {
    pub fn str(text: &str) -> Self {
        Self::String(text.to_owned())
    }
    /// Encode as a named (`Some`) or unnamed record.
    pub fn record(&self, name: Option<&str>) -> Vec<u8> {
        let (kind, payload) = match self {
            Self::List(items) => {
                let mut body: Vec<u8> = items.iter().flat_map(|v| v.record(None)).collect();
                body.push(0);
                (1u8, [leb(body.len()), body].concat())
            }
            Self::Object(members) => {
                let mut body: Vec<u8> = members
                    .iter()
                    .flat_map(|(n, v)| v.record(Some(n)))
                    .collect();
                body.push(0);
                (2, [leb(body.len()), body].concat())
            }
            Self::Bool(v) => (6, vec![u8::from(*v)]),
            Self::String(text) => {
                let mut bytes = text.as_bytes().to_vec();
                bytes.push(0);
                (7, [leb(bytes.len()), bytes].concat())
            }
            Self::Int(v) => (8, v.to_le_bytes().to_vec()),
            Self::Long(v) => (9, v.to_le_bytes().to_vec()),
            Self::Float(v) => (11, v.to_le_bytes().to_vec()),
            Self::Double(v) => (12, v.to_le_bytes().to_vec()),
            Self::Guid(v) => (15, v.to_vec()),
            Self::Sha1(v) => (16, v.to_vec()),
            Self::Blob(v) => (19, [leb(v.len()), v.clone()].concat()),
        };
        let mut out = match name {
            Some(n) => {
                let mut head = vec![kind];
                head.extend_from_slice(n.as_bytes());
                head.push(0);
                head
            }
            None => vec![kind | 0x80],
        };
        out.extend_from_slice(&payload);
        out
    }
}

/// A container with the clear (`00 D1 CE 03`) obfuscation header.
pub fn clear_header(body: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 0x22C];
    out[..4].copy_from_slice(&[0x00, 0xD1, 0xCE, 0x03]);
    out.extend_from_slice(body);
    out
}

/// One cas block record with the given codec (0 stored, 2 zlib, 9 LZ4).
pub fn block(payload: &[u8], codec: u8) -> Vec<u8> {
    let body = match codec {
        2 => miniz_oxide::deflate::compress_to_vec_zlib(payload, 6),
        9 => lz4_flex::block::compress(payload),
        _ => payload.to_vec(),
    };
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.push(codec);
    out.push(0x70 | ((body.len() >> 16) as u8 & 0x0F));
    out.extend_from_slice(&((body.len() & 0xFFFF) as u16).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Field types the partition writer supports.
#[derive(Clone, Debug)]
pub enum Field {
    Int(i32),
    Long(i64),
    Guids(Vec<[u8; 16]>),
    UInt(u32),
    Float(f32),
    Bool(bool),
    CString(String),
    /// Enumerator name and value; the enum type lists exactly this enumerator.
    Enum(&'static str, &'static str, i32),
    /// Pointer to `(partition GUID, instance GUID)` through the import table.
    Import([u8; 16], [u8; 16]),
    /// Array of imports.
    Imports(Vec<([u8; 16], [u8; 16])>),
    /// Raw pointer word: 0 is null, 1-based local index, or high-bit import.
    /// Permits constructed invalid pointers as well as internal graph edges.
    RawPointer(u32),
    RawPointers(Vec<u32>),
    /// Array of signed 32-bit integers.
    Ints(Vec<i32>),
    /// Array of structs, each a list of `Float`/`Int`/`CString` members.
    Structs(&'static str, Vec<Vec<(&'static str, Field)>>),
    /// Inline struct of scalar members.
    Struct(&'static str, Vec<(&'static str, Field)>),
    /// A 16-byte GUID, such as a chunk reference.
    Guid([u8; 16]),
}

struct FieldDesc {
    name: u32,
    kind: u16,
    class_ref: u16,
    offset: u32,
}
struct ClassDesc {
    name: u32,
    first: i32,
    count: u8,
    size: u16,
}

/// Builds a partition with one exported primary instance.
#[derive(Default)]
pub struct PartitionWriter {
    names: Vec<u8>,
    fields: Vec<FieldDesc>,
    classes: Vec<ClassDesc>,
    imports: Vec<([u8; 16], [u8; 16])>,
    strings: Vec<u8>,
    arrays: Vec<(u32, u32, i32)>,
    array_data: Vec<u8>,
}

impl PartitionWriter {
    fn name(&mut self, text: &str) -> u32 {
        let hash = name_hash(text.as_bytes());
        self.names.extend_from_slice(text.as_bytes());
        self.names.push(0);
        hash
    }
    fn string(&mut self, text: &str) -> u32 {
        let at = self.strings.len() as u32;
        self.strings.extend_from_slice(text.as_bytes());
        self.strings.push(0);
        at
    }
    fn import(&mut self, file: [u8; 16], instance: [u8; 16]) -> u32 {
        let index = self
            .imports
            .iter()
            .position(|i| *i == (file, instance))
            .unwrap_or_else(|| {
                self.imports.push((file, instance));
                self.imports.len() - 1
            });
        0x8000_0000 | index as u32
    }
    /// Declare a class from `(name, code, class_ref, offset)` rows.
    fn class(&mut self, name: &str, size: u16, rows: Vec<(String, u8, u16, u32)>) -> u16 {
        let first = self.fields.len() as i32;
        let count = rows.len() as u8;
        for (field, code, class_ref, offset) in rows {
            let hash = self.name(&field);
            self.fields.push(FieldDesc {
                name: hash,
                kind: u16::from(code) << 4,
                class_ref,
                offset,
            });
        }
        let hash = self.name(name);
        self.classes.push(ClassDesc {
            name: hash,
            first,
            count,
            size,
        });
        (self.classes.len() - 1) as u16
    }
    /// Struct layout from the first item; enum members list every enumerator
    /// used by any item. Returns the class index and size.
    fn struct_class(&mut self, name: &str, items: &[Vec<(&'static str, Field)>]) -> (u16, u16) {
        let first = items.first().cloned().unwrap_or_default();
        let mut rows = Vec::new();
        let mut offset = 0;
        for (i, (member, field)) in first.iter().enumerate() {
            if let Field::Struct(type_name, _) = field {
                let nested: Vec<_> = items
                    .iter()
                    .map(|item| match &item[i].1 {
                        Field::Struct(name, fields) if name == type_name => fields.clone(),
                        _ => panic!("inconsistent synthetic struct"),
                    })
                    .collect();
                let (class, size) = self.struct_class(type_name, &nested);
                rows.push((member.to_string(), 0x02, class, offset));
                offset += u32::from(size);
                continue;
            } else if let Field::Enum(type_name, _, _) = field {
                let mut enumerators: Vec<(String, u8, u16, u32)> = Vec::new();
                for item in items {
                    if let Some((_, Field::Enum(_, enumerator, value))) = item.get(i)
                        && !enumerators.iter().any(|e| e.0 == *enumerator)
                    {
                        enumerators.push((enumerator.to_string(), 0x0F, 0, *value as u32));
                    }
                }
                let enum_class = self.class(type_name, 4, enumerators);
                rows.push((member.to_string(), 0x08, enum_class, offset));
            } else {
                rows.push((member.to_string(), scalar_code(field), 0, offset));
            }
            offset += 4;
        }
        let size = u16::try_from(offset).expect("synthetic struct size");
        (self.class(name, size, rows), size)
    }
    fn scalar(&mut self, field: &Field) -> [u8; 4] {
        match field {
            Field::Int(v) => v.to_le_bytes(),
            Field::UInt(v) => v.to_le_bytes(),
            Field::Float(v) => v.to_le_bytes(),
            Field::Bool(v) => [u8::from(*v), 0, 0, 0],
            Field::CString(text) => self.string(text).to_le_bytes(),
            Field::Enum(_, _, value) => value.to_le_bytes(),
            Field::RawPointer(value) => value.to_le_bytes(),
            Field::Import(file, instance) => self.import(*file, *instance).to_le_bytes(),
            _ => panic!("not a scalar field"),
        }
    }

    fn inline(&mut self, field: &Field) -> Vec<u8> {
        if let Field::Struct(_, fields) = field {
            fields.iter().flat_map(|(_, f)| self.inline(f)).collect()
        } else {
            self.scalar(field).to_vec()
        }
    }

    /// Encode a partition with GUID `partition` whose single exported instance
    /// of `class` (GUID `instance`) has the given fields in order.
    pub fn build(
        self,
        partition: [u8; 16],
        class: &str,
        instance: [u8; 16],
        fields: Vec<(&'static str, Field)>,
    ) -> Vec<u8> {
        self.build_many(partition, class, instance, fields, Vec::new())
    }

    /// Field rows and payload of one object.
    #[allow(clippy::type_complexity)]
    fn object(
        &mut self,
        fields: &[(&'static str, Field)],
    ) -> (Vec<(String, u8, u16, u32)>, Vec<u8>) {
        let mut rows = Vec::new();
        let mut payload = Vec::new();
        for (name, field) in fields {
            let offset = 8 + payload.len() as u32;
            match field {
                Field::Long(v) => {
                    rows.push((name.to_string(), 0x12, 0, offset));
                    payload.extend_from_slice(&v.to_le_bytes());
                }
                Field::Guid(v) => {
                    rows.push((name.to_string(), 0x15, 0, offset));
                    payload.extend_from_slice(v);
                }
                Field::Guids(list) => {
                    let array_class =
                        self.class("array", 16, vec![("member".to_string(), 0x15, 0, 0)]);
                    let at = self.array_data.len() as u32;
                    for value in list {
                        self.array_data.extend_from_slice(value);
                    }
                    self.arrays
                        .push((at, list.len() as u32, i32::from(array_class)));
                    rows.push((name.to_string(), 0x04, array_class, offset));
                    payload.extend_from_slice(&((self.arrays.len() - 1) as u32).to_le_bytes());
                }
                Field::Enum(type_name, enumerator, value) => {
                    let enum_class = self.class(
                        type_name,
                        4,
                        vec![(enumerator.to_string(), 0x0F, 0, *value as u32)],
                    );
                    rows.push((name.to_string(), 0x08, enum_class, offset));
                    payload.extend_from_slice(&value.to_le_bytes());
                }
                Field::Import(file, inst) => {
                    rows.push((name.to_string(), 0x03, 0, offset));
                    let value = self.import(*file, *inst);
                    payload.extend_from_slice(&value.to_le_bytes());
                }
                Field::RawPointer(value) => {
                    rows.push((name.to_string(), 0x03, 0, offset));
                    payload.extend_from_slice(&value.to_le_bytes());
                }
                Field::RawPointers(list) => {
                    let array_class =
                        self.class("array", 4, vec![("member".to_string(), 0x03, 0, 0)]);
                    let at = self.array_data.len() as u32;
                    for value in list {
                        self.array_data.extend_from_slice(&value.to_le_bytes());
                    }
                    self.arrays
                        .push((at, list.len() as u32, i32::from(array_class)));
                    rows.push((name.to_string(), 0x04, array_class, offset));
                    payload.extend_from_slice(&((self.arrays.len() - 1) as u32).to_le_bytes());
                }
                Field::Ints(list) => {
                    let array_class =
                        self.class("array", 4, vec![("member".to_string(), 0x0F, 0, 0)]);
                    let at = self.array_data.len() as u32;
                    for value in list {
                        self.array_data.extend_from_slice(&value.to_le_bytes());
                    }
                    self.arrays
                        .push((at, list.len() as u32, i32::from(array_class)));
                    rows.push((name.to_string(), 0x04, array_class, offset));
                    payload.extend_from_slice(&((self.arrays.len() - 1) as u32).to_le_bytes());
                }
                Field::Imports(list) => {
                    let array_class =
                        self.class("array", 4, vec![("member".to_string(), 0x03, 0, 0)]);
                    let at = self.array_data.len() as u32;
                    for (file, inst) in list {
                        let value = self.import(*file, *inst);
                        self.array_data.extend_from_slice(&value.to_le_bytes());
                    }
                    self.arrays
                        .push((at, list.len() as u32, i32::from(array_class)));
                    rows.push((name.to_string(), 0x04, array_class, offset));
                    payload.extend_from_slice(&((self.arrays.len() - 1) as u32).to_le_bytes());
                }
                Field::Struct(type_name, members) => {
                    let (struct_class, size) =
                        self.struct_class(type_name, std::slice::from_ref(members));
                    rows.push((name.to_string(), 0x02, struct_class, offset));
                    let bytes = self.inline(field);
                    debug_assert_eq!(size as usize, bytes.len());
                    payload.extend_from_slice(&bytes);
                }
                Field::Structs(type_name, items) => {
                    let (struct_class, _) = self.struct_class(type_name, items);
                    let array_class = self.class(
                        "array",
                        4,
                        vec![("member".to_string(), 0x02, struct_class, 0)],
                    );
                    let at = self.array_data.len() as u32;
                    for item in items {
                        for (_, member) in item {
                            let bytes = self.inline(member);
                            self.array_data.extend_from_slice(&bytes);
                        }
                    }
                    self.arrays
                        .push((at, items.len() as u32, i32::from(array_class)));
                    rows.push((name.to_string(), 0x04, array_class, offset));
                    payload.extend_from_slice(&((self.arrays.len() - 1) as u32).to_le_bytes());
                }
                scalar => {
                    rows.push((name.to_string(), scalar_code(scalar), 0, offset));
                    let bytes = self.scalar(scalar);
                    payload.extend_from_slice(&bytes);
                }
            }
        }
        (rows, payload)
    }

    /// Like [`Self::build`], followed by unexported instances of other classes
    /// (one instance each, in order).
    pub fn build_many(
        mut self,
        partition: [u8; 16],
        class: &str,
        instance: [u8; 16],
        fields: Vec<(&'static str, Field)>,
        extras: Vec<(&str, Vec<(&'static str, Field)>)>,
    ) -> Vec<u8> {
        let (rows, payload) = self.object(&fields);
        let primary = self.class(class, (8 + payload.len()) as u16, rows);
        let mut instances = vec![primary];
        let mut data = instance.to_vec();
        data.extend_from_slice(&payload);
        for (extra_class, extra_fields) in &extras {
            let (rows, payload) = self.object(extra_fields);
            instances.push(self.class(extra_class, (8 + payload.len()) as u16, rows));
            data.extend_from_slice(&payload);
        }
        let mut out = vec![0u8; 64];
        out[..4].copy_from_slice(&[0xCE, 0xD1, 0xB2, 0x0F]);
        for (file, inst) in &self.imports {
            out.extend_from_slice(file);
            out.extend_from_slice(inst);
        }
        out.extend_from_slice(&self.names);
        for f in &self.fields {
            out.extend_from_slice(&f.name.to_le_bytes());
            out.extend_from_slice(&f.kind.to_le_bytes());
            out.extend_from_slice(&f.class_ref.to_le_bytes());
            out.extend_from_slice(&f.offset.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        for c in &self.classes {
            out.extend_from_slice(&c.name.to_le_bytes());
            out.extend_from_slice(&c.first.to_le_bytes());
            out.push(c.count);
            out.push(4);
            out.extend_from_slice(&0x30u16.to_le_bytes());
            out.extend_from_slice(&c.size.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        for class_index in &instances {
            out.extend_from_slice(&class_index.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
        }
        pad16(&mut out);
        for (at, count, class_ref) in &self.arrays {
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&class_ref.to_le_bytes());
        }
        pad16(&mut out);
        let strings_at = out.len() as u32;
        // Strings are padded so instance data starts on a 16-byte boundary.
        while !self.strings.len().is_multiple_of(16) {
            self.strings.push(0);
        }
        while !data.len().is_multiple_of(16) {
            data.push(0);
        }
        out.extend_from_slice(&self.strings);
        out.extend_from_slice(&data);
        out.extend_from_slice(&self.array_data);
        let header = [
            strings_at,
            (self.strings.len() + data.len()) as u32,
            self.imports.len() as u32,
        ];
        for (i, word) in header.iter().enumerate() {
            out[4 + i * 4..8 + i * 4].copy_from_slice(&word.to_le_bytes());
        }
        let counts = [
            instances.len() as u16,
            1,
            instances.len() as u16,
            self.classes.len() as u16,
            self.fields.len() as u16,
            self.names.len() as u16,
        ];
        for (i, word) in counts.iter().enumerate() {
            out[16 + i * 2..18 + i * 2].copy_from_slice(&word.to_le_bytes());
        }
        let tail = [
            self.strings.len() as u32,
            self.arrays.len() as u32,
            data.len() as u32,
        ];
        for (i, word) in tail.iter().enumerate() {
            out[28 + i * 4..32 + i * 4].copy_from_slice(&word.to_le_bytes());
        }
        out[40..56].copy_from_slice(&partition);
        out
    }
}

fn scalar_code(field: &Field) -> u8 {
    match field {
        Field::Int(_) => 0x0F,
        Field::UInt(_) => 0x10,
        Field::Float(_) => 0x13,
        Field::Bool(_) => 0x0A,
        Field::CString(_) => 0x07,
        Field::RawPointer(_) | Field::Import(_, _) => 0x03,
        _ => panic!("not a scalar field"),
    }
}

fn pad16(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(16) {
        out.push(0);
    }
}

/// One EBX asset of a synthetic installation.
pub struct Asset {
    pub name: String,
    pub partition: Vec<u8>,
}

/// Write a minimal unpatched installation under `root`: one cas superbundle
/// `Win32/synthetic` with one bundle holding every asset as LZ4 cas records,
/// plus an executable file with the given bytes.
pub fn write_installation(root: &Path, executable: &[u8], assets: &[Asset]) -> std::io::Result<()> {
    write_superbundles(
        root,
        executable,
        &[CasSuperbundle {
            name: "Win32/synthetic",
            bundle: "win32/synthetic",
            assets,
        }],
        &[],
    )
}

/// A cas superbundle with one bundle, for [`write_superbundles`].
pub struct CasSuperbundle<'a> {
    pub name: &'a str,
    pub bundle: &'a str,
    pub assets: &'a [Asset],
}

/// A non-cas superbundle whose `.sb` holds inline chunks (id, bytes).
pub struct InlineSuperbundle<'a> {
    pub name: &'a str,
    pub chunks: &'a [([u8; 16], Vec<u8>)],
}

fn lz4_record(payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::new();
    for chunk in payload.chunks(0x10000) {
        record.extend_from_slice(&block(chunk, 9));
    }
    record
}

/// Write an unpatched installation with the given superbundles, listed in
/// this order in `layout.toc`. All cas records share `cas_01.cas`.
pub fn write_superbundles(
    root: &Path,
    executable: &[u8],
    cas_superbundles: &[CasSuperbundle<'_>],
    inline: &[InlineSuperbundle<'_>],
) -> std::io::Result<()> {
    let data = root.join("Data");
    std::fs::create_dir_all(&data)?;
    std::fs::write(root.join("NFS16.exe"), executable)?;
    let names = cas_superbundles
        .iter()
        .map(|s| s.name)
        .chain(inline.iter().map(|s| s.name));
    let layout = Db::Object(vec![(
        "superBundles",
        Db::List(
            names
                .map(|name| Db::Object(vec![("name", Db::str(name))]))
                .collect(),
        ),
    )]);
    std::fs::write(data.join("layout.toc"), clear_header(&layout.record(None)))?;
    let mut cas = Vec::new();
    let mut catalog = b"NyanNyanNyanNyan".to_vec();
    let mut records = 0u64;
    for superbundle in cas_superbundles {
        let mut entries = Vec::new();
        for asset in superbundle.assets {
            records += 1;
            let mut sha1 = [0u8; 20];
            sha1[..8].copy_from_slice(&records.to_le_bytes());
            let record = lz4_record(&asset.partition);
            catalog.extend_from_slice(&sha1);
            catalog.extend_from_slice(&(cas.len() as u32).to_le_bytes());
            catalog.extend_from_slice(&(record.len() as u32).to_le_bytes());
            catalog.extend_from_slice(&1u32.to_le_bytes());
            cas.extend_from_slice(&record);
            entries.push(Db::Object(vec![
                ("name", Db::str(&asset.name)),
                ("sha1", Db::Sha1(sha1)),
                ("size", Db::Long(record.len() as i64)),
                ("originalSize", Db::Long(asset.partition.len() as i64)),
            ]));
        }
        let bundle = Db::Object(vec![
            ("path", Db::str(superbundle.bundle)),
            ("ebx", Db::List(entries)),
        ])
        .record(None);
        let toc = Db::Object(vec![
            (
                "bundles",
                Db::List(vec![Db::Object(vec![
                    ("id", Db::str(superbundle.bundle)),
                    ("offset", Db::Long(0)),
                    ("size", Db::Int(bundle.len() as i32)),
                ])]),
            ),
            ("cas", Db::Bool(true)),
        ]);
        let base = data.join(superbundle.name);
        std::fs::create_dir_all(base.parent().expect("superbundle directory"))?;
        std::fs::write(base.with_extension("sb"), &bundle)?;
        std::fs::write(base.with_extension("toc"), clear_header(&toc.record(None)))?;
    }
    std::fs::write(data.join("cas_01.cas"), &cas)?;
    std::fs::write(data.join("cas.cat"), &catalog)?;
    for superbundle in inline {
        let mut sb = Vec::new();
        let mut chunks = Vec::new();
        for (id, bytes) in superbundle.chunks {
            let record = lz4_record(bytes);
            chunks.push(Db::Object(vec![
                ("id", Db::Guid(*id)),
                ("offset", Db::Long(sb.len() as i64)),
                ("size", Db::Int(record.len() as i32)),
            ]));
            sb.extend_from_slice(&record);
        }
        let toc = Db::Object(vec![
            ("bundles", Db::List(Vec::new())),
            ("chunks", Db::List(chunks)),
            ("cas", Db::Bool(false)),
        ]);
        let base = data.join(superbundle.name);
        std::fs::create_dir_all(base.parent().expect("superbundle directory"))?;
        std::fs::write(base.with_extension("sb"), &sb)?;
        std::fs::write(base.with_extension("toc"), clear_header(&toc.record(None)))?;
    }
    Ok(())
}

/// A text database binary chunk named `name` holding `(string ID, encoded
/// bytes)` entries; IDs are keyed with [`crate::textdb::string_id_hash`].
pub fn text_database(name: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut keyed: Vec<(u32, &[u8])> = entries
        .iter()
        .map(|(id, bytes)| (crate::textdb::string_id_hash(id), *bytes))
        .collect();
    keyed.sort_by_key(|(key, _)| *key);
    keyed.dedup_by_key(|(key, _)| *key);
    let table_at = 12 + 128;
    let text_at = table_at + 8 * keyed.len();
    let mut body = Vec::new();
    for value in [keyed.len(), table_at, text_at] {
        body.extend_from_slice(&(value as u32).to_le_bytes());
    }
    let mut label = name.as_bytes().to_vec();
    label.resize(128, 0);
    body.extend_from_slice(&label);
    let mut text = Vec::new();
    for (key, bytes) in &keyed {
        body.extend_from_slice(&key.to_le_bytes());
        body.extend_from_slice(&(text.len() as u32).to_le_bytes());
        text.extend_from_slice(bytes);
        text.push(0);
    }
    body.extend_from_slice(&text);
    let mut out = crate::textdb::BINARY_MAGIC.to_le_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// A histogram chunk with `entries` code units, of which the given
/// `(byte, code unit)` pairs are set; 256 entries is the single-byte form.
pub fn histogram(entries: u32, mapping: &[(u8, u16)]) -> Vec<u8> {
    let mut units = vec![0u16; entries as usize];
    for (byte, unit) in mapping {
        units[usize::from(*byte)] = *unit;
    }
    let mut body = entries.to_le_bytes().to_vec();
    for unit in units {
        body.extend_from_slice(&unit.to_le_bytes());
    }
    let mut out = crate::textdb::HISTOGRAM_MAGIC.to_le_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}
