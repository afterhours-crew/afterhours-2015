// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local};

fn site(index: usize) -> Site {
    Site {
        key: ("a".into(), index),
        ..Site::root("a")
    }
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), 2_664_990_019, local(2), 0),
                edge(local(2), name("0"), local(3), 0),
                edge(local(2), name("1"), local(4), 0),
                edge(local(3), name("0"), local(5), 0),
                edge(local(3), name("1"), local(6), 0),
                edge(local(5), name("ParticipantManager"), local(7), 0),
                edge(local(6), name("ParticipantStates"), local(8), 0),
                edge(local(4), 2_093_602_801, local(9), 0),
            ]),
        )],
    );
    for (index, class) in [
        (1, NOTIFICATION),
        (4, "ParticipantSetFromBPPEntityData"),
        (5, "ParticipantSetPlayerAdapterEntityData"),
        (6, "ParticipantSetStateAdapterEntityData"),
        (7, "ParticipantManagerEntityData"),
        (8, "TeleportingParticipantStateEntityData"),
        (9, "BoolParticipantPropertyEntityData"),
    ] {
        m.put("a", index, class, &[]);
    }
    for (index, operation) in [(2, "INTERSECTION"), (3, "DIF")] {
        m.put(
            "a",
            index,
            "ParticipantSetPredicateEntityData",
            &[
                ("NumberOfOperands", Value::UInt32(2)),
                (
                    "Operation",
                    Value::Enum {
                        value: 0,
                        name: Some(operation.into()),
                    },
                ),
            ],
        );
    }
    m
}
fn compile(m: &mut Memory) -> Result<Program, Error> {
    read(
        &mut Graph::new(m),
        &site(1),
        &BTreeMap::from([(site(9), "constructed/introduced".into())]),
        &mut |_, s| {
            Ok(Endpoint {
                scene_key: 3,
                serializer: s.key.1,
            })
        },
    )
}

#[test]
fn hidden_then_introduced_then_transition_then_return_keeps_account_ownership() {
    let program = compile(&mut fixture()).unwrap();
    let cases = [
        (true, false, false, false), // owned player, not introduced
        (true, false, true, true),
        (true, true, true, false),   // teleport transition hides it
        (true, false, true, true),   // resume, without earning progress again
        (false, false, true, false), // another participant isn't in this player set
    ];
    for (player, transition, introduced, expected) in cases {
        let state = |fact: &Fact| {
            Some(match fact {
                Fact::Player(id) => {
                    assert_eq!(*id, Identity::from(&site(7)));
                    player
                }
                Fact::States(states) => {
                    assert_eq!(
                        states.as_slice(),
                        [Endpoint {
                            scene_key: 3,
                            serializer: 8
                        }]
                    );
                    transition
                }
                Fact::Boolean(id) => {
                    assert_eq!(id, "constructed/introduced");
                    introduced
                }
                _ => panic!(),
            })
        };
        assert_eq!(program.evaluate(state).unwrap(), expected);
        assert_eq!(program.evaluate(state).unwrap(), expected);
    }
    // Missing progress remains an error even when player membership is false.
    assert!(
        program
            .evaluate(|f| (!matches!(f, Fact::Boolean(_))).then_some(false))
            .is_err()
    );
}

#[test]
fn copied_membership_is_independent_of_player_input_and_survives_union() {
    let mut m = fixture();
    m.put("a", 4, "ParticipantSetCopyEntityData", &[]);
    m.put(
        "a",
        2,
        "ParticipantSetPredicateEntityData",
        &[
            ("NumberOfOperands", Value::UInt32(2)),
            (
                "Operation",
                Value::Enum {
                    value: 0,
                    name: Some("UNION".into()),
                },
            ),
        ],
    );
    let program = compile(&mut m).unwrap();
    assert!(
        program
            .facts()
            .any(|f| *f == Fact::Copy(Identity::from(&site(4))))
    );
    assert!(
        program
            .evaluate(|f| Some(matches!(f, Fact::Copy(_))))
            .unwrap()
    );
    assert!(!program.evaluate(|_| Some(false)).unwrap());
    assert!(
        program
            .evaluate(|f| match f {
                Fact::Copy(_) => None,
                _ => Some(true),
            })
            .is_err()
    );
}

