// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};

fn site(i: usize) -> Site {
    Site {
        key: ("mode".into(), i),
        bus: ("mode".into(), 0),
        parents: vec![],
    }
}

#[test]
fn imported_states_follow_bound_scene_channels_and_preserve_owner_identity() {
    let (mut m, fields) = fixture();
    let mut lifecycle = extract(&mut Graph::new(&mut m), &fields).unwrap();
    m.put(
        "mode",
        6,
        "ParticipantStateEntityData",
        &[
            ("HandlesMarkedForDelete", Value::Bool(true)),
            ("SendRpcToClientSide", Value::Bool(false)),
        ],
    );
    m.put("mode", 7, "LinkedSchematicChannelEntityData", &[]);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(7), 77, local(6), 0),
                edge(local(7), 88, local(6), 0),
            ]),
        )],
    );
    let bindings = BTreeMap::from([(
        site(4),
        (
            site(7),
            Binding::Scene {
                content_key: 100,
                serializer: 700,
            },
        ),
    )]);
    resolve_imports(
        &mut lifecycle,
        &mut Graph::new(&mut m),
        &fields,
        &bindings,
        |_, s| Ok((100, 900 + s.key.1)),
    )
    .unwrap();
    let a = &lifecycle.imports[&Endpoint {
        serializer: 4,
        port: 77,
    }][0];
    assert_eq!(a.scene_content_key, 100);
    assert_eq!(a.state.serializer, 906);
    assert_eq!(a.identity, crate::scoring::events::Identity::from(&site(6)));
    assert_eq!(a.class, "ParticipantStateEntityData");
    assert_eq!(a.state.kind, StateKind::External);
    assert_eq!(
        a,
        &lifecycle.imports[&Endpoint {
            serializer: 4,
            port: 88
        }][0]
    );
    m.put(
        "mode",
        8,
        "StateSequenceParticipantStateEntityData",
        &[
            ("HandlesMarkedForDelete", Value::Bool(true)),
            ("SendRpcToClientSide", Value::Bool(false)),
        ],
    );
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(7), 77, local(6), 0),
                edge(local(7), 77, local(8), 0),
                edge(local(7), 88, local(6), 0),
            ]),
        )],
    );
    resolve_imports(
        &mut lifecycle,
        &mut Graph::new(&mut m),
        &fields,
        &bindings,
        |_, s| Ok((100, 900 + s.key.1)),
    )
    .unwrap();
    assert_eq!(
        lifecycle.imports[&Endpoint {
            serializer: 4,
            port: 77
        }]
            .iter()
            .map(|s| s.state.serializer)
            .collect::<Vec<_>>(),
        [906, 908]
    );
    let imported = &lifecycle.imports[&Endpoint {
        serializer: 4,
        port: 77,
    }][1];
    assert_eq!(imported.class, "StateSequenceParticipantStateEntityData");
    assert_eq!(imported.state.kind, StateKind::External);
    assert_eq!(imported.state.configuration, StateConfiguration::Default);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![edge(local(7), 77, local(7), 77)]),
        )],
    );
    assert!(
        resolve_imports(
            &mut lifecycle,
            &mut Graph::new(&mut m),
            &fields,
            &bindings,
            |_, _| Ok((100, 906))
        )
        .is_err()
    );
}

