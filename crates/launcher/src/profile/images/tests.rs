// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Constructed installations with synthetic textures; no game data.

use super::*;
use nfs_frostbite::synthetic::{
    Asset, CasSuperbundle, Field, PartitionWriter, Resource, bc1_solid, texture_header,
    write_superbundles,
};

/// A car item: definition GUID, medium and large texture names.
pub(crate) type Car<'a> = (&'a str, &'a str, &'a str);

pub(crate) const FINGERPRINT: &str =
    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const RED: u16 = 0xF800;
const BLUE: u16 = 0x001F;

fn guid(text: &str) -> [u8; 16] {
    super::super::content::guid(text).unwrap()
}

/// One texture of the constructed installation.
struct Texture {
    id: &'static str,
    format: u32,
    pixels: Vec<u8>,
    listed: bool,
    whole: bool,
}

fn bc1_8x8(color: u16) -> Vec<u8> {
    (0..4).flat_map(|_| bc1_solid(color)).collect()
}

fn bc3_8x8(color: u16) -> Vec<u8> {
    (0..4)
        .flat_map(|_| {
            let mut block = vec![255, 255, 0, 0, 0, 0, 0, 0];
            block.extend_from_slice(&bc1_solid(color));
            block
        })
        .collect()
}

fn vec2(x: f32, y: f32) -> Field {
    Field::Struct("Vec2", vec![("x", Field::Float(x)), ("y", Field::Float(y))])
}

/// Textures for the medium and large mappings: `A_MED` (BC1 red, also used
/// by `B_MED`), `A_LARGE` (BC3 blue), `PART_MED` (a partial rectangle),
/// `BC4_MED` (an undecoded format) and `NOCHUNK_MED` (no listed chunk).
fn textures() -> (Vec<Texture>, Vec<Texture>) {
    let texture = |id, format, pixels, listed, whole| Texture {
        id,
        format,
        pixels,
        listed,
        whole,
    };
    (
        vec![
            texture("A_MED", 54, bc1_8x8(RED), true, true),
            texture("PART_MED", 54, bc1_8x8(BLUE), true, false),
            texture("BC4_MED", 62, vec![0; 32], true, true),
            texture("NOCHUNK_MED", 54, bc1_8x8(BLUE), false, true),
        ],
        vec![texture("A_LARGE", 60, bc3_8x8(BLUE), true, true)],
    )
}

