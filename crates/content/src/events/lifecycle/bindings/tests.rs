// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local};

#[test]
fn copied_sets_retain_optional_typed_sources_without_aliasing_the_owner() {
    let mut m = Memory::default();
    m.put("mode", 1, "ParticipantSetEntityData", &[]);
    m.put("mode", 2, "ParticipantSetCopyEntityData", &[]);
    m.put("mode", 3, "ParticipantSetCopyEntityData", &[]);
    m.put(
        "mode",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array((1..=3).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(2), 0x2e0d_19d8, local(1), 0)]),
            ),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let b = extract(&mut g, &nodes, &[]).unwrap();
    assert_eq!(b.sets.len(), 3);
    assert_eq!(
        b.sets[1].kind,
        SetKind::Copy {
            source: Some(SetSource::Local(b.sets[0].identity.clone())),
        }
    );
    assert_eq!(b.sets[2].kind, SetKind::Copy { source: None });
    assert_ne!(b.sets[0].identity, b.sets[1].identity);
    m.put("mode", 1, "FloatEntityData", &[]);
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &[]).is_err());
}

#[test]
fn integer_mirrors_bind_typed_hash_assets_and_local_manager_and_provider() {
    use crate::graph::tests::ptr;
    let mut m = Memory::default();
    m.put(
        "mode",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array((1..=4).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(2), 0xc286_a9af, local(4), 0),
                    edge(local(3), 0xc286_a9af, local(4), 0),
                    edge(local(3), 0x0df2_30d5, local(1), 0),
                ]),
            ),
        ],
    );
    m.put("mode", 1, "IntParticipantPropertyEntityData", &[]);
    m.put(
        "mode",
        2,
        "IntParticipantPropertyMirrorEntityData",
        &[("Identifier", ptr("i", 0))],
    );
    m.put(
        "mode",
        3,
        "IntParticipantPropertyMirrorProviderEntityData",
        &[("Identifier", ptr("i", 0))],
    );
    m.put("mode", 4, "MirroringManagerEntityData", &[]);
    m.put(
        "i",
        0,
        "IntParticipantPropertyIdentifier",
        &[("HashedId", Value::UInt32(0xf123_4567))],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let b = extract(&mut g, &nodes, &[]).unwrap();
    assert_eq!(b.mirrors[0].identifier, 0xf123_4567);
    assert_eq!(b.order, [Observer::Mirror(0), Observer::MirrorProvider(0)]);
    assert_eq!(
        b.mirrors[0].manager,
        Some(mirrors::Manager::Local(b.mirror_managers[0].clone()))
    );
    assert_eq!(
        b.mirror_providers[0].source,
        Some(IntegerSource::Local(b.integers[0].clone()))
    );
    m.put(
        "i",
        0,
        "UnrelatedIdentifier",
        &[("HashedId", Value::UInt32(0xf123_4567))],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &[]).is_err());
}

#[test]
fn integer_sorters_read_the_native_order_and_typed_source() {
    use nfs_world::scoring::rank_sorter::Order;
    let mut m = Memory::default();
    m.put(
        "mode",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2)])),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(2), 0x0df2_30d5, local(1), 0)]),
            ),
        ],
    );
    m.put("mode", 1, "IntParticipantPropertyEntityData", &[]);
    for (number, name, expected) in [
        (0, "Descending", Order::Descending),
        (1, "Ascending", Order::Ascending),
    ] {
        m.put(
            "mode",
            2,
            "IntParticipantPropertySorterEntityData",
            &[(
                "SortOrder",
                Value::Enum {
                    value: number,
                    name: Some(name.into()),
                },
            )],
        );
        let mut g = Graph::new(&mut m);
        let nodes = g.read(Site::root("mode")).unwrap();
        let b = extract(&mut g, &nodes, &[]).unwrap();
        assert_eq!(b.order, [Observer::Sorter(0)]);
        assert_eq!(b.sorters[0].order, expected);
        assert_eq!(
            b.sorters[0].source,
            IntegerSource::Local(b.integers[0].clone())
        );
    }
    m.put("mode", 1, "FloatParticipantPropertyEntityData", &[]);
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &[]).is_err());
}

