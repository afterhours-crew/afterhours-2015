// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The game's world map, decoded from the player's installation into the
//! data folder by `map-images` for a map view. No image is shipped,
//! committed or embedded.
//!
//! In the supported build the pause-menu world map is the
//! `UITiledTextureAsset` [`TILED_TEXTURE`]: square tiles at several detail
//! levels. Level 0 would be the full `FullTextureSize`; each level halves
//! it, and the build ships levels 1 to 3 (8192, 4096 and 2048 pixels
//! across, in 256-pixel tiles). A `Tiles` entry gives its `MipLevel`,
//! column (`HorizontalIndex`), row (`VerticalIndex`) and `NormalizedRect`,
//! and imports a `TextureAsset` under [`TILE_PREFIX`] whose first mip is
//! decoded as for the car images.
//!
//! The map covers the box of the `OBBData` in [`WORLD_PART`] (the UI map's
//! world part): in the supported build 8192 m either side of the origin in
//! x and z. A world position maps to u = (x - min_x) / (max_x - min_x) and
//! v = (z - min_z) / (max_z - min_z), from the image's top-left corner, so
//! +z points down the image. That orientation was checked by drawing the
//! road network over the decoded map.
//!
//! Tiles go to `artifacts/content/map-images/<content fingerprint>/` as
//! `<level>-<column>-<row>.png` with an `index.json`, published by one
//! rename like the car images.

use crate::{
    exit::{Failure, Result},
    paths,
    profile::{
        images::{ordinary, png, write_synced},
        names::failure,
    },
    records,
};
use nfs_frostbite::{
    Limits,
    ebx::{Fields, Partition, Pointer, Value},
    install::{EbxEntry, Installation, Store},
    texture::{self, Format, TYPE_2D, TextureHeader},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Directory under `artifacts/content` that holds the tile sets.
pub const DIRECTORY: &str = "map-images";
pub const INDEX: &str = "index.json";
pub const INDEX_FORMAT: &str = "afterhours-map-images";
pub const INDEX_VERSION: u32 = 1;
/// The superbundle holding the world map, its tiles and the UI map layer.
pub const MAP_SUPERBUNDLE: &str = "Win32/levels/genesis01/genesis01";
pub const TILED_TEXTURE: &str = "ui/menu/worldmaptiledtexture";
/// Assets a tile may import; the supported build's are all here.
pub const TILE_PREFIX: &str = "ui/menu/tiledtexture/";
pub const WORLD_PART: &str = "levels/genesis01/ui/layer1_uimap";
/// How a tile's file is named in the set's directory.
pub const FILE_PATTERN: &str = "{level}-{column}-{row}.png";
pub const MAX_INDEX_BYTES: u64 = 1 << 20;
/// Tile entries read from the tiled texture.
pub const MAX_TILES: usize = 4096;
/// Detail levels read from the tiled texture.
pub const MAX_LEVELS: usize = 8;
/// Tiles along one side of a level.
pub const MAX_SIDE_TILES: u32 = 128;
/// Largest tile edge in pixels.
pub const MAX_TILE_SIZE: u32 = 1024;
/// PNG bytes in one set.
pub const MAX_SET_BYTES: usize = 256 << 20;
/// How far a tile's rectangle may be from its column and row, in tiles.
const RECT_TOLERANCE: f32 = 1.0 / 64.0;

/// One detail level of the map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Level {
    /// The tiled texture's mip level: 0 is the full size, each level halves it.
    pub level: u32,
    /// Columns and rows of tiles.
    pub tiles_per_side: u32,
    /// Width and height of the whole level in pixels.
    pub size: u32,
    /// Tiles of this level in the set.
    pub tiles: usize,
}

/// The world box the map covers, in metres. A position maps to
/// u = (x - min_x) / (max_x - min_x) and v = (z - min_z) / (max_z - min_z)
/// from the image's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct World {
    pub min_x: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_z: f32,
}

/// A listed tile that could not be produced, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Missing {
    pub level: u32,
    pub column: u32,
    pub row: u32,
    pub reason: String,
}

/// One tile file of a set, as the index records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tile {
    pub level: u32,
    pub column: u32,
    pub row: u32,
}

impl Tile {
    pub fn file(self) -> String {
        format!("{}-{}-{}.png", self.level, self.column, self.row)
    }
}

