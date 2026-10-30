// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};

fn site(index: usize) -> Site {
    Site {
        key: ("a".into(), index),
        ..Site::root("a")
    }
}

fn predicate_object(memory: &mut Memory, index: usize, operation: &str) {
    memory.put(
        "a",
        index,
        "ParticipantSetPredicateEntityData",
        &[
            ("NumberOfOperands", Value::Int32(2)),
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

fn fixture(offset: usize) -> Memory {
    let id = |n| n + offset;
    let mut m = Memory::default();
    let mut edges = vec![
        edge(local(id(1)), 2_664_990_019, local(id(2)), 0),
        edge(local(id(6)), name("ParticipantSet"), local(id(7)), 0),
        edge(local(id(8)), 2_093_602_801, local(id(13)), 0),
        edge(local(id(9)), 2_093_602_801, local(id(14)), 0),
        edge(local(id(12)), 2_093_602_801, local(id(15)), 0),
        edge(local(id(16)), name("ParticipantManager"), local(id(18)), 0),
        edge(local(id(17)), name("ParticipantStates"), local(id(19)), 0),
        edge(local(id(17)), name("ParticipantStates"), local(id(20)), 0),
    ];
    for (node, op, a, b) in [
        (2, "DIF", 3, 4),
        (3, "UNION", 5, 6),
        (4, "DIF", 8, 9),
        (5, "DIF", 10, 9),
        (10, "DIF", 7, 11),
        (11, "DIF", 12, 8),
        (7, "DIF", 16, 17),
    ] {
        predicate_object(&mut m, id(node), op);
        edges.extend([
            edge(local(id(node)), name("0"), local(id(a)), 0),
            edge(local(id(node)), name("1"), local(id(b)), 0),
        ]);
    }
    // Deliberately reverse declarations: these are graph edges, not an index
    // pattern or dependency on the ordering of the reference recording.
    edges.reverse();
    m.put(
        "a",
        0,
        "Blueprint",
        &[
            ("LinkConnections", Value::Array(edges)),
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(id(1)), 513_251_846, local(id(22)), name("In")),
                    edge(
                        local(id(22)),
                        name("Out"),
                        local(id(21)),
                        name("IsSatisfied"),
                    ),
                ]),
            ),
        ],
    );
    for (index, class) in [
        (1, NOTIFICATION),
        (6, "ParticipantSetCopyEntityData"),
        (8, "ParticipantSetFromBPPEntityData"),
        (9, "ParticipantSetFromBPPEntityData"),
        (12, "ParticipantSetFromBPPEntityData"),
        (13, "BoolParticipantPropertyEntityData"),
        (14, "BoolParticipantPropertyEntityData"),
        (15, "BoolParticipantPropertyEntityData"),
        (16, "ParticipantSetPlayerAdapterEntityData"),
        (17, "ParticipantSetStateAdapterEntityData"),
        (18, "ParticipantManagerEntityData"),
        (19, "TeleportingParticipantStateEntityData"),
        (20, "StateSequenceParticipantStateEntityData"),
        (21, "RequirementEntityData"),
        (22, "NotEntityData"),
    ] {
        m.put("a", id(index), class, &[]);
    }
    m
}

#[test]
fn requirement_dependencies_preserve_event_controlled_copy_and_all_boolean_inputs() {
    for offset in [0, 37] {
        let mut m = fixture(offset);
        let mut g = Graph::new(&mut m);
        let (notification_site, satisfied) =
            notification(&mut g, &site(21 + offset)).unwrap().unwrap();
        assert!(!satisfied);
        assert_eq!(notification_site, site(1 + offset));
        let found = dependency(&mut g, &notification_site).unwrap();
        assert_eq!(found.states, [site(19 + offset), site(20 + offset)]);
        assert_eq!(
            (found.b, found.c, found.x),
            (site(13 + offset), site(15 + offset), site(14 + offset))
        );
        assert_eq!(found.mirrored, site(6 + offset));
        assert_eq!(found.player_manager, site(18 + offset));
    }
}

