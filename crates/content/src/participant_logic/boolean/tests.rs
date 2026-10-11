// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn site(i: usize) -> Site {
    Site {
        key: ("a".into(), i),
        ..Site::root("a")
    }
}
fn array(v: Vec<Value>) -> Value {
    Value::Array(v)
}
fn structure(v: Vec<(&str, Value)>) -> Value {
    Value::Struct(Fields(v.into_iter().map(|(k, v)| (k.into(), v)).collect()))
}
fn set(m: &mut Memory, i: usize, k: &str, v: Value) {
    m.0.get_mut(&("a".into(), i))
        .unwrap()
        .fields
        .0
        .retain(|(n, _)| n != k);
    m.0.get_mut(&("a".into(), i))
        .unwrap()
        .fields
        .0
        .push((k.into(), v));
}
fn fields() -> Vec<(Site, Kind)> {
    vec![
        (site(1), Kind::Root),
        (site(2), Kind::RpcReference),
        (site(4), Kind::BoolProperty),
        (site(8), Kind::RpcReference),
    ]
}
fn base() -> (Memory, Rules) {
    let r = crate::BuildIdentity::supported().participant_logic;
    let p = &r.boolean_ports;
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            (
                "PropertyConnections",
                array(vec![edge(
                    local(5),
                    p["set_contains"],
                    local(4),
                    p["synced_input"],
                )]),
            ),
            (
                "LinkConnections",
                array(vec![
                    edge(local(5), p["participant"], local(3), 0),
                    edge(local(5), p["set_link"], local(8), 991),
                    edge(local(3), p["provider_root"], local(1), 0),
                ]),
            ),
        ],
    );
    m.put("a", 1, "ParticipantLogicEntityData", &[]);
    m.put(
        "a",
        2,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "a",
        8,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put("a", 3, "ParticipantLogicParticipantProviderEntityData", &[]);
    m.put("a", 4, "SyncedBoolEntityData", &[]);
    m.put("a", 5, "ParticipantSetAccessorEntityData", &[]);
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            array(vec![
                structure(vec![
                    ("Id", Value::UInt32(991)),
                    ("LinkTypeHash", Value::UInt32(p["participant_set_type"])),
                ]),
                structure(vec![
                    ("Id", Value::UInt32(234)),
                    ("LinkTypeHash", Value::UInt32(p["context_type"])),
                ]),
            ]),
        )],
    );
    (m, r)
}
fn program(m: &mut Memory, r: &Rules) -> Result<Program, Error> {
    Ok(compile(&mut Graph::new(m), &fields(), r)?
        .remove(0)
        .initial
        .unwrap())
}
#[test]
fn membership_uses_the_exact_reference_and_current_participant_state() {
    let (mut m, r) = base();
    let program = program(&mut m, &r).unwrap();
    for state in [false, true] {
        assert_eq!(
            program
                .evaluate(|i| {
                    assert_eq!(
                        *i,
                        Input::ParticipantSetContains(Port {
                            serializer: 3,
                            field: 991
                        })
                    );
                    Some(state)
                })
                .unwrap(),
            state
        );
    }
    assert!(program.evaluate(|_| None).is_err());
    let p = &r.boolean_ports;
    set(
        &mut m,
        0,
        "LinkConnections",
        array(vec![
            edge(local(5), p["participant"], local(3), 0),
            edge(local(5), p["set_link"], local(8), 991),
            edge(local(3), p["provider_root"], local(2), 0),
        ]),
    );
    assert!(compile(&mut Graph::new(&mut m), &fields(), &r).is_err());
}
#[test]
fn disconnected_is_distinct_from_unknown_or_ambiguous_input() {
    let (mut m, r) = base();
    set(&mut m, 0, "PropertyConnections", array(vec![]));
    assert!(
        compile(&mut Graph::new(&mut m), &fields(), &r).unwrap()[0]
            .initial
            .is_none()
    );
    let row = edge(local(5), 123, local(4), r.boolean_ports["synced_input"]);
    set(&mut m, 0, "PropertyConnections", array(vec![row.clone()]));
    assert!(program(&mut m, &r).is_err());
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![row.clone(), row]),
    );
    assert!(program(&mut m, &r).is_err());
}