#[test]
fn unknown_class_missing_identifier_ambiguous_links_and_cycles_fail() {
    let mut m = fixture();
    m.put("a", 4, "UnimplementedSet", &[]);
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    assert!(
        read(
            &mut Graph::new(&mut m),
            &site(1),
            &BTreeMap::new(),
            &mut |_, _| Ok(Endpoint {
                scene_key: 3,
                serializer: 8
            })
        )
        .is_err()
    );
    for links in [
        vec![
            edge(local(1), 2_664_990_019, local(2), 0),
            edge(local(1), 2_664_990_019, local(2), 0),
        ],
        vec![
            edge(local(1), 2_664_990_019, local(2), 0),
            edge(local(2), name("0"), local(2), 0),
            edge(local(2), name("1"), local(4), 0),
        ],
    ] {
        let mut m = fixture();
        m.put(
            "a",
            0,
            "Blueprint",
            &[("LinkConnections", Value::Array(links))],
        );
        assert!(compile(&mut m).is_err());
    }
}

#[test]
fn event_owned_sets_remain_required_inputs_even_when_another_operand_is_false() {
    for (class, expected) in [
        (
            "ParticipantSetEntityData",
            Fact::Mutable(Identity::from(&site(4))),
        ),
        (
            "IntPropertyWithinRangeParticipantSetEntityData",
            Fact::IntegerRange(Identity::from(&site(4))),
        ),
    ] {
        let mut m = fixture();
        m.put("a", 4, class, &[]);
        if class == "IntPropertyWithinRangeParticipantSetEntityData" {
            range_fixture(&mut m);
        }
        let program = compile(&mut m).unwrap();
        assert!(program.facts().any(|f| *f == expected));
        assert!(
            program
                .evaluate(|f| (f != &expected).then_some(false))
                .is_err()
        );
        assert!(!program.evaluate(|_| Some(false)).unwrap());
        assert!(
            program
                .evaluate(|f| Some(!matches!(f, Fact::States(_))))
                .unwrap()
        );
    }
}

fn range_fixture(m: &mut Memory) {
    m.put(
        "a",
        4,
        "IntPropertyWithinRangeParticipantSetEntityData",
        &[
            ("MinValue", Value::Int32(0)),
            ("MaxValue", Value::Int32(10)),
        ],
    );
    m.put("a", 10, "IntParticipantPropertyEntityData", &[]);
    let root = m.0.get_mut(&("a".into(), 0)).unwrap();
    let Value::Array(links) = &mut root
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "LinkConnections")
        .unwrap()
        .1
    else {
        panic!()
    };
    links.push(edge(local(4), (-620_217_377i32) as u32, local(10), 0));
    root.fields
        .0
        .push(("PropertyConnections".into(), Value::Array(vec![])));
    root.fields
        .0
        .push(("EventConnections".into(), Value::Array(vec![])));
}

