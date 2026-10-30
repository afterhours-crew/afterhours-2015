// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::values::name;
use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::{Fields, Value};

fn site(index: usize) -> Site {
    Site {
        key: ("a".into(), index),
        ..Site::root("a")
    }
}
fn interface(id: u32, text: &str) -> Value {
    Value::Struct(Fields(vec![
        ("Id".into(), Value::UInt32(id)),
        ("Value".into(), Value::String(text.into())),
    ]))
}
fn event(source: usize, output: &str, target: usize, input: &str) -> Value {
    Value::Struct(Fields(vec![
        ("Source".into(), local(source)),
        ("Target".into(), local(target)),
        (
            "SourceEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(name(output)))])),
        ),
        (
            "TargetEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(name(input)))])),
        ),
    ]))
}

fn fixture() -> Memory {
    let mut memory = Memory::default();
    memory.put(
        "a",
        0,
        "Blueprint",
        &[
            ("EventConnections", Value::Array(vec![])),
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(1), 1, local(3), name("In0")),
                    edge(local(1), 2, local(3), name("In1")),
                    edge(local(1), 3, local(2), 42),
                    edge(local(3), name("Out"), local(2), 42),
                ]),
            ),
        ],
    );
    memory.put(
        "a",
        1,
        "InterfaceDescriptorData",
        &[(
            "Fields",
            Value::Array(vec![
                interface(1, "Int32 2100"),
                interface(2, "Int32 3700"),
                interface(3, "Int32 400"),
                interface(4, ""),
            ]),
        )],
    );
    memory.put("a", 2, "LinkedSchematicChannelEntityData", &[]);
    memory.put(
        "a",
        3,
        "IntHubEntityData",
        &[
            ("InputCount", Value::UInt32(2)),
            ("InputSelect", Value::UInt32(0)),
        ],
    );
    memory
}

#[test]
fn startup_calculation_replaces_initial_reward_without_waiving_other_ambiguity() {
    let mut memory = fixture();
    assert_eq!(
        amount(&mut Graph::new(&mut memory), &site(2), 42).unwrap(),
        2100
    );
    // The general evaluator continues to reject this disagreeing fan-in.
    assert!(
        Evaluator::new(&mut Graph::new(&mut memory))
            .input(&site(2), 42)
            .is_err()
    );
    memory.put(
        "a",
        3,
        "IntHubEntityData",
        &[
            ("InputCount", Value::UInt32(2)),
            ("InputSelect", Value::UInt32(1)),
        ],
    );
    assert_eq!(
        amount(&mut Graph::new(&mut memory), &site(2), 42).unwrap(),
        3700
    );
    let Value::Array(edges) = &mut memory
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "PropertyConnections")
        .unwrap()
        .1
    else {
        panic!("connections")
    };
    edges.push(edge(local(3), name("Out"), local(2), 42));
    assert!(amount(&mut Graph::new(&mut memory), &site(2), 42).is_err());
}

#[test]
fn boolean_selection_distinguishes_no_default_from_initial_publication() {
    for (text, fallback, on_start, expected) in [
        ("", false, false, 2100),
        ("", true, false, 2100),
        ("", true, true, 3700),
        ("Boolean False", true, true, 2100),
        ("Boolean True", false, false, 3700),
        ("Boolean True", true, false, 3700),
    ] {
        let mut memory = fixture();
        memory.put(
            "a",
            1,
            "InterfaceDescriptorData",
            &[(
                "Fields",
                Value::Array(vec![
                    interface(1, "Int32 2100"),
                    interface(2, "Int32 3700"),
                    interface(3, "Int32 400"),
                    interface(4, text),
                ]),
            )],
        );
        memory.put(
            "a",
            0,
            "Blueprint",
            &[
                (
                    "PropertyConnections",
                    Value::Array(vec![
                        edge(local(1), 1, local(3), name("In0")),
                        edge(local(1), 2, local(3), name("In1")),
                        edge(local(1), 4, local(4), name("Bool")),
                    ]),
                ),
                (
                    "EventConnections",
                    Value::Array(vec![event(4, "OnTrue", 3, "SelectIn1")]),
                ),
            ],
        );
        memory.put(
            "a",
            4,
            "CompareBoolEntityData",
            &[
                ("Bool", Value::Bool(fallback)),
                ("TriggerOnStart", Value::Bool(on_start)),
                ("TriggerOnPropertyChange", Value::Bool(true)),
            ],
        );
        assert_eq!(
            Evaluator::new(&mut Graph::new(&mut memory))
                .output(&site(3), name("Out"))
                .unwrap(),
            Number::Integer(expected),
            "{text:?}, {fallback}, {on_start}"
        );
    }
}