/// `index.json` of a set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub format: String,
    pub version: u32,
    pub game_directory: PathBuf,
    /// The tiled texture's `Name`.
    pub asset: String,
    pub tile_size: u32,
    /// Width and height of level 0, which the build may not ship.
    pub full_size: u32,
    pub levels: Vec<Level>,
    pub world: Option<World>,
    pub tiles: Vec<Tile>,
    /// Decoded tiles by their source texture format, such as `BC3_UNORM`.
    pub formats: BTreeMap<String, usize>,
    pub missing: Vec<Missing>,
    pub warnings: Vec<String>,
}

/// A set read back from the data folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Cached {
    pub directory: PathBuf,
    pub index: Index,
}

/// `<data>/artifacts/content/map-images`.
pub fn root(data: &Path) -> PathBuf {
    data.join("artifacts").join("content").join(DIRECTORY)
}

/// `<level>-<column>-<row>.png` with decimal numbers.
fn is_tile_file(name: &str) -> bool {
    name.strip_suffix(".png").is_some_and(|stem| {
        let parts: Vec<&str> = stem.split('-').collect();
        parts.len() == 3
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.len() <= 4
                    && part.bytes().all(|b| b.is_ascii_digit())
                    && (part.len() == 1 || !part.starts_with('0'))
            })
    })
}

