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

    /// Decode every object of an entry.
    pub fn objects(&mut self, entry: &EbxEntry) -> Result<Vec<Object>, Error> {
        let data = self.bytes(entry)?;
        Partition::parse(&data, &self.limits)?.objects(&self.limits)
    }
}

#[cfg(test)]
mod tests;
