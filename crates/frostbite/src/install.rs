// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An installation: `Data/` plus the optional `Update/Patch/Data/` layer.
//!
//! `layout.toc` lists superbundles. For each one with `cas` set, the patch
//! `.toc` (when present) replaces the base bundle list; a patch bundle is read
//! from the patch `.sb` unless it is marked `base`. Bundle manifests list EBX
//! entries by name and SHA-1; the patch catalogue takes precedence over the
//! base catalogue when resolving records.
use crate::cas::{self, Record};
use crate::dbobject::{self, Value};
use crate::ebx::{Object, Partition};
use crate::{Error, Limits};
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// One EBX entry of a bundle manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EbxEntry {
    pub name: String,
    pub sha1: [u8; 20],
    pub original_size: u64,
    /// Base and delta records of a `casPatchType` 2 entry.
    pub delta: Option<([u8; 20], [u8; 20])>,
}

/// Where an EBX entry was listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EbxLocation {
    pub superbundle: String,
    pub bundle: String,
    pub entry: EbxEntry,
}

/// File bytes used to read installed content. Large archives are represented
/// by their consumed record ranges, not by the entire archive.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceRange {
    pub path: PathBuf,
    pub offset: u64,
    pub size: usize,
}

/// One resource entry of a bundle manifest. Its bytes are read like an EBX
/// entry's through [`Store::bytes`] on `entry`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResLocation {
    pub superbundle: String,
    pub bundle: String,
    pub entry: EbxEntry,
    /// The resource ID an EBX `ResourceRef` field holds.
    pub rid: u64,
    pub res_type: u32,
    pub meta: Vec<u8>,
}

/// A chunk a non-cas superbundle stores inline in its own `.sb`, such as a
/// localized text database under `Win32/loc/`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineChunk {
    pub id: [u8; 16],
    pub source: SourceRange,
}

/// Opened installation with its superbundle list.
#[derive(Clone, Debug)]
pub struct Installation {
    root: PathBuf,
    superbundles: Vec<String>,
    limits: Limits,
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, Error> {
    let size = std::fs::metadata(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Missing(path.display().to_string()),
            _ => Error::Io(e),
        })?
        .len();
    if size > limit as u64 {
        return Err(Error::Bound("container file size"));
    }
    Ok(std::fs::read(path)?)
}

/// Read `size` bytes at `offset` of a file without loading the whole file.
fn read_range(file: &mut File, offset: u64, size: usize) -> Result<Vec<u8>, Error> {
    file.seek(SeekFrom::Start(offset))?;
    let mut out = vec![0u8; size];
    file.read_exact(&mut out).map_err(|e| match e.kind() {
        std::io::ErrorKind::UnexpectedEof => Error::Malformed("record exceeds file"),
        _ => Error::Io(e),
    })?;
    Ok(out)
}

