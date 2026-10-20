// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Each car's in-game images, decoded from the player's installation into
//! the data folder by `car-images` and read back by `profile`. No image is
//! shipped, committed or embedded.
//!
//! In the supported build a car item's `MediumTextureName` and
//! `LargeTextureName` are not asset paths but IDs of entries in two UI
//! texture mapping assets of the frontend superbundle,
//! [`MEDIUM_MAPPING`] and [`LARGE_MAPPING`]. An entry (`Id`, `Texture`,
//! `Min`, `Max`) imports a `TextureAsset`, whose `Resource` names a 128-byte
//! texture header listed in the same bundle; the header names a pixel chunk
//! that a `.toc` lists by SHA-1. Only entries covering the whole texture
//! (UV 0,0 to 1,1) are used. The first mip is decoded to RGBA and written as
//! PNG.
//!
//! Images go to `artifacts/content/car-images/<content fingerprint>/`,
//! beside the prepared content entries and never inside one: the content
//! cache refuses to replace an entry that holds files it did not write. A set
//! is published by one rename with an `index.json`; files are named by the
//! SHA-1 of their pixel chunk.

use super::names::{self, ITEM_SUPERBUNDLE, failure};
use crate::{
    exit::{Failure, Result},
    paths, records,
};
use nfs_frostbite::{
    Limits,
    ebx::{Partition, Pointer, Value},
    install::{EbxEntry, Installation, Store},
    texture::{self, Format, TextureHeader},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Directory under `artifacts/content` that holds the image sets.
pub const DIRECTORY: &str = "car-images";
pub const INDEX: &str = "index.json";
pub const INDEX_FORMAT: &str = "afterhours-car-images";
pub const INDEX_VERSION: u32 = 1;
pub const MEDIUM_MAPPING: &str = "ui/textures/items/vehiclemediumtexturemappingasset";
pub const LARGE_MAPPING: &str = "ui/textures/items/itemslargetexturemappingasset";
/// Assets a mapping entry may import; the supported build's are all here.
const TEXTURE_PREFIX: &str = "ui/";
const MAPPING_CLASSES: [&str; 2] = ["UITextureMappingAsset", "RaceUITextureMappingAsset"];
pub const MAX_INDEX_BYTES: u64 = 1 << 20;
/// Mapping entries read per mapping asset.
pub const MAX_MAPPING_ENTRIES: usize = 4096;
/// Image files in one set.
pub const MAX_IMAGES: usize = 1024;
/// PNG bytes in one set.
pub const MAX_SET_BYTES: usize = 512 << 20;

/// Which of a car's two images.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Medium,
    Large,
}

impl Size {
    pub const ALL: [Size; 2] = [Size::Medium, Size::Large];
    fn field(self) -> &'static str {
        match self {
            Self::Medium => "MediumTextureName",
            Self::Large => "LargeTextureName",
        }
    }
    fn mapping(self) -> &'static str {
        match self {
            Self::Medium => MEDIUM_MAPPING,
            Self::Large => LARGE_MAPPING,
        }
    }
}

/// One image file of a set, as the index records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexImage {
    /// `<chunk SHA-1>.png` in the set's directory.
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// The texture's `RenderFormat` name, such as `BC1_UNORM`.
    pub format: String,
    /// The `TextureAsset` the mapping entry imports.
    pub texture: String,
}

/// A named image that could not be produced, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Unresolved {
    pub definition: String,
    pub size: Size,
    /// The car item's texture name.
    pub texture_name: String,
    pub reason: String,
}

/// `index.json` of a set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub format: String,
    pub version: u32,
    pub game_directory: PathBuf,
    /// Car definitions the installation lists.
    pub cars: usize,
    /// Car items naming a texture, per size.
    pub named: BTreeMap<Size, usize>,
    pub images: BTreeMap<String, BTreeMap<Size, IndexImage>>,
    pub unresolved: Vec<Unresolved>,
}

/// One cached image for `profile`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CarImage {
    /// Absolute path of the PNG file.
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// The source texture's `RenderFormat` name, such as `BC1_UNORM`.
    pub source_format: String,
}

/// A car's cached images; a size is `null` when the car names none or it
/// could not be decoded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CarImages {
    pub medium: Option<CarImage>,
    pub large: Option<CarImage>,
}