#[test]
fn finish_configuration_uses_the_bound_waypoint_laps_and_native_unbound_default() {
    let (mut m, fields) = fixture();
    let mut lifecycle = extract(&mut Graph::new(&mut m), &fields).unwrap();
    lifecycle.transitions[0].kind = TransitionKind::FinishLine;
    m.put("mode", 7, "ChannelWaypointListProviderEntityData", &[]);
    m.put(
        "mode",
        6,
        "WaypointListChannelEntityData",
        &[("NumberOfLaps", Value::Int32(3))],
    );
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(3), 0xfd69_801e, local(7), 0),
                    edge(local(7), 0x87b3_1017, local(4), 0),
                ]),
            ),
            ("PropertyConnections", Value::Array(vec![])),
        ],
    );
    let channels = BTreeMap::from([(
        site(4),
        (
            site(6),
            Binding::Scene {
                content_key: 100,
                serializer: 6,
            },
        ),
    )]);
    resolve_configurations(&mut lifecycle, &mut Graph::new(&mut m), &fields, &channels).unwrap();
    assert_eq!(
        lifecycle.transitions[0].configuration,
        TransitionConfiguration::FinishLine { laps: 3 }
    );
    m.put("mode", 6, "WrongEntityData", &[]);
    assert!(
        resolve_configurations(&mut lifecycle, &mut Graph::new(&mut m), &fields, &channels)
            .is_err()
    );
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[("LinkConnections", Value::Array(vec![]))],
    );
    resolve_configurations(&mut lifecycle, &mut Graph::new(&mut m), &fields, &channels).unwrap();
    assert_eq!(
        lifecycle.transitions[0].configuration,
        TransitionConfiguration::FinishLine { laps: 1 }
    );
}
pub(super) fn fixture() -> (Memory, Vec<FieldSite>) {
    let mut m = Memory::default();
    let classes = [
        "GameModeEntityData",
        "GameModeStateEntityData",
        "SimpleGameModeStateEntityData",
        "ParticipantTransitionEntityData",
        "LinkedSchematicChannelReferenceEntityData",
        "TriggeredParticipantTransitionEntityData",
    ];
    for (i, class) in classes.iter().enumerate() {
        m.put(
            "mode",
            i,
            class,
            &[
                ("HandlesMarkedForDelete", Value::Bool(false)),
                ("SendRpcToClientSide", Value::Bool(true)),
            ],
        );
    }
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(0), INITIAL, local(1), 0),
                edge(local(0), FINAL, local(2), 0),
                edge(local(0), STATES, local(1), 0),
                edge(local(0), STATES, local(2), 0),
                edge(local(0), TRANSITIONS, local(3), 0),
                edge(local(3), SOURCE, local(1), 0),
                edge(local(3), TARGET, local(2), 0),
                edge(local(5), SOURCE, local(4), 77),
                edge(local(5), TARGET, local(4), 88),
            ]),
        )],
    );
    let fields = classes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                site(i),
                (*c).into(),
                if i == 0 {
                    Kind::GameModeRoot
                } else {
                    Kind::Rpc
                },
            )
        })
        .collect();
    (m, fields)
}

#[test]
fn updateable_state_retains_its_update_set_and_rejects_a_clock_substitute() {
    let (mut m, _) = fixture();
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![edge(local(1), 0x5ba0_872d, local(6), 0)]),
        )],
    );
    m.put("mode", 6, "UpdateSetEntityData", &[]);
    let mut g = Graph::new(&mut m);
    let data = g.object(&site(1)).unwrap();
    let config = state_configuration(&mut g, &site(1), StateKind::Updateable, &data).unwrap();
    assert_eq!(
        config,
        StateConfiguration::Updateable {
            update_set: crate::scoring::events::Identity::from(&site(6))
        }
    );
    m.put("mode", 6, "GameTimeEntityData", &[]);
    assert!(
        state_configuration(
            &mut Graph::new(&mut m),
            &site(1),
            StateKind::Updateable,
            &data
        )
        .is_err()
    );
}

#[test]
fn preserves_authored_order_and_external_transition_ports_without_inventing_a_parent() {
    let (mut m, fields) = fixture();
    let l = extract(&mut Graph::new(&mut m), &fields).unwrap();
    assert_eq!((l.initial, l.final_state, l.root_state), (1, 2, None));
    assert_eq!(l.registered_states, vec![1, 2]);
    assert_eq!(l.registered_transitions, vec![3]);
    assert_eq!(l.transitions.len(), 2);
    assert_eq!(l.transitions[0].kind, TransitionKind::Completed);
    assert_eq!(
        l.transitions[0].sources,
        vec![Endpoint {
            serializer: 1,
            port: 0
        }]
    );
    assert_eq!(
        l.transitions[1].sources,
        vec![Endpoint {
            serializer: 4,
            port: 77
        }]
    );
    assert_eq!(
        l.transitions[1].target,
        Some(Endpoint {
            serializer: 4,
            port: 88
        })
    );
    assert!(
        l.states
            .iter()
            .all(|s| s.send_rpc_to_client && !s.handles_marked_for_delete)
    );
}

#[test]
fn rejects_missing_duplicate_wrong_class_and_foreign_lifecycle_bindings() {
    for bad_edges in [
        vec![],
        vec![
            edge(local(0), INITIAL, local(1), 0),
            edge(local(0), INITIAL, local(2), 0),
        ],
        vec![edge(local(0), STATES, local(3), 0)],
        vec![
            edge(local(0), STATES, local(1), 0),
            edge(local(0), STATES, local(1), 0),
        ],
        vec![edge(local(3), SOURCE, local(0), 0)],
        vec![edge(local(3), SOURCE, local(4), 0)],
    ] {
        let (mut m, fields) = fixture();
        m.put(
            "mode",
            0,
            "GameModeEntityData",
            &[("LinkConnections", Value::Array(bad_edges))],
        );
        assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
    }
    let (mut m, mut fields) = fixture();
    fields.push(fields[1].clone());
    assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
    let (mut m, fields) = fixture();
    m.put(
        "mode",
        1,
        "WaitForAllAiToSpawnStateEntityData",
        &[("HandlesMarkedForDelete", Value::Int32(0))],
    );
    assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
}

