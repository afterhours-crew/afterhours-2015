// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;
const ROOT: u32 = 42;
fn realm() -> Value {
    Value::Enum {
        value: 1,
        name: Some("Realm_Server".into()),
    }
}
fn declaration(ty: u32) -> Value {
    Value::Struct(Fields(vec![
        ("Id".into(), Value::Int32(91)),
        ("LinkTypeHash".into(), Value::UInt32(ty)),
        ("Realm".into(), realm()),
    ]))
}
fn memory() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array((1..=5).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), score::CONTEXT, local(2), 0),
                    edge(local(2), score::PARTICIPANT, local(3), 0),
                    edge(local(3), ROOT, local(4), 0),
                    edge(local(2), score::KEEPERS, local(5), 91),
                    edge(local(2), score::KEEPERS, local(5), 91),
                ]),
            ),
        ],
    );
    m.put(
        "a",
        1,
        "ScoreEntityData",
        &[("Score", Value::Int32(23)), ("Realm", realm())],
    );
    m.put("a", 2, "ScoreContextEntityData", &[("Realm", realm())]);
    m.put(
        "a",
        3,
        "ParticipantLogicParticipantProviderEntityData",
        &[("Realm", realm())],
    );
    m.put("a", 4, "ParticipantLogicEntityData", &[]);
    m.put(
        "a",
        5,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[("Links", Value::Array(vec![declaration(KEEPER_TYPE)]))],
    );
    m
}
#[test]
fn default_context_owner_and_ordered_duplicate_keeper_occurrences_are_retained() {
    let rows = read(&mut memory(), "a", ROOT).unwrap();
    assert_eq!(rows.len(), 1);
    let n = &rows[0];
    assert_eq!(
        (
            n.identity.index,
            n.default,
            n.context.identity.index,
            n.context.participant_provider.index
        ),
        (1, 23, 2, 3)
    );
    assert_eq!(n.context.keepers.len(), 2);
    assert_eq!(n.context.keepers[0], n.context.keepers[1]);
    assert_eq!(
        (
            n.context.keepers[0].identity.index,
            n.context.keepers[0].field
        ),
        (5, 91)
    );
}
#[test]
fn missing_ambiguous_context_and_foreign_participant_are_rejected() {
    for (source, port, target) in [(1, score::CONTEXT, 3), (3, ROOT, 2)] {
        let mut m = memory();
        let object = m.0.get_mut(&("a".into(), 0)).unwrap();
        let Value::Array(links) = &mut object.fields.0[1].1 else {
            panic!()
        };
        let at = if source == 1 { 0 } else { 2 };
        links[at] = edge(local(source), port, local(target), 0);
        assert!(read(&mut m, "a", ROOT).is_err());
    }
    for duplicate in [false, true] {
        let mut m = memory();
        let object = m.0.get_mut(&("a".into(), 0)).unwrap();
        let Value::Array(links) = &mut object.fields.0[1].1 else {
            panic!()
        };
        if duplicate {
            links.push(links[0].clone());
        } else {
            links.remove(0);
        }
        assert!(read(&mut m, "a", ROOT).is_err());
    }
}
#[test]
fn channel_type_missing_duplicate_and_client_only_declarations_fail() {
    let mut client = declaration(KEEPER_TYPE);
    let Value::Struct(fields) = &mut client else {
        panic!()
    };
    fields.0[2].1 = Value::Enum {
        value: 0,
        name: Some("Realm_Client".into()),
    };
    for rows in [
        vec![],
        vec![declaration(7)],
        vec![declaration(KEEPER_TYPE); 2],
        vec![client],
    ] {
        let mut m = memory();
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[("Links", Value::Array(rows))],
        );
        assert!(read(&mut m, "a", ROOT).is_err());
    }
}
#[test]
fn signed_defaults_are_preserved_but_wrong_types_and_unconstructed_targets_fail() {
    for default in [-10, 0, i32::MAX] {
        let mut m = memory();
        m.put(
            "a",
            1,
            "ScoreEntityData",
            &[("Score", Value::Int32(default)), ("Realm", realm())],
        );
        assert_eq!(read(&mut m, "a", ROOT).unwrap()[0].default, default);
    }
    let mut m = memory();
    m.put(
        "a",
        1,
        "ScoreEntityData",
        &[("Score", Value::UInt32(23)), ("Realm", realm())],
    );
    assert!(read(&mut m, "a", ROOT).is_err());
    let mut m = memory();
    let object = m.0.get_mut(&("a".into(), 0)).unwrap();
    let Value::Array(nodes) = &mut object.fields.0[0].1 else {
        panic!()
    };
    nodes.remove(1);
    assert!(read(&mut m, "a", ROOT).is_err());
    assert!(read(&mut memory(), "a", 0).is_err());
}