/// Read and validate a set's index, checking every file it names exists.
fn validate(directory: &Path) -> std::result::Result<Index, String> {
    let index_path = directory.join(INDEX);
    if !ordinary(directory, true) || !ordinary(&index_path, false) {
        return Err("the map set is incomplete".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&index_path)
        .and_then(|file| file.take(MAX_INDEX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err("the map index exceeds its bound".into());
    }
    let index: Index = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if index.format != INDEX_FORMAT || index.version != INDEX_VERSION {
        return Err("the map index has another format".into());
    }
    if index.tiles.len() > MAX_TILES || index.levels.len() > MAX_LEVELS {
        return Err("the map index exceeds its bound".into());
    }
    let levels: HashMap<u32, u32> = index
        .levels
        .iter()
        .map(|level| (level.level, level.tiles_per_side))
        .collect();
    for tile in &index.tiles {
        let inside = levels
            .get(&tile.level)
            .is_some_and(|side| tile.column < *side && tile.row < *side);
        if !inside || !ordinary(&directory.join(tile.file()), false) {
            return Err("the map index names a missing or invalid tile".into());
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

fn uint(fields: &Fields, name: &str) -> Option<u32> {
    match fields.get(name) {
        Some(Value::UInt32(value)) => Some(*value),
        _ => None,
    }
}

fn float(fields: Option<&Fields>, name: &str) -> Option<f32> {
    match fields?.get(name) {
        Some(Value::Float32(value)) if value.is_finite() => Some(*value),
        _ => None,
    }
}

fn vec3(fields: Option<&Fields>) -> Option<[f32; 3]> {
    Some([
        float(fields, "x")?,
        float(fields, "y")?,
        float(fields, "z")?,
    ])
}

/// The world box of the UI map's world part: its largest unrotated
/// `OBBData` in x and z. Rotated, empty or incomplete boxes are skipped.
fn world_box(store: &mut Store, entry: &EbxEntry) -> std::result::Result<World, String> {
    let objects = store
        .objects(entry)
        .map_err(|e| failure("map world part", e))?;
    let mut best: Option<(f32, World)> = None;
    for object in objects.iter().filter(|o| o.class == "OBBData") {
        let transform = object.fields.get("Transform").and_then(Value::as_fields);
        let axis = |name| {
            vec3(
                transform
                    .and_then(|t| t.get(name))
                    .and_then(Value::as_fields),
            )
        };
        let half = vec3(object.fields.get("HalfExtents").and_then(Value::as_fields));
        let (Some(right), Some(up), Some(forward), Some(trans), Some(half)) = (
            axis("right"),
            axis("up"),
            axis("forward"),
            axis("trans"),
            half,
        ) else {
            continue;
        };
        let unrotated =
            right == [1.0, 0.0, 0.0] && up == [0.0, 1.0, 0.0] && forward == [0.0, 0.0, 1.0];
        if !unrotated || half[0] <= 0.0 || half[2] <= 0.0 {
            continue;
        }
        let area = half[0] * half[2];
        if best.as_ref().is_none_or(|(largest, _)| area > *largest) {
            best = Some((
                area,
                World {
                    min_x: trans[0] - half[0],
                    min_z: trans[2] - half[2],
                    max_x: trans[0] + half[0],
                    max_z: trans[2] + half[2],
                },
            ));
        }
    }
    best.map(|(_, world)| world)
        .ok_or_else(|| "the map world part has no unrotated box".into())
}

/// One tile entry of the tiled texture.
struct Entry {
    tile: Tile,
    partition: [u8; 16],
    instance: [u8; 16],
}

/// One decoded tile before it is written.
struct Decoded {
    tile: Tile,
    png: Vec<u8>,
}

/// The levels and tile entries of the tiled texture, with tiles that cannot
/// be placed already in `missing`.
fn layout(
    fields: &Fields,
    missing: &mut Vec<Missing>,
) -> std::result::Result<(u32, u32, Vec<Level>, Vec<Entry>), String> {
    let tile_size = uint(fields, "TileSize")
        .filter(|size| (1..=MAX_TILE_SIZE).contains(size))
        .ok_or("the world map has no usable tile size")?;
    let full_size = uint(fields, "FullTextureSize").ok_or("the world map has no full size")?;
    let infos = fields
        .get("MipLevelInfos")
        .and_then(Value::as_array)
        .ok_or("the world map has no detail levels")?;
    if infos.is_empty() || infos.len() > MAX_LEVELS {
        return Err("the world map's detail levels exceed their bound".into());
    }
    let mut levels = Vec::new();
    for info in infos {
        let info = info
            .as_fields()
            .ok_or("a detail level is not a structure")?;
        let (Some(level), Some(side), Some(size)) = (
            uint(info, "MipLevel"),
            uint(info, "SideTileCount"),
            uint(info, "FullTextureSize"),
        ) else {
            return Err("a detail level is incomplete".into());
        };
        if side == 0 || side > MAX_SIDE_TILES || side.checked_mul(tile_size) != Some(size) {
            return Err("a detail level does not fit its tiles".into());
        }
        if levels.iter().any(|l: &Level| l.level == level) {
            return Err("a detail level is listed twice".into());
        }
        levels.push(Level {
            level,
            tiles_per_side: side,
            size,
            tiles: 0,
        });
    }
    levels.sort_by_key(|l| l.level);
    let tiles = fields
        .get("Tiles")
        .and_then(Value::as_array)
        .ok_or("the world map has no tiles")?;
    if tiles.len() > MAX_TILES {
        return Err("the world map's tiles exceed their bound".into());
    }
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for item in tiles {
        let Some(tile) = item.as_fields() else {
            continue;
        };
        let (Some(level), Some(column), Some(row)) = (
            uint(tile, "MipLevel"),
            uint(tile, "HorizontalIndex"),
            uint(tile, "VerticalIndex"),
        ) else {
            continue;
        };
        let place = Tile { level, column, row };
        let mut miss = |reason: &str| {
            missing.push(Missing {
                level,
                column,
                row,
                reason: reason.to_owned(),
            })
        };
        let Some(side) = levels
            .iter()
            .find(|l| l.level == level)
            .map(|l| l.tiles_per_side)
        else {
            miss("its detail level is not listed");
            continue;
        };
        if column >= side || row >= side {
            miss("it lies outside its detail level");
            continue;
        }
        let rect = tile.get("NormalizedRect").and_then(Value::as_fields);
        let corner = |name, index| {
            float(rect, name).is_some_and(|value| {
                (value * side as f32 - index as f32).abs() <= RECT_TOLERANCE * side as f32
            })
        };
        if !(corner("x", column) && corner("y", row)) {
            miss("its rectangle does not match its column and row");
            continue;
        }
        if !seen.insert(place) {
            miss("it is listed twice");
            continue;
        }
        let Some(Pointer::Import {
            partition,
            instance,
        }) = tile.get("Texture").and_then(Value::as_pointer)
        else {
            miss("it has no texture");
            continue;
        };
        entries.push(Entry {
            tile: place,
            partition,
            instance,
        });
    }
    Ok((tile_size, full_size, levels, entries))
}

/// The world map's tiles, decoded from the installation at `game`. A tile
/// that cannot be decoded becomes a [`Missing`] entry; errors that stop the
/// whole set are sentences.
fn decode_all(game: &Path, limits: Limits) -> std::result::Result<(Index, Vec<Decoded>), String> {
    let install = Installation::open(game, limits).map_err(|e| failure("installation", e))?;
    let mut store = Store::open(&install).map_err(|e| failure("archive catalogue", e))?;
    let listed = install
        .superbundle_ebx(MAP_SUPERBUNDLE)
        .map_err(|e| failure("map asset list", e))?;
    let by_name: HashMap<&str, &EbxEntry> = listed
        .iter()
        .map(|location| (location.entry.name.as_str(), &location.entry))
        .collect();
    let entry = by_name
        .get(TILED_TEXTURE)
        .ok_or_else(|| format!("the installation has no {TILED_TEXTURE}"))?;
    let objects = store.objects(entry).map_err(|e| failure("world map", e))?;
    let asset = objects
        .iter()
        .find(|object| object.class == "UITiledTextureAsset")
        .ok_or("the world map asset has no tiled texture")?;
    let name = asset
        .fields
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or(TILED_TEXTURE)
        .to_owned();
    let mut missing = Vec::new();
    let (tile_size, full_size, mut levels, entries) = layout(&asset.fields, &mut missing)?;

    let mut warnings = Vec::new();
    let world = match by_name.get(WORLD_PART) {
        None => Err(format!("the installation has no {WORLD_PART}")),
        Some(entry) => world_box(&mut store, entry),
    }
    .map_err(|why| warnings.push(format!("No world box for the map: {why}.")))
    .ok();

    // Partition GUIDs of the imported tile textures, from the tile assets only.
    let partitions: BTreeSet<[u8; 16]> = entries.iter().map(|e| e.partition).collect();
    let mut textures: HashMap<[u8; 16], &EbxEntry> = HashMap::new();
    for location in &listed {
        if textures.len() == partitions.len() {
            break;
        }
        if !location.entry.name.starts_with(TILE_PREFIX) {
            continue;
        }
        let data = store
            .bytes(&location.entry)
            .map_err(|e| failure("map tile", e))?;
        let guid = Partition::parse(&data, &limits)
            .map_err(|e| failure("map tile", e))?
            .guid;
        if partitions.contains(&guid) {
            textures.insert(guid, &location.entry);
        }
    }
    let resources: HashMap<u64, EbxEntry> = install
        .superbundle_res(MAP_SUPERBUNDLE)
        .map_err(|e| failure("resource list", e))?
        .into_iter()
        .map(|location| (location.rid, location.entry))
        .collect();
    // Texture headers, then the chunks they name.
    let mut headers = Vec::new();
    for entry in entries {
        let header = (|| {
            let texture = textures
                .get(&entry.partition)
                .ok_or("its texture asset is not listed")?;
            let objects = store
                .objects(texture)
                .map_err(|e| failure("tile texture", e))?;
            let object = objects
                .iter()
                .find(|object| object.guid == Some(entry.instance))
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
            if header.kind != TYPE_2D
                || u32::from(header.width) != tile_size
                || u32::from(header.height) != tile_size
            {
                return Err("its texture is not a tile-sized 2D texture".to_owned());
            }
            Ok::<_, String>(header)
        })();
        match header {
            Ok(header) => headers.push((entry.tile, header)),
            Err(reason) => missing.push(Missing {
                level: entry.tile.level,
                column: entry.tile.column,
                row: entry.tile.row,
                reason,
            }),
        }
    }
    let chunk_ids: BTreeSet<[u8; 16]> = headers.iter().map(|(_, h)| h.chunk).collect();
    let chunks = install
        .chunk_records(&chunk_ids)
        .map_err(|e| failure("chunk list", e))?;
    let mut decoded = Vec::new();
    let mut formats = BTreeMap::new();
    let mut total = 0usize;
    for (tile, header) in headers {
        let image = (|| {
            let sha1 = chunks
                .get(&header.chunk)
                .ok_or("its pixel chunk is not listed")?;
            let pixels = store
                .record_bytes(sha1)
                .map_err(|e| failure("pixel chunk", e))?;
            let rgba =
                texture::decode(&header, &pixels, &limits).map_err(|e| failure("texture", e))?;
            Ok::<_, String>(png(rgba.width, rgba.height, &rgba.rgba))
        })();
        match image {
            Ok(png) => {
                total += png.len();
                if total > MAX_SET_BYTES {
                    return Err("the map exceeds its size bound".into());
                }
                *formats
                    .entry(
                        Format::render_format_name(header.format)
                            .unwrap_or("unknown")
                            .to_owned(),
                    )
                    .or_insert(0) += 1;
                decoded.push(Decoded { tile, png });
            }
            Err(reason) => missing.push(Missing {
                level: tile.level,
                column: tile.column,
                row: tile.row,
                reason,
            }),
        }
    }
    for level in &mut levels {
        level.tiles = decoded
            .iter()
            .filter(|d| d.tile.level == level.level)
            .count();
    }
    let mut tiles: Vec<Tile> = decoded.iter().map(|d| d.tile).collect();
    tiles.sort();
    missing.sort_by_key(|m| (m.level, m.row, m.column));
    let index = Index {
        format: INDEX_FORMAT.into(),
        version: INDEX_VERSION,
        game_directory: game.to_path_buf(),
        asset: name,
        tile_size,
        full_size,
        levels,
        world,
        tiles,
        formats,
        missing,
        warnings,
    };
    Ok((index, decoded))
}

/// Remove an unusable set, only when it holds nothing but set files.
fn remove_set(directory: &Path) -> Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|name| name == INDEX || is_tile_file(name));
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
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub cached: Cached,
    /// Whether this call decoded the set (false when one was reused).
    pub built: bool,
}

/// The map set for content `fingerprint`: the published one when it
/// validates, otherwise decoded from the installation at `game` and
/// published by one rename. Never writes inside a content entry.
pub fn prepare(data: &Path, fingerprint: &str, game: &Path, limits: Limits) -> Result<Prepared> {
    if !records::is_lower_hex(fingerprint, 64) {
        return Err(Failure::refused(
            "The map needs a prepared content entry; run setup first.",
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
        .map_err(|why| Failure::refused(format!("The map could not be read: {why}.")))?;
    match std::fs::symlink_metadata(&root) {
        Ok(meta) if paths::is_link(&meta) || !meta.is_dir() => {
            return Err(Failure::refused(
                "The map image directory must be an ordinary directory.",
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
        for tile in &decoded {
            write_synced(&temporary.join(tile.tile.file()), &tile.png)?;
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

/// `map-images --json`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct MapImagesReport {
    pub version: u32,
    /// The tile set's directory.
    pub directory: PathBuf,
    /// The content entry the set belongs to.
    pub fingerprint: String,
    /// The installation the map was decoded from.
    pub game_directory: PathBuf,
    /// Whether this run decoded the set; `false` when a complete set was
    /// reused without reading the installation.
    pub built: bool,
    /// The tiled texture's name in the game.
    pub asset: String,
    /// Width and height of every tile in pixels.
    pub tile_size: u32,
    /// Width and height of level 0, which the build may not ship.
    pub full_size: u32,
    /// How a tile's file is named in `directory`: `{level}-{column}-{row}.png`,
    /// columns from the left and rows from the top, counting from 0.
    pub file_pattern: String,
    /// Detail levels, the most detailed first.
    pub levels: Vec<Level>,
    /// The world box the map covers; `null` when the installation has none
    /// this program can read.
    pub world: Option<World>,
    /// PNG files in the set.
    pub files: usize,
    /// Decoded tiles by their source texture format, such as `BC3_UNORM`.
    pub formats: BTreeMap<String, usize>,
    /// Listed tiles that could not be decoded, and why.
    pub missing: Vec<Missing>,
    pub warnings: Vec<String>,
}

impl MapImagesReport {
    pub const VERSION: u32 = 1;

    pub fn new(prepared: &Prepared, fingerprint: &str) -> Self {
        let index = &prepared.cached.index;
        Self {
            version: Self::VERSION,
            directory: prepared.cached.directory.clone(),
            fingerprint: fingerprint.to_owned(),
            game_directory: index.game_directory.clone(),
            built: prepared.built,
            asset: index.asset.clone(),
            tile_size: index.tile_size,
            full_size: index.full_size,
            file_pattern: FILE_PATTERN.into(),
            levels: index.levels.clone(),
            world: index.world,
            files: index.tiles.len(),
            formats: index.formats.clone(),
            missing: index.missing.clone(),
            warnings: index.warnings.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