#[test]
fn reporter_binding_keeps_distinct_sources_and_checks_single_source_cardinality() {
    let (mut m, fields) = super::super::tests::fixture();
    m.put("mode", 6, "StateReporterEntityData", &[]);
    m.put("mode", 7, "CompoundStatesReporterEntityData", &[]);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            ("Objects", Value::Array(vec![local(6), local(7)])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(6), 0x0dc0_f0f2, local(1), 0),
                    edge(local(7), 0xc5df_0f41, local(1), 0),
                    edge(local(7), 0xc5df_0f41, local(2), 0),
                    edge(local(7), 0xc5df_0f41, local(1), 0),
                ]),
            ),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let b = extract(&mut g, &nodes, &fields).unwrap();
    assert_eq!(b.order, [Observer::Reporter(0), Observer::Reporter(1)]);
    assert_eq!(b.reporters[0].states.len(), 1);
    assert_eq!(b.reporters[1].states.len(), 2);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            ("Objects", Value::Array(vec![local(6)])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(6), 0x0dc0_f0f2, local(1), 0),
                    edge(local(6), 0x0dc0_f0f2, local(2), 0),
                ]),
            ),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &fields).is_err());
}

#[test]
fn predicate_and_update_set_wiring_preserve_numeric_operand_and_manager_order() {
    let (mut m, fields) = super::super::tests::fixture();
    m.put("mode", 6, "TriggeredPredicateEntityData", &[]);
    m.put(
        "mode",
        7,
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
    m.put("mode", 8, "UpdateableManagerEntityData", &[]);
    m.put("mode", 9, "UpdateSetEntityData", &[]);
    m.put("mode", 10, "UpdateSetEntityData", &[]);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            ("Objects", Value::Array((6..=10).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(
                        local(7),
                        nfs_world::scoring::integers::port("", 0),
                        local(6),
                        0,
                    ),
                    edge(
                        local(7),
                        nfs_world::scoring::integers::port("", 1),
                        local(6),
                        0,
                    ),
                    edge(local(8), 0x7644_d2cd, local(10), 0),
                    edge(local(8), 0x7644_d2cd, local(9), 0),
                ]),
            ),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let bindings = extract(&mut g, &nodes, &fields).unwrap();
    assert_eq!(
        bindings.order,
        [Observer::Predicate(0), Observer::Predicate(1)]
    );
    assert!(
        matches!(&bindings.predicates[1].kind, PredicateKind::Boolean { operation: BooleanOperation::And, operands } if operands.len() == 2 && operands[0] == operands[1])
    );
    assert_eq!(
        bindings.update_managers[0]
            .sets
            .iter()
            .map(|s| s.index)
            .collect::<Vec<_>>(),
        [10, 9]
    );
    m.put("mode", 9, "GameTimeEntityData", &[]);
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &fields).is_err());
}

fn fixture() -> (Memory, Vec<FieldSite>) {
    let (mut m, fields) = super::super::tests::fixture();
    m.put(
        "mode",
        6,
        "ParticipantStateListBoolParticipantPropertyEntityData",
        &[],
    );
    m.put(
        "mode",
        7,
        "ParticipantStateRangeBoolParticipantPropertyEntityData",
        &[],
    );
    for i in [8, 9] {
        m.put(
            "mode",
            i,
            "ParticipantLogicSpawnerEntityData",
            &[("ParticipantLogicBlueprint", local(10))],
        );
    }
    m.put("mode", 10, "ParticipantLogicBlueprint", &[]);
    m.put(
        "mode",
        11,
        "ParticipantLogicSpawnerSetControlledEntityData",
        &[("ParticipantLogicBlueprint", local(10))],
    );
    m.put("mode", 12, "ParticipantSetEntityData", &[]);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            (
                "Objects",
                Value::Array((1..=12).filter(|&i| i != 10).map(local).collect()),
            ),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(6), 0x9e22_81be, local(2), 0),
                    edge(local(6), 0x9e22_81be, local(1), 0),
                    edge(local(7), 0xff47_6cc8, local(1), 0),
                    edge(local(7), 0x68d2_51b8, local(2), 0),
                    edge(local(8), 0xa707_ccbf, local(6), 0),
                    edge(local(9), 0xa707_ccbf, local(7), 0),
                    edge(local(11), 0xb075_bd0b, local(12), 0),
                ]),
            ),
        ],
    );
    (m, fields)
}

