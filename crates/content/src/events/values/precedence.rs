// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn site(i: usize) -> Site {
    Site {
        key: ("inner".into(), i),
        ..Site::root("inner")
    }
}

fn fixture(fallback: &str, condition: bool, reverse: bool) -> Memory {
    let mut m = Memory::default();
    let mut inputs = vec![
        edge(local(1), OUTPUT, local(3), name("In0")),
        edge(local(2), 7, local(3), name("In0")),
        edge(local(4), VALUE, local(1), CONDITION),
    ];
    if reverse {
        inputs.reverse();
    }
    m.put(
        "inner",
        0,
        "LogicPrefabBlueprint",
        &[
            ("EventConnections", Value::Array(vec![])),
            ("PropertyConnections", Value::Array(inputs)),
        ],
    );
    m.put(
        "inner",
        1,
        "ConditionalFloatEntityData",
        &[
            ("Condition", Value::Bool(false)),
            ("ValueIfTrue", Value::Float32(3.5)),
            ("ValueIfFalse", Value::Float32(0.75)),
        ],
    );
    m.put(
        "inner",
        2,
        "InterfaceDescriptorData",
        &[(
            "Fields",
            Value::Array(vec![Value::Struct(Fields(vec![
                ("Id".into(), Value::UInt32(7)),
                ("Value".into(), Value::String(fallback.into())),
            ]))]),
        )],
    );
    m.put(
        "inner",
        3,
        "FloatHubEntityData",
        &[
            ("InputCount", Value::Int32(1)),
            ("InputSelect", Value::Int32(0)),
        ],
    );
    m.put(
        "inner",
        4,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(condition))],
    );
    m
}

#[test]
fn conditional_startup_publication_supersedes_only_unbound_interface_default() {
    for reverse in [false, true] {
        for (condition, expected) in [(true, 3.5), (false, 0.75)] {
            for fallback in ["Float32 1.25", ""] {
                let mut m = fixture(fallback, condition, reverse);
                assert_eq!(
                    Evaluator::new(&mut Graph::new(&mut m))
                        .output(&site(3), OUT)
                        .unwrap(),
                    Number::Float(expected)
                );
            }
        }
    }
}

#[test]
fn conditional_does_not_override_a_bound_interface_or_other_producer() {
    let mut m = fixture("Float32 1.25", true, false);
    m.put(
        "outer",
        0,
        "LogicPrefabBlueprint",
        &[
            ("EventConnections", Value::Array(vec![])),
            (
                "PropertyConnections",
                Value::Array(vec![edge(local(2), VALUE, local(1), 7)]),
            ),
        ],
    );
    m.put(
        "outer",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("inner", 0))],
    );
    m.put(
        "outer",
        2,
        "FloatEntityData",
        &[("DefaultValue", Value::Float32(9.))],
    );
    let mut nested = site(3);
    nested.parents.push(Frame {
        bus: ("outer".into(), 0),
        reference: ("outer".into(), 1),
    });
    assert!(
        Evaluator::new(&mut Graph::new(&mut m))
            .output(&nested, OUT)
            .is_err()
    );
    m.put(
        "outer",
        0,
        "LogicPrefabBlueprint",
        &[
            ("EventConnections", Value::Array(vec![])),
            ("PropertyConnections", Value::Array(vec![])),
        ],
    );
    assert_eq!(
        Evaluator::new(&mut Graph::new(&mut m))
            .output(&nested, OUT)
            .unwrap(),
        Number::Float(3.5)
    );

    m.put(
        "inner",
        2,
        "FloatEntityData",
        &[("DefaultValue", Value::Float32(9.))],
    );
    m.put(
        "inner",
        0,
        "LogicPrefabBlueprint",
        &[
            ("EventConnections", Value::Array(vec![])),
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(1), OUTPUT, local(3), name("In0")),
                    edge(local(2), VALUE, local(3), name("In0")),
                    edge(local(4), VALUE, local(1), CONDITION),
                ]),
            ),
        ],
    );
    assert!(
        Evaluator::new(&mut Graph::new(&mut m))
            .output(&site(3), OUT)
            .is_err()
    );
}

#[test]
fn conditional_precedence_preserves_type_and_input_errors() {
    let mut m = fixture("Int32 1", true, false);
    assert!(
        Evaluator::new(&mut Graph::new(&mut m))
            .output(&site(3), OUT)
            .is_err()
    );
    let mut m = fixture("Float32 1.25", true, false);
    m.put(
        "inner",
        4,
        "IntEntityData",
        &[("DefaultValue", Value::Int32(1))],
    );
    assert!(
        Evaluator::new(&mut Graph::new(&mut m))
            .output(&site(3), OUT)
            .is_err()
    );
    let mut m = fixture("", true, false);
    m.put(
        "inner",
        1,
        "ConditionalFloatEntityData",
        &[
            ("Condition", Value::Bool(false)),
            ("ValueIfTrue", Value::Int32(7)),
            ("ValueIfFalse", Value::Float32(0.75)),
        ],
    );
    assert!(
        Evaluator::new(&mut Graph::new(&mut m))
            .output(&site(3), OUT)
            .is_err()
    );
}
