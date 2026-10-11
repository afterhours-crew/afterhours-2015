// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_heat2::{Encoder, Kind, Limits, Member, Schema, Type, TypeId, decode_with_schema};

const MAP: [u8; 3] = [0x8e, 0x88, 0x77];
const FIELD: [u8; 3] = [0xd3, 0x9c, 0x25];
const TYPES: &[Type<'_>] = &[
    Type::Scalar(Kind::Integer),
    Type::Struct(&[Member {
        tag: FIELD,
        ty: TypeId(0),
    }]),
    Type::List(TypeId(1)),
    Type::Map {
        key: TypeId(0),
        value: TypeId(2),
    },
    Type::Struct(&[Member {
        tag: MAP,
        ty: TypeId(3),
    }]),
];

fn encode(w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
    w.integer_struct_list_map(MAP, &[(7, vec![Some(5), None]), (8, vec![])], |w, value| {
        if let Some(value) = value {
            w.integer(FIELD, *value)?;
        }
        Ok(())
    })
}

#[test]
fn independent_nested_bytes_preserve_empty_lists_and_structs() {
    // Two keys. Key7 has [{TYPE=5},{}]; key8 has []. Both headers are3.
    let golden = [
        0x8e, 0x88, 0x77, 5, 0, 3, 2, 7, 3, 2, 0xd3, 0x9c, 0x25, 0, 5, 0, 0, 8, 3, 0,
    ];
    let schema = Schema::new(TYPES).unwrap();
    let mut w = Encoder::new(Limits::default());
    encode(&mut w).unwrap();
    assert_eq!(w.finish_with_schema(schema, TypeId(4)).unwrap(), golden);
    let decoded = decode_with_schema(&golden, Limits::default(), schema, TypeId(4)).unwrap();
    assert_eq!(decoded.stats().values, 8);
    for end in 1..golden.len() {
        assert!(decode_with_schema(&golden[..end], Limits::default(), schema, TypeId(4)).is_err());
    }
    let mut empty = Encoder::new(Limits::default());
    empty
        .integer_struct_list_map(MAP, &[] as &[(u32, Vec<()>)], |_, _| unreachable!())
        .unwrap();
    assert_eq!(
        empty.finish_with_schema(schema, TypeId(4)).unwrap(),
        [0x8e, 0x88, 0x77, 5, 0, 3, 0]
    );
}

#[test]
fn nested_budgets_are_exact_and_failure_poisons_writer() {
    let exact = Limits {
        max_bytes: 20,
        max_values: 8,
        max_depth: 3,
        max_collection: 2,
        ..Limits::default()
    };
    let schema = Schema::new(TYPES).unwrap();
    let mut w = Encoder::new(exact);
    encode(&mut w).unwrap();
    assert_eq!(w.finish_with_schema(schema, TypeId(4)).unwrap().len(), 20);
    for limits in [
        Limits {
            max_bytes: 19,
            ..exact
        },
        Limits {
            max_values: 7,
            ..exact
        },
        Limits {
            max_depth: 2,
            ..exact
        },
        Limits {
            max_collection: 1,
            ..exact
        },
    ] {
        let mut w = Encoder::new(limits);
        let error = encode(&mut w).unwrap_err();
        assert_eq!(w.integer(FIELD, 0).unwrap_err(), error);
        assert_eq!(w.finish_with_schema(schema, TypeId(4)).unwrap_err(), error);
    }
}