#[test]
fn initial_setup_state_is_independent_of_registered_states() {
    let (mut m, mut fields) = fixture();
    fields.truncate(4);
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(0), INITIAL, local(1), 0),
                edge(local(0), FINAL, local(2), 0),
                edge(local(0), STATES, local(2), 0),
                edge(local(0), TRANSITIONS, local(3), 0),
                edge(local(3), SOURCE, local(1), 0),
                edge(local(3), TARGET, local(2), 0),
            ]),
        )],
    );
    let lifecycle = extract(&mut Graph::new(&mut m), &fields).unwrap();
    assert_eq!(lifecycle.initial, 1);
    assert_eq!(lifecycle.registered_states, [2]);
    assert!(lifecycle.states.iter().any(|s| s.serializer == 1));
    assert_eq!(
        lifecycle.transitions[0].sources,
        [Endpoint {
            serializer: 1,
            port: 0
        }]
    );
    // The initial target must still be a real local state, never a channel or
    // a transition masquerading as one merely because registration is absent.
    for wrong in [3, 4, 99] {
        let (mut invalid, _) = fixture();
        invalid.put(
            "mode",
            0,
            "GameModeEntityData",
            &[(
                "LinkConnections",
                Value::Array(vec![
                    edge(local(0), INITIAL, local(wrong), 0),
                    edge(local(0), FINAL, local(2), 0),
                    edge(local(0), STATES, local(2), 0),
                ]),
            )],
        );
        assert!(extract(&mut Graph::new(&mut invalid), &fields).is_err());
    }
}

#[test]
fn repeated_prefabs_resolve_to_distinct_state_serializers() {
    let mut m = Memory::default();
    m.put(
        "mode",
        0,
        "GameModeEntityData",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(0), INITIAL, local(10), 7),
                edge(local(0), FINAL, local(11), 7),
                edge(local(0), STATES, local(11), 7),
                edge(local(0), STATES, local(10), 7),
            ]),
        )],
    );
    for i in [10, 11] {
        m.put(
            "mode",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("c", 0))],
        );
    }
    m.put(
        "c",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Interface", local(2)),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(2), 7, local(1), 0)]),
            ),
        ],
    );
    m.put(
        "c",
        1,
        "GameModeStateEntityData",
        &[
            ("HandlesMarkedForDelete", Value::Bool(true)),
            ("SendRpcToClientSide", Value::Bool(false)),
        ],
    );
    m.put("c", 2, "InterfaceDescriptorData", &[]);
    let mut fields = vec![(site(0), "GameModeEntityData".into(), Kind::GameModeRoot)];
    for i in [10, 11] {
        fields.push((
            Site {
                key: ("c".into(), 1),
                bus: ("c".into(), 0),
                parents: vec![Frame {
                    bus: ("mode".into(), 0),
                    reference: ("mode".into(), i),
                }],
            },
            "GameModeStateEntityData".into(),
            Kind::RpcReferences,
        ));
    }
    let result = extract(&mut Graph::new(&mut m), &fields).unwrap();
    assert_eq!((result.initial, result.final_state), (1, 2));
    assert_eq!(result.registered_states, vec![2, 1]);
    assert_eq!(result.states.len(), 2);
}

#[test]
fn repeated_authored_registrations_are_preserved() {
    let (mut m, fields) = fixture();
    let root = m.0.get_mut(&("mode".into(), 0)).unwrap();
    let Value::Array(edges) = &mut root.fields.0[0].1 else {
        panic!()
    };
    edges.push(edge(local(0), TRANSITIONS, local(3), 0));
    let result = extract(&mut Graph::new(&mut m), &fields).unwrap();
    assert_eq!(result.registered_transitions, vec![3, 3]);
    assert_eq!(result.transitions.len(), 2);
}

fn manager(m: &mut Memory, source: usize) {
    use nfs_frostbite::ebx::Fields;
    m.put(
        "mode",
        4,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            Value::Array(vec![Value::Struct(Fields(vec![(
                "Id".into(),
                Value::Int32(77),
            )]))]),
        )],
    );
    let root = m.0.get_mut(&("mode".into(), 0)).unwrap();
    let Value::Array(edges) = &mut root.fields.0[0].1 else {
        panic!()
    };
    edges.push(edge(local(source), 0x1527_581a, local(4), 77));
}

