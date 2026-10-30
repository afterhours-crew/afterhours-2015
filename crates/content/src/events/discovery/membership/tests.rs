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
