// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Constructed installations with a synthetic tiled world map; no game data.

use super::*;
use crate::profile::images::tests::{FINGERPRINT, data_with_content, pixels};
use nfs_frostbite::synthetic::{
    Asset, CasSuperbundle, Field, PartitionWriter, Resource, bc1_solid, texture_header,
    write_superbundles,
};

const RED: u16 = 0xF800;
const GREEN: u16 = 0x07E0;
const BLUE: u16 = 0x001F;
const WHITE: u16 = 0xFFFF;
const BC3: u32 = 60;
const BC4: u32 = 62;
const TILE: u32 = 8;

fn bc3(color: u16, size: u16) -> Vec<u8> {
    let blocks = usize::from(size / 4).pow(2);
    (0..blocks)
        .flat_map(|_| {
            let mut block = vec![255, 255, 0, 0, 0, 0, 0, 0];
            block.extend_from_slice(&bc1_solid(color));
            block
        })
        .collect()
}

/// One tile entry and the texture it imports.
#[derive(Clone)]
struct MapTile {
    level: u32,
    column: u32,
    row: u32,
    /// Top-left corner of `NormalizedRect`.
    rect: (f32, f32),
    format: u32,
    size: u16,
    pixels: Vec<u8>,
    /// Whether the pixel chunk is listed.
    listed: bool,
    /// Whether the entry imports a texture at all.
    textured: bool,
}

fn tile(level: u32, column: u32, row: u32, side: u32, color: u16) -> MapTile {
    MapTile {
        level,
        column,
        row,
        rect: (column as f32 / side as f32, row as f32 / side as f32),
        format: BC3,
        size: TILE as u16,
        pixels: bc3(color, TILE as u16),
        listed: true,
        textured: true,
    }
}

/// Level 1: 2x2 tiles (red, green / blue, white); level 2: one red tile.
fn standard_tiles() -> Vec<MapTile> {
    vec![
        tile(1, 0, 0, 2, RED),
        tile(1, 1, 0, 2, GREEN),
        tile(1, 0, 1, 2, BLUE),
        tile(1, 1, 1, 2, WHITE),
        tile(2, 0, 0, 1, RED),
    ]
}

fn vec3(x: f32, y: f32, z: f32) -> Field {
    Field::Struct(
        "Vec3",
        vec![
            ("x", Field::Float(x)),
            ("y", Field::Float(y)),
            ("z", Field::Float(z)),
        ],
    )
}

/// An `OBBData` with the given right axis, centre and half extents.
fn obb(
    right: (f32, f32, f32),
    centre: (f32, f32, f32),
    half: (f32, f32, f32),
) -> Vec<(&'static str, Field)> {
    let forward = if right == (1.0, 0.0, 0.0) {
        vec3(0.0, 0.0, 1.0)
    } else {
        vec3(-right.2, 0.0, right.0)
    };
    vec![
        ("Flags", Field::UInt(33_554_432)),
        (
            "Transform",
            Field::Struct(
                "LinearTransform",
                vec![
                    ("right", vec3(right.0, right.1, right.2)),
                    ("up", vec3(0.0, 1.0, 0.0)),
                    ("forward", forward),
                    ("trans", vec3(centre.0, centre.1, centre.2)),
                ],
            ),
        ),
        ("HalfExtents", vec3(half.0, half.1, half.2)),
    ]
}

/// The supported build's two boxes: the 8192 m map box and a smaller one.
fn standard_boxes() -> Vec<Vec<(&'static str, Field)>> {
    vec![
        obb((1.0, 0.0, 0.0), (0.0, 740.0, 0.0), (8192.0, 650.0, 8192.0)),
        obb(
            (1.0, 0.0, 0.0),
            (-3080.688, 911.5, 1341.4),
            (1873.1, 650.0, 1620.3),
        ),
    ]
}