#[test]
fn requirement_dependencies_reject_unknown_operations_membership_sources_and_state_classes() {
    for (index, class) in [
        (8, "UnknownSetEntityData"),
        (6, "ParticipantSetPredicateEntityData"),
        (16, "ParticipantSetStateAdapterEntityData"),
        (20, "UnknownStateEntityData"),
    ] {
        let mut m = fixture(0);
        m.put("a", index, class, &[]);
        assert!(dependency(&mut Graph::new(&mut m), &site(1)).is_err());
    }
    let mut m = fixture(0);
    predicate_object(&mut m, 4, "UNION");
    assert!(dependency(&mut Graph::new(&mut m), &site(1)).is_err());
}

#[test]
fn requirement_dependencies_reject_distinct_instances_in_repeated_operands() {
    for (source, port, target) in [(11, name("1"), 12), (10, name("0"), 5), (5, name("1"), 8)] {
        let mut m = fixture(0);
        let Value::Array(edges) =
            m.0.get_mut(&("a".into(), 0))
                .unwrap()
                .fields
                .0
                .iter_mut()
                .find(|(name, _)| name == "LinkConnections")
                .unwrap()
                .1
                .clone()
        else {
            panic!()
        };
        let mut changed = edges;
        let original = edge(
            local(source),
            port,
            local(if source == 11 {
                8
            } else if source == 10 {
                7
            } else {
                9
            }),
            0,
        );
        let position = changed.iter().position(|e| *e == original).unwrap();
        changed[position] = edge(local(source), port, local(target), 0);
        m.0.get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(name, _)| name == "LinkConnections")
            .unwrap()
            .1 = Value::Array(changed);
        assert!(dependency(&mut Graph::new(&mut m), &site(1)).is_err());
    }
}

#[test]
fn cyclic_requirement_negation_is_rejected() {
    let mut m = fixture(0);
    m.put(
        "a",
        0,
        "Blueprint",
        &[(
            "PropertyConnections",
            Value::Array(vec![
                edge(local(22), name("Out"), local(21), name("IsSatisfied")),
                edge(local(22), name("Out"), local(22), name("In")),
            ]),
        )],
    );
    assert!(notification(&mut Graph::new(&mut m), &site(21)).is_err());
}

#[test]
fn requirement_boolean_identifiers_follow_links_and_reject_conflicting_writers() {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "Blueprint",
        &[
            ("Objects", Value::Array(vec![local(7), local(3)])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(7), 3_959_795_656, local(9), 0),
                    edge(local(3), 3_959_795_656, local(2), 0),
                ]),
            ),
        ],
    );
    for (node, identifier) in [(7, "b"), (3, "c")] {
        m.put(
            "a",
            node,
            "IdentifiedBoolEntityData",
            &[("BoolIdentifier", ptr(identifier, 0))],
        );
        m.put(identifier, 0, "BoolIdentifierAsset", &[]);
    }
    m.put("a", 9, "BoolParticipantPropertyEntityData", &[]);
    m.put("a", 2, "BoolParticipantPropertyEntityData", &[]);
    let roots = vec![("a".into(), Site::root("a"))];
    let found = identified_bools(&mut Graph::new(&mut m), &roots).unwrap();
    assert_eq!(
        found.keys().cloned().collect::<Vec<_>>(),
        [site(2), site(9)]
    );
    // Resolving a pointer, not the identifier object's display fields, owns the
    // identifier. Neither absence nor a Boolean value is supplied as a default.
    assert_eq!(found[&site(9)], "b");
    assert_eq!(found[&site(2)], "c");
    let object = m.0.get_mut(&("a".into(), 0)).unwrap();
    let (_, links) = object
        .fields
        .0
        .iter_mut()
        .find(|(field, _)| field == "LinkConnections")
        .unwrap();
    *links = Value::Array(vec![
        edge(local(7), 3_959_795_656, local(9), 0),
        edge(local(3), 3_959_795_656, local(9), 0),
    ]);
    assert!(identified_bools(&mut Graph::new(&mut m), &roots).is_err());
    m.put("a", 9, "UnknownPropertyEntityData", &[]);
    assert!(identified_bools(&mut Graph::new(&mut m), &roots).is_err());
}