#[test]
fn event_range_preserves_connected_bounds_and_exact_property_owner() {
    let mut m = fixture();
    range_fixture(&mut m);
    let p = compile(&mut m).unwrap();
    assert_eq!(
        p.ranges()[&Identity::from(&site(4))],
        IntegerRange {
            input: Identity::from(&site(10)),
            input_port: 0,
            minimum: 0,
            maximum: 10,
        }
    );
    m.put(
        "a",
        11,
        "IntEntityData",
        &[("DefaultValue", Value::Int32(20))],
    );
    let root = m.0.get_mut(&("a".into(), 0)).unwrap();
    root.fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "PropertyConnections")
        .unwrap()
        .1 = Value::Array(vec![edge(
        local(11),
        name("Value"),
        local(4),
        name("MinValue"),
    )]);
    let p = compile(&mut m).unwrap();
    assert_eq!(
        (
            p.ranges()[&Identity::from(&site(4))].minimum,
            p.ranges()[&Identity::from(&site(4))].maximum
        ),
        (20, 10)
    );
    assert!(
        p.evaluate(|f| (!matches!(f, Fact::IntegerRange(_))).then_some(true))
            .is_err()
    );
    m.put("a", 10, "UnrelatedProperty", &[]);
    assert!(compile(&mut m).is_err());
    m.put("a", 10, "IntParticipantPropertyEntityData", &[]);
    m.put(
        "a",
        11,
        "IntEntityData",
        &[("DefaultValue", Value::Float32(2.))],
    );
    assert!(compile(&mut m).is_err());
    m.put(
        "a",
        11,
        "IntEntityData",
        &[("DefaultValue", Value::Int32(20))],
    );
    let root = m.0.get_mut(&("a".into(), 0)).unwrap();
    root.fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "EventConnections")
        .unwrap()
        .1 = Value::Array(vec![Value::Struct(nfs_frostbite::ebx::Fields(vec![(
        "Target".into(),
        local(11),
    )]))]);
    assert!(
        compile(&mut m).is_err(),
        "event-written literal cannot become a static bound"
    );
}

#[test]
fn range_relay_uses_its_bound_definition_and_rejects_schema_matches_elsewhere() {
    let mut m = fixture();
    range_fixture(&mut m);
    let channel = |m: &mut Memory, index, class, schema| {
        m.put("a", index, class, &[("ChannelAsset", local(schema))]);
    };
    channel(&mut m, 10, "LinkedSchematicChannelRelayEntityData", 20);
    channel(&mut m, 11, "LinkedSchematicChannelEntityData", 20);
    channel(&mut m, 12, "LinkedSchematicChannelEntityData", 20);
    m.put("a", 13, "IntParticipantPropertyEntityData", &[]);
    m.put("a", 14, "IntParticipantPropertyEntityData", &[]);
    m.put("a", 20, "SchematicChannelAsset", &[]);
    m.put("a", 21, "SchematicChannelAsset", &[]);
    let port = 123;
    let replace_links = |m: &mut Memory, edges: Vec<Value>| {
        m.0.get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(n, _)| n == "LinkConnections")
            .unwrap()
            .1 = Value::Array(edges);
    };
    let edges = vec![
        edge(local(1), 2_664_990_019, local(4), 0),
        edge(local(4), (-620_217_377i32) as u32, local(10), port),
        edge(local(10), 0x2780_2951, local(11), 0),
        edge(local(11), port, local(13), 0),
        // Identical schema, different definition and participant property.
        edge(local(12), port, local(14), 0),
    ];
    replace_links(&mut m, edges.clone());
    let program = compile(&mut m).unwrap();
    assert_eq!(
        program.ranges()[&Identity::from(&site(4))].input,
        Identity::from(&site(13))
    );
    let mut wrong = edges.clone();
    wrong.remove(2);
    replace_links(&mut m, wrong);
    assert!(
        compile(&mut m).is_err(),
        "nearby matching schema cannot replace a missing binding"
    );
    let mut wrong = edges.clone();
    wrong.push(edge(local(10), 0x2780_2951, local(12), 0));
    replace_links(&mut m, wrong);
    assert!(compile(&mut m).is_err());
    let mut cycle = edges.clone();
    cycle[3] = edge(local(11), port, local(10), port);
    replace_links(&mut m, cycle);
    assert!(compile(&mut m).is_err());
    replace_links(&mut m, edges);
    channel(&mut m, 11, "LinkedSchematicChannelEntityData", 21);
    assert!(compile(&mut m).is_err());
}