#[test]
fn integer_range_selector_uses_closed_bounds_before_reward_evaluation() {
    for (difficulty, expected) in [(10, 2100), (11, 3700), (25, 3700), (26, 2100)] {
        let mut memory = fixture();
        memory.put(
            "a",
            0,
            "Blueprint",
            &[
                (
                    "PropertyConnections",
                    Value::Array(vec![
                        edge(local(1), 1, local(3), name("In0")),
                        edge(local(1), 2, local(3), name("In1")),
                        edge(local(5), name("Value"), local(4), name("Value")),
                    ]),
                ),
                (
                    "EventConnections",
                    Value::Array(vec![event(4, "InRange", 3, "SelectIn1")]),
                ),
            ],
        );
        memory.put(
            "a",
            4,
            "CompareIntRangeEntityData",
            &[
                ("Min", Value::Int32(11)),
                ("Max", Value::Int32(25)),
                ("TriggerOnStart", Value::Bool(true)),
            ],
        );
        memory.put(
            "a",
            5,
            "IntEntityData",
            &[("DefaultValue", Value::Int32(difficulty))],
        );
        assert_eq!(
            Evaluator::new(&mut Graph::new(&mut memory))
                .output(&site(3), name("Out"))
                .unwrap(),
            Number::Integer(expected),
        );
    }
}

#[test]
fn negative_or_missing_calculator_and_conflicting_start_selectors_are_rejected() {
    let mut memory = fixture();
    memory.put(
        "a",
        3,
        "IntHubEntityData",
        &[
            ("InputCount", Value::UInt32(2)),
            ("InputSelect", Value::UInt32(2)),
        ],
    );
    assert!(amount(&mut Graph::new(&mut memory), &site(2), 42).is_err());
    memory.put(
        "a",
        3,
        "IntHubEntityData",
        &[
            ("InputCount", Value::UInt32(2)),
            ("InputSelect", Value::UInt32(0)),
        ],
    );
    memory.put(
        "a",
        1,
        "InterfaceDescriptorData",
        &[(
            "Fields",
            Value::Array(vec![
                interface(1, "Int32 -1"),
                interface(2, "Int32 3700"),
                interface(3, "Int32 400"),
            ]),
        )],
    );
    assert!(amount(&mut Graph::new(&mut memory), &site(2), 42).is_err());
    memory.put(
        "a",
        0,
        "Blueprint",
        &[(
            "PropertyConnections",
            Value::Array(vec![edge(local(1), 3, local(2), 42)]),
        )],
    );
    assert!(amount(&mut Graph::new(&mut memory), &site(2), 42).is_err());
    memory.put(
        "a",
        0,
        "Blueprint",
        &[
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(1), 1, local(3), name("In0")),
                    edge(local(1), 2, local(3), name("In1")),
                ]),
            ),
            (
                "EventConnections",
                Value::Array(vec![
                    event(4, "OnTrue", 3, "SelectIn0"),
                    event(4, "OnTrue", 3, "SelectIn1"),
                ]),
            ),
        ],
    );
    memory.put(
        "a",
        4,
        "CompareBoolEntityData",
        &[
            ("Bool", Value::Bool(true)),
            ("TriggerOnStart", Value::Bool(true)),
            ("TriggerOnPropertyChange", Value::Bool(false)),
        ],
    );
    assert!(
        Evaluator::new(&mut Graph::new(&mut memory))
            .output(&site(3), name("Out"))
            .is_err()
    );
}
