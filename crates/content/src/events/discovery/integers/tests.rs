// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;

fn id(index: usize) -> Identity {
    Identity::from(&Site {
        key: ("a".into(), index),
        ..Site::root("a")
    })
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "Blueprint",
        &[
            ("Objects", Value::Array((1..=7).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 910_586_961, local(2), 0),
                    edge(local(1), 915_182_239, local(3), 0),
                    edge(local(4), name("Level"), local(3), 0),
                    // A different property/store must not borrow the first binding.
                    edge(local(6), 910_586_961, local(7), 0),
                    edge(local(6), 915_182_239, local(8), 0),
                    edge(local(4), name("Score"), local(8), 0),
                    edge(local(1), (-1_772_170_871i32) as u32, local(9), 0),
                    edge(local(6), (-1_772_170_871i32) as u32, local(9), 0),
                ]),
            ),
        ],
    );
    for (i, class) in [
        (1, "IntPropertyPersistenceInterfaceEntityData"),
        (2, "IntParticipantPropertyEntityData"),
        (3, "PlayerIntStoreEntityData"),
        (6, "IntPropertyPersistenceInterfaceEntityData"),
        (7, "IntParticipantPropertyEntityData"),
        (8, "PlayerIntStoreEntityData"),
    ] {
        m.put("a", i, class, &[]);
    }
    m.put(
        "a",
        4,
        "PersistentTableUsingStoresEntityData",
        &[
            ("TableName", Value::String("ConstructedStats".into())),
            ("TableAsset", local(5)),
        ],
    );
    m.put("a", 9, "ParticipantManagerEntityData", &[]);
    m.put(
        "a",
        5,
        "PersistentTableAsset",
        &[
            ("TableName", Value::String("ConstructedStats".into())),
            ("HasSecondaryKey", Value::Bool(false)),
            (
                "Properties",
                Value::Array(
                    ["Level", "Score"]
                        .into_iter()
                        .map(|s| {
                            Value::Struct(Fields(vec![
                                ("Name".into(), Value::String(s.into())),
                                (
                                    "DataType".into(),
                                    Value::Enum {
                                        value: 0,
                                        name: Some("PersistentPropertyType_Int".into()),
                                    },
                                ),
                            ]))
                        })
                        .collect(),
                ),
            ),
        ],
    );
    m
}
fn read_selected(m: &mut Memory) -> Result<BTreeMap<Identity, Store>, Error> {
    read(
        &mut Graph::new(m),
        &[Site::root("a")],
        &BTreeSet::from([id(2), id(7)]),
    )
}
fn field<'a>(m: &'a mut Memory, index: usize, name: &str) -> &'a mut Value {
    &mut m
        .0
        .get_mut(&("a".into(), index))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == name)
        .unwrap()
        .1
}
#[test]
fn binds_exact_property_and_table_column_without_sharing_adjacent_values() {
    assert_eq!(
        read_selected(&mut fixture()).unwrap(),
        BTreeMap::from([
            (
                id(2),
                Store {
                    table: "ConstructedStats".into(),
                    column: "Level".into(),
                    player_manager: id(9)
                }
            ),
            (
                id(7),
                Store {
                    table: "ConstructedStats".into(),
                    column: "Score".into(),
                    player_manager: id(9)
                }
            ),
        ])
    );
    let mut m = fixture();
    let Value::Array(links) = field(&mut m, 0, "LinkConnections") else {
        panic!()
    };
    links.reverse();
    assert_eq!(
        read_selected(&mut m).unwrap(),
        read_selected(&mut fixture()).unwrap()
    );
    assert!(
        read(
            &mut Graph::new(&mut m),
            &[Site::root("a")],
            &BTreeSet::from([id(9)])
        )
        .is_err()
    );
}
#[test]
fn rejects_ambiguous_writers_columns_wrong_types_and_missing_table_ownership() {
    for extra in [
        edge(local(1), 910_586_961, local(2), 0),
        edge(local(4), name("Score"), local(3), 0),
    ] {
        let mut m = fixture();
        let Value::Array(links) = field(&mut m, 0, "LinkConnections") else {
            panic!()
        };
        links.push(extra);
        assert!(read_selected(&mut m).is_err());
    }
    for replacement in [Value::Bool(true), Value::String("not a flag".into())] {
        let mut m = fixture();
        *field(&mut m, 5, "HasSecondaryKey") = replacement;
        assert!(read_selected(&mut m).is_err());
    }
    let mut m = fixture();
    let Value::Array(columns) = field(&mut m, 5, "Properties") else {
        panic!()
    };
    let Value::Struct(first) = &mut columns[0] else {
        panic!()
    };
    first.0.iter_mut().find(|(n, _)| n == "DataType").unwrap().1 = Value::Enum {
        value: 1,
        name: Some("PersistentPropertyType_Float".into()),
    };
    assert!(read_selected(&mut m).is_err());
    let mut m = fixture();
    let Value::Array(links) = field(&mut m, 0, "LinkConnections") else {
        panic!()
    };
    links.remove(2);
    assert!(read_selected(&mut m).is_err());
}
