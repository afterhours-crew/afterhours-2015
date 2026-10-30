// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};

fn source() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "SubWorldData",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), 177557, ptr("b", 2), 0),
                edge(local(1), 177556, local(3), 0),
                edge(ptr("b", 2), STATES, local(4), 0),
                edge(ptr("b", 2), STATES, local(5), 0),
                edge(local(3), BEGIN, local(5), 0),
                edge(local(3), END, local(6), 0),
            ]),
        )],
    );
    m.put(
        "a",
        1,
        "BooleanOperationPredicateEntityData",
        &[
            ("NumberOfOperands", Value::Int32(2)),
            (
                "Operation",
                Value::Enum {
                    value: 0,
                    name: Some("AND".into()),
                },
            ),
        ],
    );
    // Imported layer objects use the enclosing scene bus, never asset b's bus.
    m.put(
        "b",
        2,
        "ParticipantStateListBoolParticipantPropertyEntityData",
        &[],
    );
    m.put(
        "a",
        3,
        "ParticipantStateRangeBoolParticipantPropertyEntityData",
        &[],
    );
    for id in 4..=6 {
        m.put("a", id, "ParticipantStateEntityData", &[]);
    }
    m
}
fn compile(m: &mut Memory) -> Result<Predicate<Site>, Error> {
    let mut site = Site::root("a");
    site.key.1 = 1;
    read(&mut Graph::new(m), site, &mut BTreeSet::new(), &mut 0)
}
#[test]
fn imported_list_and_range_preserve_order_identity_and_distinct_latch_semantics() {
    let result = compile(&mut source())
        .unwrap()
        .try_map(&mut |s| Ok::<_, Error>(s.key.1))
        .unwrap();
    assert_eq!(
        result,
        Predicate::Boolean {
            operation: BooleanOperation::And,
            operands: vec![
                Predicate::Property(Definition::List(vec![4, 5])),
                Predicate::Property(Definition::Range { begin: 5, end: 6 })
            ],
        }
    );
}
#[test]
fn missing_ambiguous_cyclic_and_unknown_inputs_never_become_inactive() {
    for case in 0..5 {
        let mut m = source();
        match case {
            0 => {
                m.0.remove(&("a".into(), 6));
            }
            1 => {
                m.put("b", 2, "UnknownPredicateEntityData", &[]);
            }
            2 => {
                m.put(
                    "a",
                    1,
                    "BooleanOperationPredicateEntityData",
                    &[
                        ("NumberOfOperands", Value::Int32(33)),
                        (
                            "Operation",
                            Value::Enum {
                                value: 0,
                                name: Some("AND".into()),
                            },
                        ),
                    ],
                );
            }
            3 | 4 => {
                let root = m.0.get_mut(&("a".into(), 0)).unwrap();
                let Value::Array(rows) = &mut root
                    .fields
                    .0
                    .iter_mut()
                    .find(|(k, _)| k == "LinkConnections")
                    .unwrap()
                    .1
                else {
                    panic!()
                };
                if case == 3 {
                    rows[0] = edge(local(1), 177557, local(1), 0);
                } else {
                    rows.push(edge(local(1), 177557, local(3), 0));
                }
            }
            _ => unreachable!(),
        }
        assert!(compile(&mut m).is_err(), "case {case}");
    }
}