#[test]
fn ai_filter_keeps_separate_human_and_ai_targets_and_requires_a_declared_manager() {
    let (mut m, mut fields) = fixture();
    fields[3].1 = "AiParticipantFilterTransitionEntityData".into();
    m.put("mode", 3, &fields[3].1, &[]);
    manager(&mut m, 3);
    let root = m.0.get_mut(&("mode".into(), 0)).unwrap();
    let Value::Array(edges) = &mut root.fields.0[0].1 else {
        panic!()
    };
    edges.push(edge(local(3), 0x5e32_a388, local(1), 0));
    let l = extract(&mut Graph::new(&mut m), &fields).unwrap();
    assert_eq!(
        l.transitions[0].target,
        Some(Endpoint {
            serializer: 2,
            port: 0
        })
    );
    assert_eq!(
        l.transitions[0].configuration,
        TransitionConfiguration::AiFilter {
            manager: Property {
                channel: "c".into(),
                port: 77
            },
            human_target: Some(Endpoint {
                serializer: 1,
                port: 0
            }),
        }
    );
    let root = m.0.get_mut(&("mode".into(), 0)).unwrap();
    let Value::Array(edges) = &mut root.fields.0[0].1 else {
        panic!()
    };
    edges.push(edge(local(3), 0x5e32_a388, local(2), 0));
    assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
}

#[test]
fn sequence_ids_countdown_defaults_and_waypoint_settings_come_from_the_state() {
    for (class, field, value) in [
        ("CountdownGameModeStateEntityData", "CountdownTime", 0.),
        (
            "ParticipantSequenceSpawnerGameModeStateEntityData",
            "Unused",
            0.,
        ),
        (
            "WaypointsGameModeStateEntityData",
            "AllowedWrongDirectionDistance",
            300.,
        ),
    ] {
        let (mut m, mut fields) = fixture();
        fields[1].1 = class.into();
        m.put("a", 0, "StateSequenceId", &[]);
        m.put(
            "mode",
            1,
            class,
            &[
                ("HandlesMarkedForDelete", Value::Bool(false)),
                ("SendRpcToClientSide", Value::Bool(true)),
                ("SequenceId", ptr("a", 0)),
                (field, Value::Float32(value)),
            ],
        );
        let l = extract(&mut Graph::new(&mut m), &fields).unwrap();
        let expected = match class {
            "CountdownGameModeStateEntityData" => StateConfiguration::Countdown {
                sequence: ("a".into(), 0),
                seconds: 0.,
            },
            "ParticipantSequenceSpawnerGameModeStateEntityData" => StateConfiguration::Sequence {
                sequence: ("a".into(), 0),
            },
            _ => StateConfiguration::Waypoints {
                allowed_wrong_direction_distance: 300.,
            },
        };
        assert_eq!(l.states[0].configuration, expected);
        if class != "WaypointsGameModeStateEntityData" {
            m.put("a", 0, "UnrelatedSequence", &[]);
            assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
        } else {
            let data = m.0.get_mut(&("mode".into(), 1)).unwrap();
            data.fields
                .0
                .iter_mut()
                .find(|(key, _)| key == field)
                .unwrap()
                .1 = Value::Float32(f32::NAN);
            assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
        }
    }
}

#[test]
fn wait_for_ai_and_dnf_retain_their_manager_and_timeout() {
    let (mut m, mut fields) = fixture();
    fields[1].1 = "WaitForAllAiToSpawnStateEntityData".into();
    m.0.get_mut(&("mode".into(), 1)).unwrap().class = fields[1].1.clone();
    manager(&mut m, 1);
    fields[3].1 = "DNFGameModeStateTransitionEntityData".into();
    m.put(
        "mode",
        3,
        &fields[3].1,
        &[("TimeOutDuration", Value::Float32(80.))],
    );
    let l = extract(&mut Graph::new(&mut m), &fields).unwrap();
    assert_eq!(
        l.states[0].configuration,
        StateConfiguration::WaitForAi {
            manager: Property {
                channel: "c".into(),
                port: 77
            },
        }
    );
    assert_eq!(
        l.transitions[0].configuration,
        TransitionConfiguration::Dnf {
            timeout_seconds: 80.
        }
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[("Links", Value::Array(vec![]))],
    );
    assert!(extract(&mut Graph::new(&mut m), &fields).is_err());
}