/// An installation with the given car items and the textures above.
pub(crate) fn write_installation(root: &Path, cars: &[Car<'_>]) {
    let mut items: Vec<Asset> = cars
        .iter()
        .enumerate()
        .map(|(i, (definition, medium, large))| Asset {
            name: format!("items/car_{i}"),
            partition: PartitionWriter::default().build(
                [i as u8 + 1; 16],
                "RaceVehicleItemData",
                guid(definition),
                vec![
                    ("MediumTextureName", Field::CString((*medium).into())),
                    ("LargeTextureName", Field::CString((*large).into())),
                ],
            ),
        })
        .collect();
    let (medium, large) = textures();
    let mut resources = Vec::new();
    let mut chunks = Vec::new();
    let mut n = 0x40u8;
    for (mapping_name, list) in [(MEDIUM_MAPPING, &medium), (LARGE_MAPPING, &large)] {
        let mut outputs = Vec::new();
        for texture in list {
            n += 1;
            let (partition, instance, chunk) =
                ([n; 16], [n ^ 0xFF; 16], [n.wrapping_add(0x80); 16]);
            let name = format!("ui/textures/test/{}", texture.id.to_lowercase());
            let rid = 0x1000 + u64::from(n);
            items.push(Asset {
                name: name.clone(),
                partition: PartitionWriter::default().build(
                    partition,
                    "TextureAsset",
                    instance,
                    vec![("Resource", Field::ResourceRef(rid))],
                ),
            });
            let mip0 = if texture.format == 62 {
                32
            } else {
                texture.pixels.len() as u32
            };
            resources.push(Resource {
                name,
                rid,
                res_type: 0x6BDE_20BA,
                meta: vec![0x0B, 0, 0, 0],
                data: texture_header(texture.format, 8, 8, chunk, mip0),
            });
            if texture.listed {
                chunks.push((chunk, texture.pixels.clone()));
            }
            let max = if texture.whole { 1.0 } else { 0.5 };
            outputs.push(vec![
                ("Id", Field::CString(texture.id.into())),
                ("Texture", Field::Import(partition, instance)),
                ("Min", vec2(0.0, 0.0)),
                ("Max", vec2(max, max)),
                ("Streaming", Field::Bool(true)),
            ]);
        }
        if mapping_name == MEDIUM_MAPPING {
            // `B_MED` shares `A_MED`'s texture.
            let mut shared = outputs[0].clone();
            shared[0] = ("Id", Field::CString("B_MED".into()));
            outputs.push(shared);
        }
        n += 1;
        items.push(Asset {
            name: mapping_name.into(),
            partition: PartitionWriter::default().build(
                [n; 16],
                "UITextureMappingAsset",
                [n ^ 0xFF; 16],
                vec![(
                    "Output",
                    Field::Structs("UITextureMappingOutputEntry", outputs),
                )],
            ),
        });
    }
    write_superbundles(
        root,
        b"exe",
        &[
            CasSuperbundle {
                name: ITEM_SUPERBUNDLE,
                bundle: "win32/levels/frontend/frontend",
                assets: &items,
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

/// A data folder with one prepared content entry, and an installation.
pub(crate) fn data_with_content(name: &str) -> (PathBuf, PathBuf) {
    let data = crate::testing::scratch(name);
    let entry = data.join("artifacts").join("content").join(FINGERPRINT);
    std::fs::create_dir_all(&entry).unwrap();
    std::fs::write(entry.join("manifest.json"), b"{}").unwrap();
    let game = crate::testing::scratch(&format!("{name}-game"));
    (data, game)
}

const A: &str = "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";
const B: &str = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";
const C: &str = "0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c";
const D: &str = "0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d";
const E: &str = "0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e";

fn sample(game: &Path) {
    write_installation(
        game,
        &[
            (A, "A_MED", "A_LARGE"),
            (B, "B_MED", ""),
            (C, "PART_MED", "MISSING_LARGE"),
            (D, "BC4_MED", ""),
            (E, "NOCHUNK_MED", ""),
        ],
    );
}

/// The decoded pixels of a PNG this module wrote.
fn pixels(path: &Path) -> (u32, u32, Vec<u8>) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    let idat = 8 + 25;
    let length = u32::from_be_bytes(bytes[idat..idat + 4].try_into().unwrap()) as usize;
    assert_eq!(&bytes[idat + 4..idat + 8], b"IDAT");
    let raw =
        miniz_oxide::inflate::decompress_to_vec_zlib(&bytes[idat + 8..idat + 8 + length]).unwrap();
    let rgba = raw
        .chunks_exact(width as usize * 4 + 1)
        .flat_map(|row| {
            assert_eq!(row[0], 0, "unfiltered rows");
            row[1..].to_vec()
        })
        .collect();
    (width, height, rgba)
}

#[test]
fn images_resolve_through_the_mappings_decode_and_publish_once() {
    let (data, game) = data_with_content("images");
    sample(&game);
    let entry = data.join("artifacts").join("content").join(FINGERPRINT);
    let prepared = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    assert!(prepared.built);
    let report = CarImagesReport::new(&prepared, FINGERPRINT);
    crate::contract::testing::validate("car-images", &report);
    assert_eq!(report.cars, 5);
    assert_eq!(
        (report.named, report.resolved),
        (
            SizeCounts {
                medium: 5,
                large: 2
            },
            SizeCounts {
                medium: 2,
                large: 1
            }
        )
    );
    assert_eq!(report.files, 2, "A and B share one texture");
    assert_eq!(
        report.formats,
        BTreeMap::from([("BC1_UNORM".into(), 2), ("BC3_UNORM".into(), 1)])
    );
    let reasons: BTreeMap<(&str, Size), &str> = report
        .unresolved
        .iter()
        .map(|u| ((u.definition.as_str(), u.size), u.reason.as_str()))
        .collect();
    assert_eq!(reasons.len(), 4, "{reasons:?}");
    assert_eq!(reasons[&(C, Size::Medium)], "maps to part of a texture");
    assert_eq!(reasons[&(C, Size::Large)], "not in the texture mapping");
    assert!(reasons[&(D, Size::Medium)].contains("texture"));
    assert_eq!(reasons[&(E, Size::Medium)], "its pixel chunk is not listed");
    // The set sits beside the content entry, not inside it.
    assert_eq!(prepared.cached.directory, root(&data).join(FINGERPRINT));
    assert_eq!(std::fs::read_dir(&entry).unwrap().count(), 1);
    let images = prepared.cached.get(A).unwrap();
    let medium = images.medium.unwrap();
    assert_eq!(
        (medium.width, medium.height, medium.source_format.as_str()),
        (8, 8, "BC1_UNORM")
    );
    let (width, height, rgba) = pixels(&medium.path);
    assert_eq!((width, height), (8, 8));
    assert!(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255])
    );
    let large = images.large.unwrap();
    let (_, _, rgba) = pixels(&large.path);
    assert!(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 255, 255])
    );
    let shared = prepared.cached.get(B).unwrap();
    assert_eq!(shared.medium.unwrap().path, medium.path);
    assert_eq!(shared.large, None);
    assert_eq!(prepared.cached.get(C), None, "nothing decoded for C");
    // A complete set is reused, never rewritten.
    let again = prepare(&data, FINGERPRINT, &game.join("absent"), Limits::default()).unwrap();
    assert!(!again.built);
    assert_eq!(again.cached, prepared.cached);
    assert_eq!(load(&data, FINGERPRINT).unwrap(), Some(prepared.cached));
    assert_eq!(load(&data, &"f".repeat(64)).unwrap(), None);
    std::fs::remove_dir_all(data).unwrap();
    std::fs::remove_dir_all(game).unwrap();
}

