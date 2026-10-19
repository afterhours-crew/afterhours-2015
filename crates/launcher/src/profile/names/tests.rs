// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Constructed installations: no game data, only the layout the reader expects.

use super::*;
use nfs_frostbite::synthetic::{
    Asset, CasSuperbundle, Field, InlineSuperbundle, PartitionWriter, histogram, text_database,
    write_superbundles,
};

/// One car item: its definition GUID and its long, medium and short name IDs.
pub(crate) type Car<'a> = (&'a str, [&'a str; 3]);

const BINARY: [u8; 16] = [0xB1; 16];
const HISTOGRAM: [u8; 16] = [0xB2; 16];
const JA_BINARY: [u8; 16] = [0xC1; 16];
const JA_HISTOGRAM: [u8; 16] = [0xC2; 16];

fn guid(text: &str) -> [u8; 16] {
    super::super::content::guid(text).unwrap()
}

fn car_item(index: u8, definition: &str, ids: [&str; 3]) -> Asset {
    let ui = vec![
        ("TextureID", Field::CString("texture".into())),
        ("LongNameId", Field::CString(ids[0].into())),
        ("MediumNameId", Field::CString(ids[1].into())),
        ("ShortNameId", Field::CString(ids[2].into())),
        ("DescriptionId", Field::CString(String::new())),
    ];
    Asset {
        name: format!("items/car_{index}"),
        partition: PartitionWriter::default().build(
            [index; 16],
            "RaceVehicleItemData",
            guid(definition),
            vec![("UI", Field::Struct("UiMetaData", ui))],
        ),
    }
}

fn text_asset(locale: &str, binary: ([u8; 16], u32), histogram: ([u8; 16], u32)) -> Asset {
    Asset {
        name: format!("localization/textdbs/{locale}"),
        partition: PartitionWriter::default().build(
            [locale.as_bytes()[0]; 16],
            "FsUITextDatabase",
            [locale.as_bytes()[1]; 16],
            vec![
                (
                    "Name",
                    Field::CString(format!("Localization/TextDbs/{locale}")),
                ),
                ("BinaryChunk", Field::Guid(binary.0)),
                ("BinaryChunkSize", Field::UInt(binary.1)),
                ("HistogramChunk", Field::Guid(histogram.0)),
                ("HistogramChunkSize", Field::UInt(histogram.1)),
            ],
        ),
    }
}

/// An installation with the given car items and English strings, plus a
/// part item, a car outside `items/` and a Japanese multi-byte database.
/// `size_error` is added to the declared English binary chunk size.
pub(crate) fn write_installation(
    root: &Path,
    cars: &[Car<'_>],
    strings: &[(&str, &[u8])],
    size_error: u32,
) {
    let mut items: Vec<Asset> = cars
        .iter()
        .enumerate()
        .map(|(i, (definition, ids))| car_item(i as u8 + 1, definition, *ids))
        .collect();
    items.push(Asset {
        name: "items/part".into(),
        partition: PartitionWriter::default().build(
            [0x70; 16],
            "AirFilterItemData",
            [0x71; 16],
            vec![("Quantity", Field::Int(1))],
        ),
    });
    let mut outside = car_item(0x72, &"72".repeat(16), ["ID_OUTSIDE", "", ""]);
    outside.name = "vehicles/not_an_item".into();
    items.push(outside);
    let binary = text_database("Global", strings);
    let latin = histogram(256, &[(0x81, 0x2019)]);
    let ja_binary = text_database("Global", &[("ID_JA", b"\x8b\x10")]);
    let ja_histogram = histogram(1643, &[(0x8b, 2)]);
    let texts = [
        text_asset(
            "en_us",
            (BINARY, binary.len() as u32 + size_error),
            (HISTOGRAM, latin.len() as u32),
        ),
        text_asset(
            "ja_jp",
            (JA_BINARY, ja_binary.len() as u32),
            (JA_HISTOGRAM, ja_histogram.len() as u32),
        ),
    ];
    write_superbundles(
        root,
        b"exe",
        &[
            CasSuperbundle {
                name: ITEM_SUPERBUNDLE,
                bundle: "win32/levels/frontend/frontend",
                assets: &items,
            },
            CasSuperbundle {
                name: TEXT_SUPERBUNDLE,
                bundle: "win32/default_settings_win32",
                assets: &texts,
            },
        ],
        &[
            InlineSuperbundle {
                name: "Win32/loc/en",
                chunks: &[(BINARY, binary), (HISTOGRAM, latin)],
            },
            InlineSuperbundle {
                name: "Win32/loc/ja",
                chunks: &[(JA_BINARY, ja_binary), (JA_HISTOGRAM, ja_histogram)],
            },
        ],
    )
    .unwrap();
}

const A: &str = "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";
const B: &str = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";

fn sample(root: &Path, size_error: u32) {
    write_installation(
        root,
        &[
            (A, ["ID_A_LONG", "ID_A_MEDIUM", "ID_A_SHORT"]),
            (B, ["ID_B_LONG", "ID_B_MEDIUM", "ID_B_SHORT_ABSENT"]),
        ],
        &[
            ("ID_A_LONG", b"Maker\x81s Car A (2000)"),
            ("ID_A_MEDIUM", b"Maker Car A"),
            ("ID_A_SHORT", b"Car A"),
            ("ID_B_LONG", b"Maker Car B (2001)"),
            ("ID_B_MEDIUM", b"Maker Car B"),
            ("ID_OUTSIDE", b"never read"),
        ],
        size_error,
    );
}

#[test]
fn names_come_from_car_items_and_the_locale_text_database() {
    let root = crate::testing::scratch("names");
    sample(&root, 0);
    let names = read(&root, "en_US", Limits::default()).unwrap();
    assert_eq!(names.locale, "en_US");
    assert_eq!(
        names.len(),
        2,
        "the part and the car outside items/ are skipped"
    );
    let a = names.get(&guid(A)).unwrap();
    assert_eq!(
        a.names,
        Names {
            long: Some("Maker\u{2019}s Car A (2000)".into()),
            medium: Some("Maker Car A".into()),
            short: Some("Car A".into()),
        }
    );
    assert!(a.unresolved.is_empty());
    let b = names.get(&guid(B)).unwrap();
    assert_eq!(b.names.long.as_deref(), Some("Maker Car B (2001)"));
    assert_eq!(b.names.short, None);
    assert_eq!(b.unresolved, ["ID_B_SHORT_ABSENT"]);
    assert!(names.get(&guid(&"72".repeat(16))).is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn absent_undecodable_or_inconsistent_databases_yield_no_names() {
    let root = crate::testing::scratch("names-missing");
    sample(&root, 0);
    let de = read(&root, "de_DE", Limits::default()).unwrap_err();
    assert!(de.contains("no de_DE text database"), "{de}");
    let ja = read(&root, "ja_JP", Limits::default()).unwrap_err();
    assert!(ja.contains("multi-byte"), "{ja}");
    assert!(read(&root.join("absent"), "en_US", Limits::default()).is_err());
    std::fs::remove_dir_all(&root).unwrap();
    sample(&root, 1);
    let size = read(&root, "en_US", Limits::default()).unwrap_err();
    assert!(size.contains("declared size"), "{size}");
    std::fs::remove_dir_all(&root).unwrap();
    // Text chunks absent from every loc superbundle.
    sample(&root, 0);
    std::fs::remove_file(root.join("Data/Win32/loc/en.toc")).unwrap();
    let chunks = read(&root, "en_US", Limits::default()).unwrap_err();
    assert!(chunks.contains("chunks are missing"), "{chunks}");
    std::fs::remove_dir_all(&root).unwrap();
    // No car items at all.
    write_installation(&root, &[], &[("ID_X", b"x")], 0);
    let none = read(&root, "en_US", Limits::default()).unwrap_err();
    assert!(none.contains("no car items"), "{none}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn item_and_car_counts_are_bounded() {
    let root = crate::testing::scratch("names-bounds");
    sample(&root, 0);
    let limits = Limits {
        max_entries: 2,
        ..Limits::default()
    };
    assert!(read(&root, "en_US", limits).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn locales_are_two_letters_underscore_two_letters() {
    for good in ["en_US", "de_DE", "pt_BR", "zh_TW"] {
        assert!(is_locale(good), "{good}");
    }
    for bad in ["", "en", "en-US", "EN_us", "en_USA", "../.."] {
        assert!(!is_locale(bad), "{bad}");
    }
}