#[test]
fn set_controlled_logic_preserves_state_boolean_and_predicate_sources() {
    use nfs_world::scoring::participant_sets::Operation;
    let (mut m, fields) = fixture();
    m.put("mode", 13, "ParticipantSetStateAdapterEntityData", &[]);
    m.put(
        "mode",
        14,
        "ParticipantSetBoolPropertyAdapterEntityData",
        &[],
    );
    m.put(
        "mode",
        15,
        "ParticipantSetPredicateEntityData",
        &[
            ("NumberOfOperands", Value::Int32(2)),
            (
                "Operation",
                Value::Enum {
                    value: 2,
                    name: Some("DIF".into()),
                },
            ),
        ],
    );
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            ("Objects", Value::Array((13..=15).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(13), 0x9e22_81be, local(1), 0),
                    edge(local(13), 0x9e22_81be, local(2), 0),
                    edge(local(14), 0x969a_1637, local(6), 0),
                    edge(
                        local(15),
                        nfs_world::scoring::integers::port("", 0),
                        local(13),
                        0,
                    ),
                    edge(
                        local(15),
                        nfs_world::scoring::integers::port("", 1),
                        local(14),
                        0,
                    ),
                ]),
            ),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let result = extract(&mut g, &nodes, &fields).unwrap();
    assert_eq!(result.sets.len(), 3);
    assert!(matches!(result.sets[0].kind, SetKind::States(ref s) if s.len() == 2));
    assert!(matches!(result.sets[1].kind, SetKind::Boolean(ref link) if link.identity.index == 6));
    assert_eq!(
        result.sets[2].kind,
        SetKind::Predicate {
            operation: Operation::Difference,
            operands: vec![
                SetSource::Local(result.sets[0].identity.clone()),
                SetSource::Local(result.sets[1].identity.clone())
            ]
        }
    );
    m.put("mode", 14, "FloatEntityData", &[]);
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &fields).is_err());
}

#[test]
fn retains_authored_sources_and_distinguishes_repeated_logic_spawners() {
    let (mut m, fields) = fixture();
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let result = extract(&mut g, &nodes, &fields).unwrap();
    assert_eq!(result.properties.len(), 2);
    assert_eq!(
        result.properties[0].definition,
        Definition::List(vec![
            Endpoint {
                serializer: 2,
                port: 0
            },
            Endpoint {
                serializer: 1,
                port: 0
            }
        ])
    );
    assert_eq!(
        result.properties[1].definition,
        Definition::Range {
            begin: Endpoint {
                serializer: 1,
                port: 0
            },
            end: Endpoint {
                serializer: 2,
                port: 0
            }
        }
    );
    assert_eq!(
        result
            .spawners
            .iter()
            .map(|s| s.occurrence)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let Some(Controller::Boolean(first)) = &result.spawners[0].controller else {
        panic!()
    };
    assert_eq!(first.identity.index, 6);
    let Some(Controller::Set(last)) = &result.spawners[2].controller else {
        panic!()
    };
    assert_eq!(last.identity.index, 12);
}

#[test]
fn rejects_ambiguous_range_and_missing_state_serializer() {
    let (mut m, mut fields) = fixture();
    fields.retain(|(s, _, _)| s.key.1 != 2);
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &fields).is_err());
    let (mut m, fields) = fixture();
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            ("Objects", Value::Array(vec![local(7)])),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(7), 0xff47_6cc8, local(1), 0)]),
            ),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    assert!(extract(&mut g, &nodes, &fields).is_err());
}

#[test]
fn disconnected_spawner_is_retained_without_inventing_an_activation() {
    let (mut m, fields) = fixture();
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            ("Objects", Value::Array(vec![local(8)])),
            ("LinkConnections", Value::Array(vec![])),
        ],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("mode")).unwrap();
    let result = extract(&mut g, &nodes, &fields).unwrap();
    assert_eq!(result.spawners.len(), 1);
    assert_eq!(result.spawners[0].controller, None);
}

#[test]
fn local_set_counters_and_sorter_queries_preserve_actual_provider_identity() {
    let mut m = Memory::default();
    m.put(
        "m",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array((1..=5).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(2), 0x1224_af09, local(1), 0),
                    edge(local(4), 0x0df2_30d5, local(3), 0),
                    edge(local(5), 0x970a_d7f8, local(4), 0),
                ]),
            ),
        ],
    );
    m.put("m", 1, "ParticipantSetEntityData", &[]);
    m.put("m", 2, "ParticipantSetCounterEntityData", &[]);
    m.put("m", 3, "IntParticipantPropertyEntityData", &[]);
    m.put(
        "m",
        4,
        "IntParticipantPropertySorterEntityData",
        &[(
            "SortOrder",
            Value::Enum {
                value: 1,
                name: Some("Ascending".into()),
            },
        )],
    );
    m.put(
        "m",
        5,
        "IntParticipantPropertySorterQueryByIndexEntityData",
        &[("Index", Value::Int32(-1))],
    );
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("m")).unwrap();
    let b = extract(&mut g, &nodes, &[]).unwrap();
    assert_eq!(
        b.set_counters[0].source,
        Some(SetSource::Local(b.sets[0].identity.clone()))
    );
    assert_eq!(b.rank_queries[0].sorter, b.sorters[0].identity);
    assert_eq!(b.rank_queries[0].index, -1);
    assert_eq!(
        b.order,
        [
            Observer::Set(0),
            Observer::SetCounter(0),
            Observer::Sorter(0)
        ]
    );
    m.put("m", 1, "UnrelatedEntityData", &[]);
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("m")).unwrap();
    assert!(extract(&mut g, &nodes, &[]).is_err());
}
