// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{Db, clear_header, leb};

#[test]
fn round_trip_of_every_type() {
    let blob = Db::Object(vec![
        ("name", Db::str("Win32/vehicles0")),
        ("cas", Db::Bool(true)),
        ("offset", Db::Int(-5)),
        ("size", Db::Long(1 << 40)),
        ("ratio", Db::Float(0.5)),
        ("exact", Db::Double(2.25)),
        ("id", Db::Guid(std::array::from_fn(|i| i as u8))),
        ("sha1", Db::Sha1(std::array::from_fn(|i| i as u8))),
        ("payload", Db::Blob(b"abc".to_vec())),
        (
            "bundles",
            Db::List(vec![
                Db::Object(vec![("id", Db::str("a"))]),
                Db::Object(vec![("id", Db::str("b"))]),
            ]),
        ),
    ])
    .record(None);
    let (value, used) = parse(&blob, &Limits::default()).unwrap();
    let value = value.unwrap();
    assert_eq!(used, blob.len());
    assert_eq!(
        value.get("name").and_then(Value::as_str),
        Some("Win32/vehicles0")
    );
    assert_eq!(value.get("cas").and_then(Value::as_bool), Some(true));
    assert_eq!(value.get("offset").and_then(Value::as_i64), Some(-5));
    assert_eq!(value.get("size").and_then(Value::as_i64), Some(1 << 40));
    assert_eq!(value.get("ratio"), Some(&Value::Float(0.5)));
    assert_eq!(value.get("exact"), Some(&Value::Double(2.25)));
    assert_eq!(
        value.get("id"),
        Some(&Value::Guid(std::array::from_fn(|i| i as u8)))
    );
    assert_eq!(
        value.get("sha1").and_then(Value::as_sha1),
        Some(std::array::from_fn(|i| i as u8))
    );
    assert_eq!(value.get("payload"), Some(&Value::Blob(b"abc".to_vec())));
    let ids: Vec<_> = value
        .get("bundles")
        .and_then(Value::as_list)
        .unwrap()
        .iter()
        .map(|b| b.get("id").and_then(Value::as_str).unwrap())
        .collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn truncated_and_unknown_types_fail() {
    let blob = Db::Object(vec![("id", Db::str("abc"))]).record(None);
    assert!(parse(&blob[..blob.len() - 3], &Limits::default()).is_err());
    assert!(matches!(
        parse(&[20, b'x', 0, 0], &Limits::default()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn container_size_mismatch_fails() {
    let mut body = Db::str("abc").record(Some("id"));
    body.push(0);
    let mut blob = vec![0x82];
    blob.extend(leb(body.len() + 1));
    blob.extend(&body);
    blob.extend([0, 0]);
    assert!(matches!(
        parse(&blob, &Limits::default()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn deobfuscation_rules() {
    let inner = Db::Object(vec![("name", Db::str("x"))]).record(None);
    let key: Vec<u8> = (0..KEY_BYTES).map(|i| (i * 7 + 3) as u8).collect();
    let mut keyed = vec![0u8; HEADER_BYTES];
    keyed[..4].copy_from_slice(&[0x00, 0xD1, 0xCE, 0x01]);
    keyed[KEY_AT..KEY_AT + KEY_BYTES].copy_from_slice(&key);
    keyed.extend(
        inner
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key[i % KEY_MODULUS] ^ 0x7B),
    );
    let (body, how) = deobfuscate(&keyed).unwrap();
    assert_eq!((how, &*body), (Obfuscation::Keyed, &inner[..]));
    let clear = clear_header(&inner);
    let (body, how) = deobfuscate(&clear).unwrap();
    assert_eq!((how, &*body), (Obfuscation::Clear, &inner[..]));
    let (body, how) = deobfuscate(&inner).unwrap();
    assert_eq!((how, &*body), (Obfuscation::None, &inner[..]));
    assert!(deobfuscate(&[0x00, 0xD1, 0xCE, 0x03, 1]).is_err());
}

#[test]
fn bounds_reject_size_depth_and_elements() {
    let blob = Db::Object(vec![("id", Db::str("abc"))]).record(None);
    let small = Limits {
        max_container_bytes: blob.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(parse(&blob, &small), Err(Error::Bound(_))));
    let mut nested = Db::List(vec![]);
    for _ in 0..5 {
        nested = Db::List(vec![nested]);
    }
    let shallow = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    assert!(matches!(
        parse(&nested.record(None), &shallow),
        Err(Error::Bound(_))
    ));
    let wide = Db::List((0..10).map(Db::Int).collect()).record(None);
    let narrow = Limits {
        max_elements: 9,
        ..Limits::default()
    };
    assert!(matches!(parse(&wide, &narrow), Err(Error::Bound(_))));
    assert!(parse(&wide, &Limits::default()).is_ok());
}

#[test]
fn overlong_length_and_unterminated_name_fail() {
    assert!(
        parse(
            &[
                0x87, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
            ],
            &Limits::default()
        )
        .is_err()
    );
    assert!(parse(&[0x07, b'n', b'o'], &Limits::default()).is_err());
}

#[test]
fn empty_root_and_load_requirement() {
    assert_eq!(parse(&[0], &Limits::default()).unwrap(), (None, 1));
    assert!(load(&[0], &Limits::default()).is_err());
}
