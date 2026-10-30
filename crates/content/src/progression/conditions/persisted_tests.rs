// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::tests::{program, rows};
use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;

fn realm(name: &str) -> Value {
    Value::Enum {
        value: 0,
        name: Some(name.into()),
    }
}
fn event(source: usize, output: u32, target: usize, input: u32) -> Value {
    Value::Struct(Fields(vec![
        ("Source".into(), local(source)),
        ("Target".into(), local(target)),
        (
            "SourceEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(output))])),
        ),
        (
            "TargetEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(input))])),
        ),
        (
            "TargetType".into(),
            realm("EventConnectionTargetType_Server"),
        ),
    ]))
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=4).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(3), 77, local(4), 0)]),
            ),
            (
                "PropertyConnections",
                Value::Array(vec![edge(local(3), 993810272, local(2), 177636)]),
            ),
            (
                "EventConnections",
                Value::Array(vec![
                    event(2, 193445336, 1, (-1541066415i32) as u32),
                    event(2, 2088725063, 1, 668205626),
                ]),
            ),
        ],
    );
    m.put(
        "a",
        1,
        "BoolEntityData",
        &[
            ("Realm", realm("Realm_Server")),
            ("DefaultValue", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        2,
        "CompareIntEntityData",
        &[
            ("Realm", realm("Realm_Server")),
            ("A", Value::Int32(0)),
            ("B", Value::Int32(0)),
            ("TriggerOnPropertyChange", Value::Bool(true)),
            ("TriggerOnStart", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        3,
        "IntValueData",
        &[
            ("Realm", realm("Realm_Server")),
            ("StableIdIsRequired", Value::Bool(true)),
        ],
    );
    m.put(
        "a",
        4,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(701))],
    );
    m
}

#[test]
fn integer_backed_boolean_retains_its_durable_owner() {
    // This is a comparison of a saved value, not an objective-completed flag
    // and not the latch's false startup default.
    let p = program(&mut fixture(), 1, VALUE).unwrap();
    assert_eq!(p.facts().count(), 1);
    assert_eq!(p.facts().next(), Some(Fact::PositiveInt(701)));
    assert!(p.evaluate(|_| None).is_err());
    assert!(!p.evaluate(|_| Some(false)).unwrap());
    assert!(p.evaluate(|_| Some(true)).unwrap());
    let mut reordered = fixture();
    rows(&mut reordered, "EventConnections").reverse();
    assert_eq!(program(&mut reordered, 1, VALUE).unwrap(), p);
}

#[test]
fn integer_projection_rejects_other_latch_writers_and_noncomplementary_comparisons() {
    for (source, output, target, input) in [
        (2, 193445336, 1, 668205626), // true and false from the same result
        (2, 193445336, 1, 123),       // unrelated event alters latch semantics
        (1, 193445336, 1, 668205626), // second writer
        (1, 193453899, 2, 5862146),   // event-driven comparison
    ] {
        let mut m = fixture();
        rows(&mut m, "EventConnections").push(event(source, output, target, input));
        assert!(program(&mut m, 1, VALUE).is_err());
    }
    for name in ["LinkConnections", "PropertyConnections", "EventConnections"] {
        let mut m = fixture();
        rows(&mut m, name).pop();
        assert!(program(&mut m, 1, VALUE).is_err());
    }
    for (node, name, value) in [
        (1, "DefaultValue", Value::Bool(true)),
        (2, "B", Value::Int32(1)),
        (2, "TriggerOnPropertyChange", Value::Bool(false)),
        (2, "Realm", realm("Realm_Client")),
        (3, "StableIdIsRequired", Value::Bool(false)),
    ] {
        let mut m = fixture();
        m.0.get_mut(&("a".into(), node))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == name)
            .unwrap()
            .1 = value;
        assert!(program(&mut m, 1, VALUE).is_err());
    }
}

#[test]
fn condition_owners_follow_scene_spawners_and_reject_foreign_roots() {
    use crate::graph::tests::ptr;
    let mut m = Memory::default();
    for root in ["a", "b"] {
        m.put(
            root,
            0,
            "ParticipantLogicBlueprint",
            &[("Objects", Value::Array(vec![]))],
        );
    }
    m.put(
        "c",
        0,
        "WorldPartData",
        &[("Objects", Value::Array(vec![local(1), local(2)]))],
    );
    m.put(
        "c",
        1,
        "ParticipantLogicSpawnerEntityData",
        &[("ParticipantLogicBlueprint", ptr("a", 0))],
    );
    m.put(
        "c",
        2,
        "ParticipantLogicSpawnerEntityData",
        &[("ParticipantLogicBlueprint", ptr("b", 0))],
    );
    assert_eq!(
        owner_roots(&mut Graph::new(&mut m), "a", None).unwrap(),
        BTreeSet::from(["a".into()])
    );
    assert_eq!(
        owner_roots(&mut Graph::new(&mut m), "a", Some("c")).unwrap(),
        BTreeSet::from(["a".into(), "b".into()])
    );
    m.put("b", 0, "UnrelatedBlueprint", &[]);
    assert!(owner_roots(&mut Graph::new(&mut m), "a", Some("c")).is_err());
    m.put("b", 0, "ParticipantLogicBlueprint", &[]);
    m.put("b", 1, "ParticipantLogicBlueprint", &[]);
    m.put(
        "c",
        2,
        "ParticipantLogicSpawnerEntityData",
        &[("ParticipantLogicBlueprint", ptr("b", 1))],
    );
    assert!(owner_roots(&mut Graph::new(&mut m), "a", Some("c")).is_err());
}