#[test]
fn damaged_sets_are_rebuilt_and_foreign_files_are_left_alone() {
    let (data, game) = data_with_content("images-damaged");
    sample(&game);
    let prepared = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    let file = prepared.cached.get(A).unwrap().medium.unwrap().path;
    std::fs::remove_file(&file).unwrap();
    assert!(
        load(&data, FINGERPRINT).is_err(),
        "a listed file is missing"
    );
    let rebuilt = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap();
    assert!(rebuilt.built && file.exists());
    std::fs::remove_file(&file).unwrap();
    std::fs::write(rebuilt.cached.directory.join("notes.txt"), b"mine").unwrap();
    let refused = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap_err();
    assert_eq!(refused.exit, crate::exit::Exit::Refused);
    assert!(rebuilt.cached.directory.join("notes.txt").exists());
    // An oversized or foreign index is not read.
    let directory = &rebuilt.cached.directory;
    std::fs::remove_file(directory.join("notes.txt")).unwrap();
    std::fs::write(
        directory.join(INDEX),
        vec![b' '; MAX_INDEX_BYTES as usize + 1],
    )
    .unwrap();
    assert!(load(&data, FINGERPRINT).is_err());
    std::fs::write(
        directory.join(INDEX),
        br#"{"format":"other","version":1,"game_directory":"","cars":0,"named":{},"images":{},"unresolved":[]}"#,
    )
    .unwrap();
    assert!(load(&data, FINGERPRINT).is_err());
    std::fs::remove_dir_all(data).unwrap();
    std::fs::remove_dir_all(game).unwrap();
}

#[test]
fn sets_need_prepared_content_and_a_readable_installation() {
    let (data, game) = data_with_content("images-preconditions");
    let refused = prepare(&data, "not-a-fingerprint", &game, Limits::default()).unwrap_err();
    assert_eq!(refused.exit, crate::exit::Exit::Refused);
    let empty = crate::testing::scratch("images-no-content");
    let absent = prepare(&empty, FINGERPRINT, &game, Limits::default()).unwrap_err();
    assert_eq!(absent.exit, crate::exit::Exit::NoSession);
    let unreadable = prepare(&data, FINGERPRINT, &game, Limits::default()).unwrap_err();
    assert!(
        unreadable.message.contains("could not be read"),
        "{unreadable}"
    );
    assert!(!root(&data).join(FINGERPRINT).exists(), "nothing published");
    for dir in [data, empty, game] {
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn png_files_carry_valid_chunks() {
    let rgba = [1, 2, 3, 4, 5, 6, 7, 8];
    let png = png(2, 1, &rgba);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(&png[16..29], [0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082, "the well-known IEND CRC");
    assert_eq!(
        &png[png.len() - 8..],
        [b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]
    );
    let ihdr_crc = u32::from_be_bytes(png[29..33].try_into().unwrap());
    assert_eq!(ihdr_crc, crc32(&[&png[12..29]]));
}
