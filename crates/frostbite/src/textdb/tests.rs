// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{histogram, text_database};

fn latin() -> Vec<u8> {
    histogram(256, &[(0x81, 0x2019), (0xAE, 0x00AE), (0x80, 0x0002)])
}

#[test]
fn string_ids_hash_case_sensitively_from_all_ones() {
    assert_eq!(string_id_hash(""), u32::MAX);
    // Keys the supported installation's English database stores for these
    // two string IDs of one car item.
    assert_eq!(
        string_id_hash("ID_CAR_SUBARU_BRZPREMIUM_2014_LONG_NAME"),
        0x67e6_8853
    );
    assert_eq!(
        string_id_hash("ID_CAR_SUBARU_BRZPREMIUM_2014_SHORT_NAME"),
        0x742e_b593
    );
    assert_ne!(string_id_hash("ID_A"), string_id_hash("id_a"));
}

#[test]
fn strings_decode_through_the_single_byte_table() {
    let binary = text_database(
        "Global",
        &[
            ("ID_PLAIN", b"Test Car GT (2000)"),
            ("ID_MAPPED", b"Maker\x81s ENGINE\xae"),
            ("ID_LEAD", b"bad \x80 lead"),
            ("ID_UNMAPPED", b"bad \xff byte"),
            ("ID_CONTROL", b"bad \x01 control"),
            ("ID_EMPTY", b""),
        ],
    );
    let db = TextDatabase::parse(&binary, &latin(), &Limits::default()).unwrap();
    assert_eq!(db.name(), "Global");
    assert_eq!(db.len(), 6);
    assert!(db.is_single_byte());
    assert_eq!(
        db.get("ID_PLAIN"),
        Lookup::Found("Test Car GT (2000)".into())
    );
    assert_eq!(
        db.get("ID_MAPPED"),
        Lookup::Found("Maker\u{2019}s ENGINE\u{ae}".into())
    );
    assert_eq!(db.get("ID_EMPTY"), Lookup::Found(String::new()));
    for id in ["ID_LEAD", "ID_UNMAPPED", "ID_CONTROL"] {
        assert_eq!(db.get(id), Lookup::Undecodable, "{id}");
    }
    assert_eq!(db.get("ID_ABSENT"), Lookup::Missing);
    assert_eq!(db.get("id_plain"), Lookup::Missing);
}

#[test]
fn multi_byte_histograms_are_reported_not_guessed() {
    let binary = text_database("Global", &[("ID_NAME", b"\x8b\x10")]);
    let db =
        TextDatabase::parse(&binary, &histogram(1643, &[(0x8b, 2)]), &Limits::default()).unwrap();
    assert!(!db.is_single_byte());
    assert_eq!(db.get("ID_NAME"), Lookup::Undecodable);
    assert_eq!(db.get("ID_ABSENT"), Lookup::Missing);
}

#[test]
fn malformed_layouts_and_bounds_are_refused() {
    let binary = text_database("Global", &[("ID_A", b"a"), ("ID_B", b"b")]);
    let parse = |binary: &[u8], histogram: &[u8]| {
        TextDatabase::parse(binary, histogram, &Limits::default())
    };
    assert!(parse(&binary, &latin()).is_ok());
    for len in 0..binary.len() {
        assert!(
            parse(&binary[..len], &latin()).is_err(),
            "truncated at {len}"
        );
    }
    let mut wrong = binary.clone();
    wrong[0] ^= 1;
    assert!(parse(&wrong, &latin()).is_err(), "magic");
    let mut longer = binary.clone();
    longer.push(0);
    assert!(parse(&longer, &latin()).is_err(), "length field");
    // Keys must stay sorted and offsets inside the text region.
    let table = 8 + 12 + 128;
    let mut unsorted = binary.clone();
    let (first, second) = unsorted[table..table + 16].split_at_mut(8);
    first.swap_with_slice(second);
    assert!(parse(&unsorted, &latin()).is_err(), "unsorted");
    let mut outside = binary.clone();
    outside[table + 4..table + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse(&outside, &latin()).is_err(), "offset");
    let mut layout = binary.clone();
    layout[8 + 8..8 + 12].copy_from_slice(&0u32.to_le_bytes());
    assert!(parse(&layout, &latin()).is_err(), "text offset");
    let hist = latin();
    for len in 0..hist.len() {
        assert!(parse(&binary, &hist[..len]).is_err(), "histogram at {len}");
    }
    assert!(parse(&binary, &histogram(1643, &[])[..100]).is_err());
    let limits = Limits {
        max_elements: 1,
        ..Limits::default()
    };
    assert!(TextDatabase::parse(&binary, &latin(), &limits).is_err());
    let limits = Limits {
        max_asset_bytes: 64,
        ..Limits::default()
    };
    assert!(TextDatabase::parse(&binary, &latin(), &limits).is_err());
}

#[test]
fn an_unterminated_or_overlong_string_is_undecodable() {
    let long = vec![b'a'; MAX_STRING_BYTES + 1];
    let binary = text_database("Global", &[("ID_LONG", &long), ("ID_OK", b"ok")]);
    let db = TextDatabase::parse(&binary, &latin(), &Limits::default()).unwrap();
    assert_eq!(db.get("ID_LONG"), Lookup::Undecodable);
    assert_eq!(db.get("ID_OK"), Lookup::Found("ok".into()));
    let mut open = text_database("Global", &[("ID_A", b"abc")]);
    open.pop();
    let length = (open.len() - 8) as u32;
    open[4..8].copy_from_slice(&length.to_le_bytes());
    let db = TextDatabase::parse(&open, &latin(), &Limits::default()).unwrap();
    assert_eq!(db.get("ID_A"), Lookup::Undecodable);
}