/// A set read back from the data folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cached {
    pub directory: PathBuf,
    pub index: Index,
}

impl Cached {
    /// The images of one car definition, if the set lists it.
    pub fn get(&self, definition: &str) -> Option<CarImages> {
        let images = self.index.images.get(definition)?;
        let image = |size| {
            images.get(&size).map(|image: &IndexImage| CarImage {
                path: self.directory.join(&image.file),
                width: image.width,
                height: image.height,
                source_format: image.format.clone(),
            })
        };
        Some(CarImages {
            medium: image(Size::Medium),
            large: image(Size::Large),
        })
    }
}

/// `<data>/artifacts/content/car-images`.
pub fn root(data: &Path) -> PathBuf {
    data.join("artifacts").join("content").join(DIRECTORY)
}

fn is_image_file(name: &str) -> bool {
    name.strip_suffix(".png")
        .is_some_and(|stem| records::is_lower_hex(stem, 40))
}

pub(crate) fn ordinary(path: &Path, directory: bool) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| {
        !paths::is_link(&meta)
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            }
    })
}

/// Read and validate a set's index, checking every file it names exists.
fn validate(directory: &Path) -> std::result::Result<Index, String> {
    let index_path = directory.join(INDEX);
    if !ordinary(directory, true) || !ordinary(&index_path, false) {
        return Err("the image set is incomplete".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&index_path)
        .and_then(|file| file.take(MAX_INDEX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err("the image index exceeds its bound".into());
    }
    let index: Index = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if index.format != INDEX_FORMAT || index.version != INDEX_VERSION {
        return Err("the image index has another format".into());
    }
    let files: BTreeSet<&str> = index
        .images
        .values()
        .flat_map(BTreeMap::values)
        .map(|image| image.file.as_str())
        .collect();
    if files.len() > MAX_IMAGES {
        return Err("the image index exceeds its bound".into());
    }
    for (definition, images) in &index.images {
        if !records::is_lower_hex(definition, 32) || images.is_empty() {
            return Err("the image index lists an invalid car".into());
        }
    }
    for file in files {
        if !is_image_file(file) || !ordinary(&directory.join(file), false) {
            return Err("the image index names a missing or invalid file".into());
        }
    }
    Ok(index)
}

/// The set for content `fingerprint`, when one has been published.
/// `Err` is a sentence for a warning: the set exists but is unusable.
pub fn load(data: &Path, fingerprint: &str) -> std::result::Result<Option<Cached>, String> {
    if !records::is_lower_hex(fingerprint, 64) {
        return Ok(None);
    }
    let directory = root(data).join(fingerprint);
    if std::fs::symlink_metadata(&directory).is_err() {
        return Ok(None);
    }
    let index = validate(&directory)?;
    Ok(Some(Cached { directory, index }))
}

/// One decoded image before it is written.
struct Decoded {
    file: String,
    png: Vec<u8>,
    image: IndexImage,
}

/// The texture a mapping entry imports.
#[derive(Clone, Copy)]
struct Target {
    partition: [u8; 16],
    instance: [u8; 16],
    whole: bool,
}

fn vector(fields: Option<&Value>) -> Option<(f32, f32)> {
    let fields = fields?.as_fields()?;
    match (fields.get("x"), fields.get("y")) {
        (Some(Value::Float32(x)), Some(Value::Float32(y))) => Some((*x, *y)),
        _ => None,
    }
}

/// Mapping entries by ID.
fn mapping(
    store: &mut Store,
    entry: &EbxEntry,
) -> std::result::Result<HashMap<String, Target>, String> {
    let objects = store
        .objects(entry)
        .map_err(|e| failure("texture mapping", e))?;
    let object = objects
        .iter()
        .find(|object| MAPPING_CLASSES.contains(&object.class.as_str()))
        .ok_or("the texture mapping asset has no mapping")?;
    let output = object
        .fields
        .get("Output")
        .and_then(Value::as_array)
        .ok_or("the texture mapping has no output list")?;
    if output.len() > MAX_MAPPING_ENTRIES {
        return Err("the texture mapping exceeds its bound".into());
    }
    let mut targets = HashMap::new();
    for item in output {
        let Some(fields) = item.as_fields() else {
            continue;
        };
        let (
            Some(id),
            Some(Pointer::Import {
                partition,
                instance,
            }),
        ) = (
            fields.get("Id").and_then(Value::as_str),
            fields.get("Texture").and_then(Value::as_pointer),
        )
        else {
            continue;
        };
        let whole = vector(fields.get("Min")) == Some((0.0, 0.0))
            && vector(fields.get("Max")) == Some((1.0, 1.0));
        targets.entry(id.to_owned()).or_insert(Target {
            partition,
            instance,
            whole,
        });
    }
    Ok(targets)
}

/// CRC-32 (ISO-HDLC) lookup table, as PNG chunks use it.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut crc = n as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[n] = crc;
        n += 1;
    }
    table
};

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = u32::MAX;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        crc = CRC_TABLE[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// An 8-bit RGBA PNG of `rgba`, rows unfiltered, zlib-compressed.
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks_exact(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    };
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &header);
    chunk(
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6),
    );
    chunk(b"IEND", &[]);
    out
}