impl Installation {
    pub fn open(root: &Path, limits: Limits) -> Result<Self, Error> {
        let layout = dbobject::load(
            &read_bounded(&root.join("Data/layout.toc"), limits.max_container_bytes)?,
            &limits,
        )?;
        let superbundles = layout
            .get("superBundles")
            .and_then(Value::as_list)
            .ok_or(Error::Malformed("layout superBundles"))?
            .iter()
            .map(|sb| {
                sb.get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or(Error::Malformed("superbundle name"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            root: root.to_owned(),
            superbundles,
            limits,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    pub fn superbundles(&self) -> &[String] {
        &self.superbundles
    }

    fn bundle_sources(&self, superbundle: &str) -> Result<Vec<(String, SourceRange)>, Error> {
        if superbundle.contains("..") {
            return Err(Error::Malformed("superbundle path"));
        }
        let base_toc = self.root.join("Data").join(format!("{superbundle}.toc"));
        let patch_toc = self
            .root
            .join("Update/Patch/Data")
            .join(format!("{superbundle}.toc"));
        if !base_toc.exists() {
            return Ok(Vec::new());
        }
        let toc = dbobject::load(
            &read_bounded(&base_toc, self.limits.max_container_bytes)?,
            &self.limits,
        )?;
        if toc.get("cas").and_then(Value::as_bool) != Some(true) {
            return Ok(Vec::new());
        }
        let patch = if patch_toc.exists() {
            Some(dbobject::load(
                &read_bounded(&patch_toc, self.limits.max_container_bytes)?,
                &self.limits,
            )?)
        } else {
            None
        };
        let list = patch
            .as_ref()
            .unwrap_or(&toc)
            .get("bundles")
            .and_then(Value::as_list)
            .ok_or(Error::Malformed("toc bundles"))?;
        if list.len() > self.limits.max_bundles {
            return Err(Error::Bound("bundles per superbundle"));
        }
        let mut out = Vec::with_capacity(list.len());
        for entry in list {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .ok_or(Error::Malformed("bundle id"))?;
            let offset = entry
                .get("offset")
                .and_then(Value::as_i64)
                .and_then(|v| u64::try_from(v).ok())
                .ok_or(Error::Malformed("bundle offset"))?;
            let size = entry
                .get("size")
                .and_then(Value::as_i64)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or(Error::Malformed("bundle size"))?;
            if size > self.limits.max_container_bytes {
                return Err(Error::Bound("bundle manifest size"));
            }
            let from_base =
                patch.is_none() || entry.get("base").and_then(Value::as_bool) == Some(true);
            let path = if from_base {
                base_toc.with_extension("sb")
            } else {
                patch_toc.with_extension("sb")
            };
            out.push((id.to_owned(), SourceRange { path, offset, size }));
        }
        Ok(out)
    }

    /// Every bundle manifest of a cas superbundle as `(bundle id, manifest)`.
    pub fn bundles(&self, superbundle: &str) -> Result<Vec<(String, Value)>, Error> {
        let sources = self.bundle_sources(superbundle)?;
        let mut handles = HashMap::new();
        let mut out = Vec::with_capacity(sources.len());
        for (id, source) in sources {
            if !handles.contains_key(&source.path) {
                handles.insert(source.path.clone(), File::open(&source.path)?);
            }
            let bytes = read_range(
                handles.get_mut(&source.path).unwrap(),
                source.offset,
                source.size,
            )?;
            let manifest = dbobject::parse(&bytes, &self.limits)?
                .0
                .ok_or(Error::Malformed("empty bundle manifest"))?;
            out.push((id, manifest));
        }
        Ok(out)
    }

    /// Index every EBX entry by name, in superbundle and bundle order.
    pub fn ebx_index(&self) -> Result<HashMap<String, Vec<EbxLocation>>, Error> {
        let mut index: HashMap<String, Vec<EbxLocation>> = HashMap::new();
        let mut entries = 0usize;
        let mut bundles = 0usize;
        for superbundle in &self.superbundles {
            for (bundle, manifest) in self.bundles(superbundle)? {
                bundles += 1;
                if bundles > self.limits.max_bundles {
                    return Err(Error::Bound("bundles"));
                }
                let Some(list) = manifest.get("ebx").and_then(Value::as_list) else {
                    continue;
                };
                for ebx in list {
                    entries += 1;
                    if entries > self.limits.max_entries {
                        return Err(Error::Bound("EBX entries"));
                    }
                    let entry = ebx_entry(ebx)?;
                    index
                        .entry(entry.name.clone())
                        .or_default()
                        .push(EbxLocation {
                            superbundle: superbundle.clone(),
                            bundle: bundle.clone(),
                            entry,
                        });
                }
            }
        }
        Ok(index)
    }

    /// EBX entries one cas superbundle lists, in bundle order, keeping the
    /// first listing of each name as [`Self::ebx_index`] readers do. Lets a
    /// reader that knows where its assets live skip every other superbundle.
    pub fn superbundle_ebx(&self, superbundle: &str) -> Result<Vec<EbxLocation>, Error> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut entries = 0usize;
        for (bundle, manifest) in self.bundles(superbundle)? {
            let Some(list) = manifest.get("ebx").and_then(Value::as_list) else {
                continue;
            };
            for ebx in list {
                entries += 1;
                if entries > self.limits.max_entries {
                    return Err(Error::Bound("EBX entries"));
                }
                let entry = ebx_entry(ebx)?;
                if seen.insert(entry.name.clone()) {
                    out.push(EbxLocation {
                        superbundle: superbundle.to_owned(),
                        bundle: bundle.clone(),
                        entry,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Resource entries one cas superbundle lists, in bundle order, keeping
    /// the first listing of each resource ID.
    pub fn superbundle_res(&self, superbundle: &str) -> Result<Vec<ResLocation>, Error> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut entries = 0usize;
        for (bundle, manifest) in self.bundles(superbundle)? {
            let Some(list) = manifest.get("res").and_then(Value::as_list) else {
                continue;
            };
            for res in list {
                entries += 1;
                if entries > self.limits.max_entries {
                    return Err(Error::Bound("resource entries"));
                }
                let entry = ebx_entry(res)?;
                // Stored as a signed 64-bit value; the ID is its bit pattern.
                let rid = res
                    .get("resRid")
                    .and_then(Value::as_i64)
                    .ok_or(Error::Malformed("resource id"))? as u64;
                let res_type = res
                    .get("resType")
                    .and_then(Value::as_i64)
                    .and_then(|v| {
                        u32::try_from(v)
                            .ok()
                            .or_else(|| i32::try_from(v).ok().map(|v| v as u32))
                    })
                    .ok_or(Error::Malformed("resource type"))?;
                let meta = res
                    .get("resMeta")
                    .and_then(Value::as_blob)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default();
                if seen.insert(rid) {
                    out.push(ResLocation {
                        superbundle: superbundle.to_owned(),
                        bundle: bundle.clone(),
                        entry,
                        rid,
                        res_type,
                        meta,
                    });
                }
            }
        }
        Ok(out)
    }

    /// The cas record SHA-1 of each wanted chunk, from the `chunks` lists of
    /// every superbundle's `.toc`, the patch layer's listing first. Bundle
    /// manifests are not read, and inline chunks have no cas record.
    pub fn chunk_records(
        &self,
        wanted: &BTreeSet<[u8; 16]>,
    ) -> Result<std::collections::BTreeMap<[u8; 16], [u8; 20]>, Error> {
        let mut found = std::collections::BTreeMap::new();
        for superbundle in &self.superbundles {
            if found.len() == wanted.len() {
                break;
            }
            if superbundle.contains("..") {
                return Err(Error::Malformed("superbundle path"));
            }
            for data in [self.root.join("Update/Patch/Data"), self.root.join("Data")] {
                let toc = data.join(format!("{superbundle}.toc"));
                if !toc.exists() {
                    continue;
                }
                let value = dbobject::load(
                    &read_bounded(&toc, self.limits.max_container_bytes)?,
                    &self.limits,
                )?;
                let Some(list) = value.get("chunks").and_then(Value::as_list) else {
                    continue;
                };
                if list.len() > self.limits.max_elements {
                    return Err(Error::Bound("chunks per superbundle"));
                }
                for entry in list {
                    let id = entry
                        .get("id")
                        .and_then(Value::as_guid)
                        .ok_or(Error::Malformed("chunk id"))?;
                    if let Some(sha1) = entry.get("sha1").and_then(Value::as_sha1)
                        && wanted.contains(&id)
                    {
                        found.entry(id).or_insert(sha1);
                    }
                }
            }
        }
        Ok(found)
    }

    /// Chunks a superbundle's `.toc` lists with an offset and size in its
    /// `.sb`: the patch layer's first, then base chunks it does not replace.
    /// Chunks kept in cas archives (listed by SHA-1) are not inline.
    pub fn inline_chunks(&self, superbundle: &str) -> Result<Vec<InlineChunk>, Error> {
        if superbundle.contains("..") {
            return Err(Error::Malformed("superbundle path"));
        }
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for data in [self.root.join("Update/Patch/Data"), self.root.join("Data")] {
            let toc = data.join(format!("{superbundle}.toc"));
            if !toc.exists() {
                continue;
            }
            let value = dbobject::load(
                &read_bounded(&toc, self.limits.max_container_bytes)?,
                &self.limits,
            )?;
            let Some(list) = value.get("chunks").and_then(Value::as_list) else {
                continue;
            };
            if list.len() > self.limits.max_elements {
                return Err(Error::Bound("chunks per superbundle"));
            }
            for entry in list {
                let id = entry
                    .get("id")
                    .and_then(Value::as_guid)
                    .ok_or(Error::Malformed("chunk id"))?;
                if entry.get("sha1").is_some() {
                    continue;
                }
                let offset = entry
                    .get("offset")
                    .and_then(Value::as_i64)
                    .and_then(|v| u64::try_from(v).ok())
                    .ok_or(Error::Malformed("chunk offset"))?;
                let size = entry
                    .get("size")
                    .and_then(Value::as_i64)
                    .and_then(|v| usize::try_from(v).ok())
                    .ok_or(Error::Malformed("chunk size"))?;
                if size > self.limits.max_asset_bytes {
                    return Err(Error::Bound("inline chunk size"));
                }
                if seen.insert(id) {
                    out.push(InlineChunk {
                        id,
                        source: SourceRange {
                            path: toc.with_extension("sb"),
                            offset,
                            size,
                        },
                    });
                }
            }
        }
        Ok(out)
    }

    /// Decompressed bytes of one inline chunk.
    pub fn inline_chunk_bytes(&self, chunk: &InlineChunk) -> Result<Vec<u8>, Error> {
        let path = &chunk.source.path;
        let mut file =
            File::open(path).map_err(|_| Error::Missing(format!("archive {}", path.display())))?;
        let raw = read_range(&mut file, chunk.source.offset, chunk.source.size)?;
        cas::read_blocks(&raw, &self.limits)
    }

    /// Bundle ids of every cas superbundle.
    pub fn bundle_ids(&self) -> Result<Vec<String>, Error> {
        let mut out = Vec::new();
        for superbundle in &self.superbundles {
            out.extend(self.bundles(superbundle)?.into_iter().map(|(id, _)| id));
        }
        Ok(out)
    }

    /// Sorted, deduplicated bundle-manifest and EBX record ranges used by this
    /// installation, including both base and delta records of patched assets.
    /// Layout, TOC and catalogue files must be fingerprinted separately.
    pub fn source_ranges(&self) -> Result<Vec<SourceRange>, Error> {
        let index = self.ebx_index()?;
        let store = Store::open(self)?;
        let mut ranges = BTreeSet::new();
        let mut bundles = 0usize;
        for superbundle in &self.superbundles {
            for (_, source) in self.bundle_sources(superbundle)? {
                bundles += 1;
                if bundles > self.limits.max_bundles {
                    return Err(Error::Bound("bundles"));
                }
                ranges.insert(source);
            }
        }
        for locations in index.values() {
            for location in locations {
                let entry = &location.entry;
                match entry.delta {
                    Some((base, delta)) => {
                        ranges.insert(store.source(&base)?);
                        ranges.insert(store.source(&delta)?);
                    }
                    None => {
                        ranges.insert(store.source(&entry.sha1)?);
                    }
                }
            }
        }
        Ok(ranges.into_iter().collect())
    }
}

fn ebx_entry(value: &Value) -> Result<EbxEntry, Error> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or(Error::Malformed("EBX entry name"))?
        .to_owned();
    let sha1 = value
        .get("sha1")
        .and_then(Value::as_sha1)
        .ok_or(Error::Malformed("EBX entry sha1"))?;
    let original_size = value
        .get("originalSize")
        .and_then(Value::as_i64)
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(Error::Malformed("EBX entry originalSize"))?;
    let delta = if value.get("casPatchType").and_then(Value::as_i64) == Some(2) {
        Some((
            value
                .get("baseSha1")
                .and_then(Value::as_sha1)
                .ok_or(Error::Malformed("EBX entry baseSha1"))?,
            value
                .get("deltaSha1")
                .and_then(Value::as_sha1)
                .ok_or(Error::Malformed("EBX entry deltaSha1"))?,
        ))
    } else {
        None
    };
    Ok(EbxEntry {
        name,
        sha1,
        original_size,
        delta,
    })
}

/// Record lookup over both catalogues with lazily opened archives.
pub struct Store {
    base: (PathBuf, HashMap<[u8; 20], Record>),
    patch: Option<(PathBuf, HashMap<[u8; 20], Record>)>,
    handles: HashMap<PathBuf, File>,
    limits: Limits,
}

impl Store {
    pub fn open(installation: &Installation) -> Result<Self, Error> {
        let limits = *installation.limits();
        let catalog = |dir: PathBuf| -> Result<_, Error> {
            let entries = cas::parse_catalog(
                &read_bounded(&dir.join("cas.cat"), limits.max_container_bytes)?,
                &limits,
            )?;
            Ok((dir, entries))
        };
        let root = installation.root();
        let patch_dir = root.join("Update/Patch/Data");
        Ok(Self {
            base: catalog(root.join("Data"))?,
            patch: if patch_dir.join("cas.cat").exists() {
                Some(catalog(patch_dir)?)
            } else {
                None
            },
            handles: HashMap::new(),
            limits,
        })
    }

    fn locate(&self, sha1: &[u8; 20]) -> Option<(PathBuf, Record)> {
        self.patch
            .iter()
            .chain(std::iter::once(&self.base))
            .find_map(|(dir, entries)| entries.get(sha1).map(|r| (dir.clone(), *r)))
    }

    fn source(&self, sha1: &[u8; 20]) -> Result<SourceRange, Error> {
        let (dir, record) = self
            .locate(sha1)
            .ok_or_else(|| Error::Missing(format!("cas record {}", crate::hex(sha1))))?;
        if record.size as usize > self.limits.max_asset_bytes {
            return Err(Error::Bound("cas record size"));
        }
        Ok(SourceRange {
            path: dir.join(format!("cas_{:02}.cas", record.archive)),
            offset: u64::from(record.offset),
            size: record.size as usize,
        })
    }

    fn raw(&mut self, sha1: &[u8; 20]) -> Result<Vec<u8>, Error> {
        let SourceRange { path, offset, size } = self.source(sha1)?;
        if !self.handles.contains_key(&path) {
            let file = File::open(&path)
                .map_err(|_| Error::Missing(format!("archive {}", path.display())))?;
            self.handles.insert(path.clone(), file);
        }
        let file = self.handles.get_mut(&path).unwrap();
        read_range(file, offset, size)
    }

    /// Decompressed bytes of an entry, applying its delta when present.
    pub fn bytes(&mut self, entry: &EbxEntry) -> Result<Vec<u8>, Error> {
        let data = match entry.delta {
            Some((base, delta)) => {
                let base = self.raw(&base)?;
                let delta = self.raw(&delta)?;
                cas::read_patched(&base, &delta, &self.limits)?
            }
            None => cas::read_blocks(&self.raw(&entry.sha1)?, &self.limits)?,
        };
        if data.len() as u64 != entry.original_size {
            return Err(Error::Malformed("asset size differs from manifest"));
        }
        Ok(data)
    }

    /// Decompressed bytes of the cas record with this SHA-1, such as a chunk
    /// a `.toc` lists.
    pub fn record_bytes(&mut self, sha1: &[u8; 20]) -> Result<Vec<u8>, Error> {
        let raw = self.raw(sha1)?;
        cas::read_blocks(&raw, &self.limits)
    }

    /// Decode every object of an entry.
    pub fn objects(&mut self, entry: &EbxEntry) -> Result<Vec<Object>, Error> {
        let data = self.bytes(entry)?;
        Partition::parse(&data, &self.limits)?.objects(&self.limits)
    }
}

#[cfg(test)]
mod tests;