struct Map {
    tiles: Vec<MapTile>,
    /// (level, tiles per side)
    levels: Vec<(u32, u32)>,
    /// `OBBData` objects of the UI map world part; `None` leaves it out.
    boxes: Option<Vec<Vec<(&'static str, Field)>>>,
    /// Whether the tiled texture asset is written.
    tiled: bool,
}

impl Default for Map {
    fn default() -> Self {
        Self {
            tiles: standard_tiles(),
            levels: vec![(1, 2), (2, 1)],
            boxes: Some(standard_boxes()),
            tiled: true,
        }
    }
}

fn write_map(root: &Path, map: &Map) {
    let mut assets = Vec::new();
    let mut resources = Vec::new();
    let mut chunks = Vec::new();
    let mut rows = Vec::new();
    for (i, tile) in map.tiles.iter().enumerate() {
        let n = 0x10 + i as u8;
        let (partition, instance, chunk) = ([n; 16], [n ^ 0xFF; 16], [n.wrapping_add(0x80); 16]);
        let name = format!(
            "{TILE_PREFIX}win32/worldmap_mip{}_{}_{}_{i}",
            tile.level, tile.column, tile.row
        );
        let rid = 0x2000 + i as u64;
        assets.push(Asset {
            name: name.clone(),
            partition: PartitionWriter::default().build(
                partition,
                "TextureAsset",
                instance,
                vec![("Resource", Field::ResourceRef(rid))],
            ),
        });
        let mip0 = if tile.format == BC4 {
            32
        } else {
            tile.pixels.len() as u32
        };
        resources.push(Resource {
            name,
            rid,
            res_type: 0x6BDE_20BA,
            meta: vec![0x0B, 0, 0, 0],
            data: texture_header(tile.format, tile.size, tile.size, chunk, mip0),
        });
        if tile.listed {
            chunks.push((chunk, tile.pixels.clone()));
        }
        let side = map
            .levels
            .iter()
            .find(|(level, _)| *level == tile.level)
            .map_or(1, |(_, side)| *side) as f32;
        rows.push(vec![
            (
                "NormalizedRect",
                Field::Struct(
                    "Vec4",
                    vec![
                        ("x", Field::Float(tile.rect.0)),
                        ("y", Field::Float(tile.rect.1)),
                        ("z", Field::Float(tile.rect.0 + 1.0 / side)),
                        ("w", Field::Float(tile.rect.1 + 1.0 / side)),
                    ],
                ),
            ),
            (
                "Texture",
                if tile.textured {
                    Field::Import(partition, instance)
                } else {
                    Field::RawPointer(0)
                },
            ),
            ("Id", Field::UInt(i as u32)),
            ("MipLevel", Field::UInt(tile.level)),
            ("HorizontalIndex", Field::UInt(tile.column)),
            ("VerticalIndex", Field::UInt(tile.row)),
            ("ParentIndex", Field::Int(-1)),
            ("FirstChildIndex", Field::Int(-1)),
            ("ThirdChildIndex", Field::Int(-1)),
        ]);
    }
    let infos = map
        .levels
        .iter()
        .map(|(level, side)| {
            vec![
                ("MipLevel", Field::UInt(*level)),
                ("TileIndexOffset", Field::UInt(0)),
                ("SideTileCount", Field::UInt(*side)),
                ("FullTextureSize", Field::UInt(side * TILE)),
            ]
        })
        .collect();
    if map.tiled {
        assets.push(Asset {
            name: TILED_TEXTURE.into(),
            partition: PartitionWriter::default().build(
                [0xA0; 16],
                "UITiledTextureAsset",
                [0xA1; 16],
                vec![
                    (
                        "Name",
                        Field::CString("UI/Menu/WorldMapTiledTexture".into()),
                    ),
                    ("TileSize", Field::UInt(TILE)),
                    ("LowestMipLevel", Field::UInt(1)),
                    ("HighestMipLevel", Field::UInt(2)),
                    ("FullTextureSize", Field::UInt(4 * TILE)),
                    ("Tiles", Field::Structs("UITiledTextureTile", rows)),
                    (
                        "MipLevelInfos",
                        Field::Structs("UITiledTextureMipLevelInfo", infos),
                    ),
                ],
            ),
        });
    }
    if let Some(boxes) = &map.boxes {
        assets.push(Asset {
            name: WORLD_PART.into(),
            partition: PartitionWriter::default().build_many(
                [0xB0; 16],
                "WorldPartData",
                [0xB1; 16],
                vec![("Name", Field::CString("levels/Genesis01/UIMap".into()))],
                boxes
                    .iter()
                    .map(|fields| ("OBBData", fields.clone()))
                    .collect(),
            ),
        });
    }
    write_superbundles(
        root,
        b"exe",
        &[
            CasSuperbundle {
                name: MAP_SUPERBUNDLE,
                bundle: "win32/levels/genesis01/ui",
                assets: &assets,
                resources: &resources,
                chunks: &[],
            },
            CasSuperbundle {
                name: "Win32/chunks0",
                bundle: "win32/chunks0",
                assets: &[],
                resources: &[],
                chunks: &chunks,
            },
        ],
        &[],
    )
    .unwrap();
}

fn first_pixel(path: &Path) -> [u8; 4] {
    let (width, height, rgba) = pixels(path);
    assert_eq!((width, height), (TILE, TILE));
    rgba[..4].try_into().unwrap()
}

fn reasons(index: &Index) -> Vec<(Tile, &str)> {
    index
        .missing
        .iter()
        .map(|m| {
            (
                Tile {
                    level: m.level,
                    column: m.column,
                    row: m.row,
                },
                m.reason.as_str(),
            )
        })
        .collect()
}

#[test]
fn world_map_tiles_decode_publish_once_and_report_the_world_box() {
    let (data, game) = data_with_content("map");
    write_map(&game, &Map::default());
    let entry = data.join("artifacts").join("content").join(FINGERPRINT);
    let prepared = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    assert!(prepared.built);
    let report = MapImagesReport::new(&prepared, FINGERPRINT);
    crate::contract::testing::validate("map-images", &report);
    assert_eq!(report.directory, root(&data).join(FINGERPRINT));
    assert_eq!(report.asset, "UI/Menu/WorldMapTiledTexture");
    assert_eq!((report.tile_size, report.full_size), (TILE, 4 * TILE));
    assert_eq!(report.file_pattern, "{level}-{column}-{row}.png");
    assert_eq!(
        report.levels,
        [
            Level {
                level: 1,
                tiles_per_side: 2,
                size: 16,
                tiles: 4
            },
            Level {
                level: 2,
                tiles_per_side: 1,
                size: 8,
                tiles: 1
            },
        ]
    );
    assert_eq!(
        report.world,
        Some(World {
            min_x: -8192.0,
            min_z: -8192.0,
            max_x: 8192.0,
            max_z: 8192.0
        }),
        "the largest box"
    );
    assert_eq!(report.files, 5);
    assert_eq!(
        report.formats,
        BTreeMap::from([("BC3_UNORM".to_owned(), 5)])
    );
    assert!(
        report.missing.is_empty() && report.warnings.is_empty(),
        "{report:?}"
    );

    // Columns from the left, rows from the top.
    let dir = &report.directory;
    assert_eq!(first_pixel(&dir.join("1-0-0.png")), [255, 0, 0, 255]);
    assert_eq!(first_pixel(&dir.join("1-1-0.png")), [0, 255, 0, 255]);
    assert_eq!(first_pixel(&dir.join("1-0-1.png")), [0, 0, 255, 255]);
    assert_eq!(first_pixel(&dir.join("1-1-1.png")), [255, 255, 255, 255]);
    assert_eq!(first_pixel(&dir.join("2-0-0.png")), [255, 0, 0, 255]);
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "1-0-0.png",
            "1-0-1.png",
            "1-1-0.png",
            "1-1-1.png",
            "2-0-0.png",
            "index.json"
        ]
    );
    // Nothing is written inside the content entry.
    assert_eq!(std::fs::read_dir(&entry).unwrap().count(), 1);

    // A complete set is reused without reading the installation.
    std::fs::remove_dir_all(&game).unwrap();
    let again = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    assert!(!again.built);
    assert_eq!(again.cached, prepared.cached);
    assert_eq!(load(&data, FINGERPRINT).unwrap(), Some(prepared.cached));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn tiles_that_cannot_be_placed_or_decoded_are_listed_as_missing() {
    let (data, game) = data_with_content("map-missing");
    let mut tiles = standard_tiles();
    // Its rectangle says column 1, its index column 0.
    tiles[1].column = 0;
    tiles[1].row = 1;
    tiles.push(MapTile {
        listed: false,
        ..tile(2, 0, 0, 1, GREEN)
    });
    tiles.push(tile(1, 2, 0, 2, RED));
    tiles.push(tile(3, 0, 0, 1, RED));
    tiles.push(tile(1, 1, 1, 2, RED));
    tiles[3].listed = false;
    tiles[2].size = 16;
    tiles[2].pixels = bc3(BLUE, 16);
    // Level 4 holds a tile without a texture and one in an undecoded format.
    tiles.push(MapTile {
        textured: false,
        ..tile(4, 0, 0, 2, RED)
    });
    tiles.push(MapTile {
        format: BC4,
        pixels: vec![0; 32],
        ..tile(4, 1, 0, 2, RED)
    });
    write_map(
        &game,
        &Map {
            tiles,
            levels: vec![(1, 2), (2, 1), (4, 2)],
            ..Map::default()
        },
    );
    let prepared = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    let index = &prepared.cached.index;
    let at = |level, column, row| Tile { level, column, row };
    assert_eq!(index.tiles, [at(1, 0, 0), at(2, 0, 0)]);
    // Sorted by level, row and column; a place's own reasons in the order
    // they were found.
    let found = reasons(index);
    assert_eq!(
        found[..8],
        [
            (at(1, 2, 0), "it lies outside its detail level"),
            (
                at(1, 0, 1),
                "its rectangle does not match its column and row"
            ),
            (at(1, 0, 1), "its texture is not a tile-sized 2D texture"),
            (at(1, 1, 1), "it is listed twice"),
            (at(1, 1, 1), "its pixel chunk is not listed"),
            (at(2, 0, 0), "it is listed twice"),
            (at(3, 0, 0), "its detail level is not listed"),
            (at(4, 0, 0), "it has no texture"),
        ]
    );
    assert_eq!(found.len(), 9);
    assert_eq!(found[8].0, at(4, 1, 0));
    assert!(
        found[8].1.starts_with("texture: unsupported content"),
        "{}",
        found[8].1
    );
    let counts: Vec<usize> = index.levels.iter().map(|l| l.tiles).collect();
    assert_eq!(counts, [1, 1, 0]);
    crate::contract::testing::validate("map-images", &MapImagesReport::new(&prepared, FINGERPRINT));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn a_missing_or_rotated_world_box_leaves_the_map_without_one() {
    for (name, boxes, why) in [
        (
            "map-no-world",
            None,
            "has no levels/genesis01/ui/layer1_uimap",
        ),
        (
            "map-rotated",
            Some(vec![obb(
                (0.0, 0.0, 1.0),
                (0.0, 0.0, 0.0),
                (8192.0, 650.0, 8192.0),
            )]),
            "has no unrotated box",
        ),
        ("map-empty-part", Some(vec![]), "has no unrotated box"),
    ] {
        let (data, game) = data_with_content(name);
        write_map(
            &game,
            &Map {
                boxes,
                ..Map::default()
            },
        );
        let prepared = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
        let index = &prepared.cached.index;
        assert_eq!(index.world, None, "{name}");
        assert_eq!(index.warnings.len(), 1, "{name}");
        assert!(
            index.warnings[0].contains(why),
            "{name}: {:?}",
            index.warnings
        );
        assert_eq!(index.tiles.len(), 5, "{name}: the tiles still decode");
        crate::contract::testing::validate(
            "map-images",
            &MapImagesReport::new(&prepared, FINGERPRINT),
        );
        std::fs::remove_dir_all(data).unwrap();
    }
}

#[test]
fn damaged_sets_are_rebuilt_and_foreign_files_refused() {
    let (data, game) = data_with_content("map-damaged");
    write_map(&game, &Map::default());
    let first = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    let dir = first.cached.directory.clone();

    std::fs::remove_file(dir.join("1-1-1.png")).unwrap();
    let why = load(&data, FINGERPRINT).unwrap_err();
    assert!(why.contains("missing or invalid tile"), "{why}");
    let rebuilt = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    assert!(rebuilt.built);
    assert!(dir.join("1-1-1.png").is_file());

    std::fs::write(dir.join(INDEX), b"{ not json").unwrap();
    assert!(load(&data, FINGERPRINT).is_err());
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    let refused = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap_err();
    assert!(
        refused.message.contains("did not write"),
        "{}",
        refused.message
    );
    assert!(dir.join("notes.txt").is_file(), "never removed");
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn prepare_refuses_without_content_or_a_world_map() {
    let (data, game) = data_with_content("map-refused");
    write_map(
        &game,
        &Map {
            tiled: false,
            ..Map::default()
        },
    );
    let error = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap_err();
    assert!(
        error
            .message
            .contains("has no ui/menu/worldmaptiledtexture"),
        "{}",
        error.message
    );
    assert!(!root(&data).exists(), "nothing is written");

    let error = prepare(&data, "not-a-fingerprint", &game, Limits::default()).unwrap_err();
    assert!(
        error.message.contains("run setup first"),
        "{}",
        error.message
    );
    let empty = crate::testing::scratch("map-no-content");
    let error = prepare(&empty, FINGERPRINT, &game, Limits::default()).unwrap_err();
    assert_eq!(error.exit, crate::exit::Exit::NoSession);
    assert_eq!(load(&empty, FINGERPRINT).unwrap(), None);

    // A level whose tiles do not add up to its size stops the set.
    let fields = Fields(vec![
        ("TileSize".into(), Value::UInt32(8)),
        ("FullTextureSize".into(), Value::UInt32(32)),
        (
            "MipLevelInfos".into(),
            Value::Array(vec![Value::Struct(Fields(vec![
                ("MipLevel".into(), Value::UInt32(1)),
                ("SideTileCount".into(), Value::UInt32(2)),
                ("FullTextureSize".into(), Value::UInt32(20)),
            ]))]),
        ),
        ("Tiles".into(), Value::Array(vec![])),
    ]);
    let why = layout(&fields, &mut Vec::new()).err().unwrap();
    assert_eq!(why, "a detail level does not fit its tiles");
    for dir in [data, empty] {
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn layout_bounds_hold() {
    let level = |side: u32| {
        Value::Struct(Fields(vec![
            ("MipLevel".into(), Value::UInt32(side)),
            ("SideTileCount".into(), Value::UInt32(side)),
            ("FullTextureSize".into(), Value::UInt32(side * 8)),
        ]))
    };
    let fields = |infos: Vec<Value>, tiles: usize, tile_size: u32| {
        Fields(vec![
            ("TileSize".into(), Value::UInt32(tile_size)),
            ("FullTextureSize".into(), Value::UInt32(1 << 16)),
            ("MipLevelInfos".into(), Value::Array(infos)),
            (
                "Tiles".into(),
                Value::Array(vec![Value::Struct(Fields::default()); tiles]),
            ),
        ])
    };
    let error = |f: Fields| layout(&f, &mut Vec::new()).err().unwrap();
    assert!(layout(&fields(vec![level(1)], MAX_TILES, 8), &mut Vec::new()).is_ok());
    assert!(error(fields(vec![level(1)], MAX_TILES + 1, 8)).contains("tiles exceed"));
    assert!(error(fields((1..=9).map(level).collect(), 0, 8)).contains("levels exceed"));
    assert!(error(fields(vec![], 0, 8)).contains("levels exceed"));
    assert!(error(fields(vec![level(MAX_SIDE_TILES + 1)], 0, 8)).contains("does not fit"));
    assert!(error(fields(vec![level(1), level(1)], 0, 8)).contains("listed twice"));
    assert!(error(fields(vec![level(1)], 0, MAX_TILE_SIZE + 1)).contains("tile size"));
    assert!(error(fields(vec![level(1)], 0, 0)).contains("tile size"));
}

#[test]
fn tile_file_names() {
    let tile = Tile {
        level: 1,
        column: 31,
        row: 7,
    };
    assert_eq!(tile.file(), "1-31-7.png");
    assert!(is_tile_file("1-31-7.png") && is_tile_file("0-0-0.png"));
    for name in [
        "1-31.png",
        "1-31-7.PNG",
        "01-1-1.png",
        "a-1-1.png",
        "1--1.png",
        "1-1-1-1.png",
        "12345-1-1.png",
        "index.json",
    ] {
        assert!(!is_tile_file(name), "{name}");
    }
}