/// Every car's images, decoded from the installation at `game`. A missing
/// or unreadable image becomes an [`Unresolved`] entry; errors that stop
/// the whole set are sentences.
fn decode_all(game: &Path, limits: Limits) -> std::result::Result<(Index, Vec<Decoded>), String> {
    let install = Installation::open(game, limits).map_err(|e| failure("installation", e))?;
    let mut store = Store::open(&install).map_err(|e| failure("archive catalogue", e))?;
    let cars = names::car_items(&install, &mut store, &limits)?;
    let listed = install
        .superbundle_ebx(ITEM_SUPERBUNDLE)
        .map_err(|e| failure("item list", e))?;
    let by_name: HashMap<&str, &EbxEntry> = listed
        .iter()
        .map(|location| (location.entry.name.as_str(), &location.entry))
        .collect();
    let mut mappings = BTreeMap::new();
    for size in Size::ALL {
        let entry = by_name
            .get(size.mapping())
            .ok_or_else(|| format!("the installation has no {}", size.mapping()))?;
        mappings.insert(size, mapping(&mut store, entry)?);
    }
    let mut unresolved = Vec::new();
    let mut named: BTreeMap<Size, usize> = BTreeMap::new();
    // (definition, size, texture name, target)
    let mut wanted = Vec::new();
    for (definition, fields) in &cars {
        let definition = super::hex(definition);
        for size in Size::ALL {
            let Some(name) = fields
                .get(size.field())
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            *named.entry(size).or_default() += 1;
            let mut miss = |reason: &str| {
                unresolved.push(Unresolved {
                    definition: definition.clone(),
                    size,
                    texture_name: name.to_owned(),
                    reason: reason.to_owned(),
                })
            };
            match mappings[&size].get(name) {
                None => miss("not in the texture mapping"),
                Some(target) if !target.whole => miss("maps to part of a texture"),
                Some(target) => wanted.push((definition.clone(), size, name.to_owned(), *target)),
            }
        }
    }
    // Partition GUIDs of the imported textures, from the `ui/` assets only.
    let partitions: BTreeSet<[u8; 16]> = wanted.iter().map(|(.., t)| t.partition).collect();
    let mut textures: HashMap<[u8; 16], &EbxEntry> = HashMap::new();
    for location in &listed {
        if textures.len() == partitions.len() {
            break;
        }
        if !location.entry.name.starts_with(TEXTURE_PREFIX) {
            continue;
        }
        let data = store
            .bytes(&location.entry)
            .map_err(|e| failure("texture asset", e))?;
        let guid = Partition::parse(&data, &limits)
            .map_err(|e| failure("texture asset", e))?
            .guid;
        if partitions.contains(&guid) {
            textures.insert(guid, &location.entry);
        }
    }
    let resources: HashMap<u64, EbxEntry> = install
        .superbundle_res(ITEM_SUPERBUNDLE)
        .map_err(|e| failure("resource list", e))?
        .into_iter()
        .map(|location| (location.rid, location.entry))
        .collect();
    // Texture headers, then the chunks they name.
    let mut headers = Vec::new();
    for (definition, size, name, target) in wanted {
        let header = (|| {
            let entry = textures
                .get(&target.partition)
                .ok_or("its texture asset is not listed")?;
            let objects = store
                .objects(entry)
                .map_err(|e| failure("texture asset", e))?;
            let object = objects
                .iter()
                .find(|object| object.guid == Some(target.instance))
                .ok_or("its texture object is missing")?;
            let Some(Value::ResourceRef(rid)) = object.fields.get("Resource") else {
                return Err("its texture has no resource".to_owned());
            };
            let resource = resources
                .get(rid)
                .ok_or("its texture resource is not listed")?;
            let bytes = store
                .bytes(resource)
                .map_err(|e| failure("texture resource", e))?;
            let header = TextureHeader::parse(&bytes).map_err(|e| failure("texture header", e))?;
            Ok::<_, String>((entry.name.clone(), header))
        })();
        match header {
            Ok((texture, header)) => headers.push((definition, size, name, texture, header)),
            Err(reason) => unresolved.push(Unresolved {
                definition,
                size,
                texture_name: name,
                reason,
            }),
        }
    }
    let chunk_ids: BTreeSet<[u8; 16]> = headers.iter().map(|(.., h)| h.chunk).collect();
    let chunks = install
        .chunk_records(&chunk_ids)
        .map_err(|e| failure("chunk list", e))?;
    let mut decoded: BTreeMap<[u8; 20], Decoded> = BTreeMap::new();
    let mut images: BTreeMap<String, BTreeMap<Size, IndexImage>> = BTreeMap::new();
    let mut total = 0usize;
    for (definition, size, name, texture, header) in headers {
        let image = (|| {
            let sha1 = chunks
                .get(&header.chunk)
                .ok_or("its pixel chunk is not listed")?;
            if let Some(done) = decoded.get(sha1) {
                return Ok::<_, String>(done.image.clone());
            }
            if decoded.len() >= MAX_IMAGES {
                return Err("the image count exceeds its bound".into());
            }
            let pixels = store
                .record_bytes(sha1)
                .map_err(|e| failure("pixel chunk", e))?;
            let rgba =
                texture::decode(&header, &pixels, &limits).map_err(|e| failure("texture", e))?;
            let png = png(rgba.width, rgba.height, &rgba.rgba);
            total += png.len();
            if total > MAX_SET_BYTES {
                return Err("the image set exceeds its size bound".into());
            }
            let file = format!("{}.png", nfs_frostbite::hex(sha1));
            let image = IndexImage {
                file: file.clone(),
                width: rgba.width,
                height: rgba.height,
                format: Format::render_format_name(header.format)
                    .unwrap_or("unknown")
                    .to_owned(),
                texture: texture.clone(),
            };
            decoded.insert(
                *sha1,
                Decoded {
                    file,
                    png,
                    image: image.clone(),
                },
            );
            Ok(image)
        })();
        match image {
            Ok(image) => {
                images.entry(definition).or_default().insert(size, image);
            }
            Err(reason) => unresolved.push(Unresolved {
                definition,
                size,
                texture_name: name,
                reason,
            }),
        }
    }
    let index = Index {
        format: INDEX_FORMAT.into(),
        version: INDEX_VERSION,
        game_directory: game.to_path_buf(),
        cars: cars.len(),
        named,
        images,
        unresolved,
    };
    Ok((index, decoded.into_values().collect()))
}

pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Remove an unusable set, only when it holds nothing but set files.
fn remove_set(directory: &Path) -> Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|name| name == INDEX || is_image_file(name));
        if !entry.file_type()?.is_file() || !ours {
            return Err(Failure::refused(format!(
                "{} holds files the launcher did not write; remove it yourself.",
                directory.display()
            )));
        }
        files.push(entry.path());
    }
    for file in files {
        std::fs::remove_file(file)?;
    }
    std::fs::remove_dir(directory)?;
    Ok(())
}

/// What [`prepare`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub cached: Cached,
    /// Whether this call decoded the set (false when one was reused).
    pub built: bool,
}

/// The image set for content `fingerprint`: the published one when it
/// validates, otherwise decoded from the installation at `game` and
/// published by one rename. Never writes inside a content entry.
pub fn prepare(data: &Path, fingerprint: &str, game: &Path, limits: Limits) -> Result<Prepared> {
    if !records::is_lower_hex(fingerprint, 64) {
        return Err(Failure::refused(
            "Car images need a prepared content entry; run setup first.",
        ));
    }
    let content = data.join("artifacts").join("content");
    if !ordinary(&data.join("artifacts"), true) || !ordinary(&content, true) {
        return Err(Failure::no_session(
            "This data directory has no prepared content; run setup first.",
        ));
    }
    let root = root(data);
    let directory = root.join(fingerprint);
    if let Ok(Some(cached)) = load(data, fingerprint) {
        return Ok(Prepared {
            cached,
            built: false,
        });
    }
    let (index, decoded) = decode_all(game, limits)
        .map_err(|why| Failure::refused(format!("Car images could not be read: {why}.")))?;
    match std::fs::symlink_metadata(&root) {
        Ok(meta) if paths::is_link(&meta) || !meta.is_dir() => {
            return Err(Failure::refused(
                "The car image directory must be an ordinary directory.",
            ));
        }
        Ok(_) => {}
        Err(_) => std::fs::create_dir(&root)?,
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temporary = root.join(format!(".build-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&temporary)?;
    let written = (|| -> Result<()> {
        for image in &decoded {
            write_synced(&temporary.join(&image.file), &image.png)?;
        }
        write_synced(&temporary.join(INDEX), &serde_json::to_vec_pretty(&index)?)?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(error);
    }
    if std::fs::symlink_metadata(&directory).is_ok()
        && let Err(error) = remove_set(&directory)
    {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, &directory) {
        let _ = std::fs::remove_dir_all(&temporary);
        // Another process may have published the same set first.
        return match load(data, fingerprint) {
            Ok(Some(cached)) => Ok(Prepared {
                cached,
                built: false,
            }),
            _ => Err(error.into()),
        };
    }
    Ok(Prepared {
        cached: Cached { directory, index },
        built: true,
    })
}

#[cfg(test)]
pub(crate) mod tests;

/// Counts per image size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SizeCounts {
    pub medium: usize,
    pub large: usize,
}

impl SizeCounts {
    fn from_iter(sizes: impl Iterator<Item = Size>) -> Self {
        let mut counts = Self::default();
        for size in sizes {
            match size {
                Size::Medium => counts.medium += 1,
                Size::Large => counts.large += 1,
            }
        }
        counts
    }
}

/// `car-images --json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CarImagesReport {
    pub version: u32,
    /// The image set's directory.
    pub directory: PathBuf,
    /// The content entry the set belongs to.
    pub fingerprint: String,
    /// The installation the images were decoded from.
    pub game_directory: PathBuf,
    /// Whether this run decoded the set; `false` when a complete set was
    /// reused without reading the installation.
    pub built: bool,
    /// Car definitions the installation lists.
    pub cars: usize,
    /// Car items that name an image, per size.
    pub named: SizeCounts,
    /// Named images decoded, per size.
    pub resolved: SizeCounts,
    /// PNG files in the set; cars sharing a texture share a file.
    pub files: usize,
    /// Decoded images by their source texture format, such as `BC1_UNORM`.
    pub formats: BTreeMap<String, usize>,
    /// Named images that could not be decoded, and why.
    pub unresolved: Vec<Unresolved>,
}

impl CarImagesReport {
    pub const VERSION: u32 = 1;

    pub fn new(prepared: &Prepared, fingerprint: &str) -> Self {
        let index = &prepared.cached.index;
        let decoded = || index.images.values().flat_map(BTreeMap::iter);
        let mut formats = BTreeMap::new();
        for (_, image) in decoded() {
            *formats.entry(image.format.clone()).or_insert(0) += 1;
        }
        let files: BTreeSet<&str> = decoded().map(|(_, image)| image.file.as_str()).collect();
        Self {
            version: Self::VERSION,
            directory: prepared.cached.directory.clone(),
            fingerprint: fingerprint.to_owned(),
            game_directory: index.game_directory.clone(),
            built: prepared.built,
            cars: index.cars,
            named: SizeCounts {
                medium: index.named.get(&Size::Medium).copied().unwrap_or(0),
                large: index.named.get(&Size::Large).copied().unwrap_or(0),
            },
            resolved: SizeCounts::from_iter(decoded().map(|(size, _)| *size)),
            files: files.len(),
            formats,
            unresolved: index.unresolved.clone(),
        }
    }
}