#[test]
fn channel_declarations_reject_wrong_type_missing_and_duplicate_ports() {
    let (mut m, r) = base();
    let declaration = |ty| {
        structure(vec![
            ("Id", Value::UInt32(991)),
            ("LinkTypeHash", Value::UInt32(ty)),
        ])
    };
    for rows in [
        vec![],
        vec![declaration(123)],
        vec![declaration(r.boolean_ports["participant_set_type"]); 2],
    ] {
        m.put("c", 0, "SchematicChannelAsset", &[("Links", array(rows))]);
        assert!(program(&mut m, &r).is_err());
    }
}
fn instruction(code: u32, p1: u32, p2: u32, result: u32) -> Value {
    structure(vec![
        (
            "Code",
            Value::Enum {
                value: code as i32,
                name: None,
            },
        ),
        ("Param1", Value::Int32(p1 as i32)),
        ("Param2", Value::UInt32(p2)),
        ("Result", Value::UInt32(result)),
    ])
}
fn math(r: &Rules, hash: u32) -> Value {
    let p = &r.boolean_ports;
    structure(vec![
        (
            "FunctionCalls",
            array(vec![structure(vec![(
                "Parameters",
                array((0..3).map(Value::UInt32).collect()),
            )])]),
        ),
        (
            "Instructions",
            array(vec![
                instruction(p["math_input_bool"], 101, 0, 0),
                instruction(p["math_input_bool"], 102, 0, 1),
                instruction(p["math_input_bool"], 103, 0, 2),
                instruction(p["math_function"], hash, 0, 0),
                instruction(p["math_return"], p["math_return_bool"], 0, 0),
            ]),
        ),
    ])
}
#[test]
fn installed_conditional_selects_both_branches_and_rejects_other_programs() {
    let (mut m, r) = base();
    let p = &r.boolean_ports;
    m.put(
        "a",
        6,
        "MathEntityData",
        &[
            ("EvaluateOnEvent", Value::Bool(false)),
            ("Assembly", math(&r, p["math_if_bool"])),
        ],
    );
    m.put(
        "a",
        7,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(true))],
    );
    m.put(
        "a",
        9,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(false))],
    );
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![
            edge(local(6), p["logic_output"], local(4), p["synced_input"]),
            edge(local(5), p["set_contains"], local(6), 101),
            edge(local(7), p["bool_value"], local(6), 102),
            edge(local(9), p["bool_value"], local(6), 103),
        ]),
    );
    let plan = program(&mut m, &r).unwrap();
    for member in [false, true] {
        assert_eq!(plan.evaluate(|_| Some(member)).unwrap(), member);
    }
    set(&mut m, 6, "Assembly", math(&r, 123));
    assert!(program(&mut m, &r).is_err());
    set(&mut m, 6, "Assembly", math(&r, p["math_if_bool"]));
    set(&mut m, 6, "EvaluateOnEvent", Value::Bool(true));
    assert!(program(&mut m, &r).is_err());
}
#[test]
fn boolean_operators_validate_all_inputs_and_bound_cycles_and_depth() {
    let (mut m, r) = base();
    let p = &r.boolean_ports;
    m.put("a", 6, "AndEntityData", &[("InputCount", Value::UInt32(2))]);
    m.put(
        "a",
        7,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(false))],
    );
    let edges = vec![
        edge(local(6), p["logic_output"], local(4), p["synced_input"]),
        edge(local(7), p["bool_value"], local(6), 101),
        edge(local(5), p["set_contains"], local(6), 102),
    ];
    set(&mut m, 0, "PropertyConnections", array(edges.clone()));
    assert!(
        !program(&mut m, &r)
            .unwrap()
            .evaluate(|_| Some(true))
            .unwrap()
    );
    // Do not hide unsupported branches behind a false constant.
    m.put("a", 5, "UnknownEntityData", &[]);
    assert!(program(&mut m, &r).is_err());
    m.put("a", 5, "ParticipantSetAccessorEntityData", &[]);
    m.put("a", 6, "OrEntityData", &[("InputCount", Value::UInt32(2))]);
    assert!(
        program(&mut m, &r)
            .unwrap()
            .evaluate(|_| Some(true))
            .unwrap()
    );
    set(&mut m, 6, "InputCount", Value::UInt32(3));
    assert!(program(&mut m, &r).is_err());
    let mut chain = vec![edge(
        local(10),
        p["logic_output"],
        local(4),
        p["synced_input"],
    )];
    for i in 10..40 {
        m.put("a", i, "NotEntityData", &[]);
        chain.push(edge(
            local(i + 1),
            p["logic_output"],
            local(i),
            p["not_input"],
        ));
    }
    m.put("a", 40, "NotEntityData", &[]);
    chain.push(edge(
        local(10),
        p["logic_output"],
        local(40),
        p["not_input"],
    ));
    set(&mut m, 0, "PropertyConnections", array(chain));
    assert!(
        program(&mut m, &r)
            .unwrap_err()
            .to_string()
            .contains("bound")
    );
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![
            edge(local(10), p["logic_output"], local(4), p["synced_input"]),
            edge(local(10), p["logic_output"], local(10), p["not_input"]),
        ]),
    );
    assert!(
        program(&mut m, &r)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}
#[test]
fn context_keeps_categories_and_active_state_in_the_explicit_input() {
    let (mut m, r) = base();
    let p = &r.boolean_ports;
    m.put(
        "a",
        5,
        "ParticipantContextReporterEntityData",
        &[("Categories", array(vec![ptr("d", 1)]))],
    );
    m.put("d", 1, "GameModeCategory", &[]);
    // Referenced assets must identify their primary object, not an arbitrary subobject.
    assert!(program(&mut m, &r).is_err());
    m.put("d", 0, "GameModeCategory", &[]);
    set(&mut m, 5, "Categories", array(vec![ptr("d", 0)]));
    m.put(
        "a",
        7,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(true))],
    );
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![
            edge(local(5), p["context_output"], local(4), p["synced_input"]),
            edge(local(7), p["bool_value"], local(5), p["active"]),
        ]),
    );
    set(
        &mut m,
        0,
        "LinkConnections",
        array(vec![
            edge(local(5), p["participant"], local(3), 0),
            edge(local(3), p["provider_root"], local(1), 0),
            edge(local(5), p["context_link"], local(8), 234),
        ]),
    );
    let program = program(&mut m, &r).unwrap();
    assert!(
        program
            .evaluate(|i| {
                assert_eq!(
                    *i,
                    Input::Context {
                        channel: Port {
                            serializer: 3,
                            field: 234
                        },
                        categories: vec!["d".into()],
                        active: true
                    }
                );
                Some(true)
            })
            .unwrap()
    );
}
