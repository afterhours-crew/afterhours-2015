// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use data::{
    Endpoint, State, StateConfiguration, StateKind, Transition, TransitionConfiguration,
    TransitionKind, bindings::*,
};

pub(super) fn identity(index: usize) -> Identity {
    Identity {
        asset: "fixture".into(),
        index,
        instances: vec![],
    }
}
fn endpoint(serializer: usize) -> Endpoint {
    Endpoint {
        serializer,
        port: 0,
    }
}
fn property() -> Property {
    Property {
        channel: "fixture-channel".into(),
        port: 1,
    }
}
fn link(index: usize) -> Link {
    Link {
        identity: identity(index),
        port: 0,
        class: "fixture".into(),
        channel: None,
    }
}
fn state(serializer: usize, kind: StateKind) -> State {
    State {
        serializer,
        kind,
        configuration: StateConfiguration::Default,
        handles_marked_for_delete: true,
        send_rpc_to_client: true,
    }
}
pub(super) fn definition() -> data::Lifecycle {
    data::Lifecycle {
        imports: BTreeMap::new(),
        bindings: Bindings::default(),
        initial: 1,
        final_state: 3,
        root_state: None,
        states: vec![
            state(1, StateKind::GameMode),
            state(2, StateKind::Waypoints),
            state(3, StateKind::Simple),
        ],
        transitions: vec![Transition {
            serializer: 4,
            kind: TransitionKind::Completed,
            sources: vec![endpoint(1)],
            target: Some(endpoint(2)),
            configuration: TransitionConfiguration::Default,
        }],
        registered_states: vec![1, 2, 3],
        registered_transitions: vec![4],
    }
}
pub(super) fn plan(mut data: data::Lifecycle) -> Arc<Plan> {
    if data.bindings.order.is_empty() {
        data.bindings.order = vec![Observer::Transition(4)];
    }
    Arc::new(Plan::new(&data, |_| Err(Error::UnknownState)).unwrap())
}
fn enter(state: StateId, participant: Participant) -> Action {
    Action::Enter { state, participant }
}
pub(super) fn inert(
    _: &dyn nfs_world::participant_states::Membership,
    _: &Effect,
) -> Result<Vec<Action>, Error> {
    Ok(vec![])
}

pub(super) fn ranked() -> Arc<Plan> {
    use nfs_world::scoring::rank_sorter::Order;
    let mut d = definition();
    d.transitions.clear();
    d.bindings.integers = vec![identity(80)];
    d.bindings.sorters = [Order::Ascending, Order::Descending]
        .into_iter()
        .enumerate()
        .map(|(i, order)| IntegerSorter {
            identity: identity(81 + i),
            source: IntegerSource::Local(identity(80)),
            order,
        })
        .collect();
    d.bindings.order = vec![Observer::Sorter(0), Observer::Sorter(1)];
    plan(d)
}

#[test]
fn results_collect_all_tag_members_and_preserve_missing_scores_and_context() {
    use data::bindings::results::{Definition, TRIGGER, Tag};
    use nfs_world::scoring::events::Context;
    let mut d = definition();
    d.bindings.integers = vec![identity(80), identity(81)];
    let a = IntegerSource::Local(identity(80));
    let b = IntegerSource::Local(identity(81));
    let tag = |n| Tag {
        asset: format!("tag{n}"),
        object: 0,
        name: format!("score{n}"),
        hash: n,
    };
    d.bindings.results.push(Definition {
        identity: identity(90),
        collector: link(99),
        scores: vec![
            (tag(1), Some(a.clone())),
            (tag(2), Some(b.clone())),
            (tag(3), None),
            (tag(1), Some(a.clone())),
        ],
    });
    let mut h = Host::new(plan(d)).unwrap();
    for (source, p, v) in [(a.clone(), 7, 100), (a.clone(), 9, 200), (b, 9, -50)] {
        h.integer(source, p, v, &mut inert).unwrap();
    }
    assert!(
        h.result_event(&identity(90), TRIGGER, None, &mut inert)
            .unwrap()
            .is_empty()
    );
    assert!(
        h.result_event(
            &identity(90),
            TRIGGER,
            Some(Context::Integer(7)),
            &mut inert
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        h.result_event(
            &identity(90),
            TRIGGER + 1,
            Some(Context::Participant(7)),
            &mut inert
        )
        .unwrap()
        .is_empty()
    );
    let mut result = None;
    h.result_event(
        &identity(90),
        TRIGGER,
        Some(Context::Participant(99)),
        &mut |_, effect| {
            if let Effect::Results(snapshot) = effect {
                result = Some(snapshot.clone());
            }
            Ok(vec![])
        },
    )
    .unwrap();
    let result = result.unwrap();
    assert_eq!(result.triggering_participant, 99);
    assert_eq!(result.values.keys().copied().collect::<Vec<_>>(), [7, 9]);
    assert_eq!(result.values[&7], BTreeMap::from([(tag(1), 100)]));
    assert_eq!(
        result.values[&9],
        BTreeMap::from([(tag(1), 200), (tag(2), -50)])
    );
    h.integer(a, 7, 300, &mut inert).unwrap();
    assert_eq!(result.values[&7][&tag(1)], 100);
    h.remove_integer_participant(9, &mut inert).unwrap();
    let out = h
        .result_event(
            &identity(90),
            TRIGGER,
            Some(Context::Participant(7)),
            &mut inert,
        )
        .unwrap();
    let Effect::Results(result) = &out[0] else {
        panic!()
    };
    assert_eq!(result.values.len(), 1);
    assert_eq!(result.values[&7][&tag(1)], 300);
}

#[test]
fn ranking_callbacks_run_in_order_and_source_failure_rolls_back() {
    let mut host = Host::new(ranked()).unwrap();
    let source = IntegerSource::Local(identity(80));
    host.integer(source.clone(), 7, 100, &mut inert).unwrap();
    let mut visited = Vec::new();
    host.integer(source.clone(), 9, 50, &mut |machine, effect| {
        if let Effect::Ranking(effect) = effect {
            visited.push(effect.sorter.index);
            if effect.sorter == identity(81) {
                return Ok(vec![enter(2, 7)]);
            }
            assert!(machine.members(2)?.contains(&7));
        }
        Ok(vec![])
    })
    .unwrap();
    assert_eq!(visited, [81, 82]);
    assert_eq!(host.ranking(0).unwrap()[0].participant, 9);
    assert_eq!(host.ranking(1).unwrap()[0].participant, 7);
    assert!(
        host.integer(source.clone(), 7, 100, &mut inert)
            .unwrap()
            .is_empty()
    );
    let before = host.ranking(0).unwrap().to_vec();
    assert_eq!(
        host.integer(source.clone(), 7, -1, &mut |_, _| Err(Error::Invalid)),
        Err(Error::Invalid)
    );
    assert_eq!(host.integer_value(&source, 7), Some(100));
    assert_eq!(host.ranking(0).unwrap(), before);
}

#[test]
fn ranking_activation_does_not_seed_and_destruction_clears_results() {
    let source = IntegerSource::Local(identity(80));
    let (mut host, effects) = Host::from_scene(
        ranked(),
        SceneInputs {
            integers: BTreeMap::from([((source.clone(), 7), 100)]),
            ..Default::default()
        },
        &mut inert,
    )
    .unwrap();
    assert!(effects.is_empty());
    assert!(host.ranking(0).unwrap().is_empty());
    assert!(
        host.integer(source.clone(), 7, 100, &mut inert)
            .unwrap()
            .is_empty()
    );
    host.integer(source.clone(), 7, 101, &mut inert).unwrap();
    host.apply(
        &[
            enter(2, 7),
            Action::Move {
                source: 2,
                participant: 7,
                target: None,
            },
        ],
        &mut inert,
    )
    .unwrap();
    assert_eq!(host.ranking(0).unwrap().len(), 1);
    let removed = host.remove_integer_participant(7, &mut inert).unwrap();
    assert_eq!(
        removed
            .iter()
            .filter(|e| matches!(e, Effect::Ranking(_)))
            .count(),
        2
    );
    assert!(host.ranking(0).unwrap().is_empty());
    assert_eq!(host.integer_value(&source, 7), None);
    assert!(
        host.remove_integer_participant(7, &mut inert)
            .unwrap()
            .is_empty()
    );
    let other = Host::new(ranked()).unwrap();
    assert!(other.ranking(0).unwrap().is_empty());
}

#[test]
fn reporters_publish_counts_before_events_and_preserve_internal_transitions() {
    let mut d = definition();
    d.transitions.clear();
    d.states
        .iter_mut()
        .for_each(|s| s.kind = StateKind::Waypoints);
    d.bindings.reporters = vec![
        StateReporter {
            identity: identity(70),
            compound: false,
            states: vec![endpoint(1)],
        },
        StateReporter {
            identity: identity(71),
            compound: true,
            states: vec![endpoint(1), endpoint(2), endpoint(1)],
        },
    ];
    d.bindings.order = vec![Observer::Reporter(0), Observer::Reporter(1)];
    let (mut h, startup) = Host::from_scene(plan(d), SceneInputs::default(), &mut inert).unwrap();
    assert_eq!(
        startup,
        vec![
            Effect::Reporter(reporters::Effect::Count {
                identity: identity(70),
                value: 0
            }),
            Effect::Reporter(reporters::Effect::Count {
                identity: identity(71),
                value: 0
            }),
        ]
    );
    h.apply(&[enter(1, 7), enter(2, 7)], &mut inert).unwrap();
    let mut counts = Vec::new();
    let events = h
        .apply(
            &[Action::Move {
                source: 1,
                participant: 7,
                target: Some(2),
            }],
            &mut |machine, effect| {
                if let Effect::Reporter(reporters::Effect::Count {
                    identity: id,
                    value,
                }) = effect
                {
                    assert!(!machine.members(1)?.contains(&7));
                    counts.push((id.index, *value));
                }
                Ok(vec![])
            },
        )
        .unwrap();
    // Entering an existing member still reports entry in the native state.
    assert_eq!(counts, vec![(70, 0), (71, 1), (71, 1)]);
    assert!(events.contains(&Effect::Reporter(reporters::Effect::Event {
        identity: identity(70),
        event: reporters::EXIT,
        participant: 7,
    })));
    assert!(!events.iter().any(|e| matches!(
        e,
        Effect::Reporter(reporters::Effect::Event {
            event: reporters::COMPOUND_EXIT,
            ..
        })
    )));
    assert!(events.contains(&Effect::Reporter(reporters::Effect::Event {
        identity: identity(71),
        event: reporters::COMPOUND_ENTER,
        participant: 7,
    })));
    let output = h
        .apply(
            &[Action::Move {
                source: 2,
                participant: 7,
                target: None,
            }],
            &mut inert,
        )
        .unwrap();
    assert!(output.contains(&Effect::Reporter(reporters::Effect::Count {
        identity: identity(71),
        value: 0
    })));
    assert!(output.contains(&Effect::Reporter(reporters::Effect::Event {
        identity: identity(71),
        event: reporters::COMPOUND_EXIT,
        participant: 7,
    })));
}

#[test]
fn reporter_completion_does_not_publish_count_and_failure_is_atomic() {
    let mut d = definition();
    d.transitions.clear();
    d.states[0].kind = StateKind::Waypoints;
    d.bindings.reporters.push(StateReporter {
        identity: identity(70),
        compound: true,
        states: vec![endpoint(1), endpoint(2)],
    });
    d.bindings.order = vec![Observer::Reporter(0)];
    let mut h = Host::new(plan(d)).unwrap();
    let output = h.apply(&[enter(1, 7), enter(2, 7)], &mut inert).unwrap();
    assert!(output.contains(&Effect::Reporter(reporters::Effect::Count {
        identity: identity(70),
        value: 2
    })));
    let complete = [Action::Complete {
        state: 1,
        participant: 7,
    }];
    assert_eq!(
        h.apply(&complete, &mut |_, effect| {
            if matches!(effect, Effect::Reporter(_)) {
                Err(Error::Invalid)
            } else {
                Ok(vec![])
            }
        }),
        Err(Error::Invalid)
    );
    assert!(h.machine().completed(1).unwrap().is_empty());
    let output = h.apply(&complete, &mut inert).unwrap();
    assert_eq!(
        output
            .iter()
            .filter_map(|e| if let Effect::Reporter(e) = e {
                Some(e.clone())
            } else {
                None
            })
            .collect::<Vec<_>>(),
        vec![reporters::Effect::Event {
            identity: identity(70),
            event: reporters::COMPLETED,
            participant: 7,
        }]
    );
}

fn watched() -> data::Lifecycle {
    let mut d = definition();
    d.bindings.properties.push(StateProperty {
        identity: identity(10),
        definition: properties::Definition::List(vec![endpoint(1), endpoint(2)]),
    });
    for i in 0..2 {
        d.bindings.spawners.push(Spawner {
            identity: identity(20 + i),
            blueprint: "same-blueprint".into(),
            occurrence: i,
            controller: Some(Controller::Boolean(link(10))),
        });
    }
    d.bindings.contexts.push(StateContext {
        identity: identity(30),
        states: vec![endpoint(1), endpoint(2)],
        manager: property(),
        categories: vec![10, 20],
    });
    d.bindings.order = vec![
        Observer::Property(0),
        Observer::Spawner(0),
        Observer::Spawner(1),
        Observer::Context(0),
        Observer::Transition(4),
    ];
    d
}

#[test]
fn activation_seeds_existing_scene_members_in_registration_order_without_entry_rpcs() {
    let mut data = watched();
    data.bindings.order = vec![
        Observer::Spawner(0),
        Observer::Property(0),
        Observer::Spawner(1),
        Observer::Context(0),
        Observer::Transition(4),
    ];
    let mut plan = (*plan(data)).clone();
    plan.external.insert(
        1,
        data::ImportedState {
            class: "ParticipantStateEntityData".into(),
            identity: identity(90),
            scene_content_key: 90,
            state: state(9, StateKind::Participant),
        },
    );
    let mut creates = Vec::new();
    let (host, effects) = Host::from_scene(
        Arc::new(plan),
        SceneInputs {
            members: vec![(1, 7, false)],
            ..Default::default()
        },
        &mut |machine, effect| {
            if let Effect::Logic {
                spawner,
                participant,
                create: true,
            } = *effect
            {
                creates.push(spawner);
                if spawner == 0 {
                    assert!(machine.members(1)?.contains(&participant));
                    return Ok(vec![Action::Move {
                        source: 1,
                        participant,
                        target: Some(2),
                    }]);
                }
                assert!(!machine.members(1)?.contains(&participant));
                assert!(machine.members(2)?.contains(&participant));
            }
            Ok(vec![])
        },
    )
    .unwrap();
    assert_eq!(creates, [0, 1]);
    assert_eq!(host.machine.members(2).unwrap(), &BTreeSet::from([7]));
    assert!(!effects.iter().any(|e| matches!(
        e,
        Effect::State(Notification::Entered { state: 1, .. }) | Effect::Context { .. }
    )));
    assert_eq!(host.observers.properties[0].value(7), Some(true));
}

#[test]
fn bind_predicates_see_later_adapter_seeds_but_boolean_predicates_do_not_seed() {
    let mut data = definition();
    let channel = Link {
        channel: Some(property()),
        port: property().port,
        ..link(100)
    };
    data.bindings.sets = vec![
        ParticipantSet {
            identity: identity(10),
            kind: SetKind::Boolean(channel.clone()),
        },
        ParticipantSet {
            identity: identity(11),
            kind: SetKind::Predicate {
                operation: Operation::Union,
                operands: vec![SetSource::Local(identity(10))],
            },
        },
    ];
    data.bindings.predicates.push(Predicate {
        identity: identity(12),
        kind: PredicateKind::Boolean {
            operation: BooleanOperation::Not,
            operands: vec![channel],
        },
    });
    data.bindings.spawners.push(Spawner {
        identity: identity(13),
        blueprint: "set-consumer".into(),
        occurrence: 0,
        controller: Some(Controller::Set(link(11))),
    });
    data.bindings.order = vec![
        Observer::Spawner(0),
        Observer::Set(0),
        Observer::Predicate(0),
        Observer::Set(1),
        Observer::Transition(4),
    ];
    let (mut host, output) = Host::from_scene(
        plan(data),
        SceneInputs {
            booleans: BTreeMap::from([((property(), 7), true)]),
            ..Default::default()
        },
        &mut inert,
    )
    .unwrap();
    assert_eq!(host.logic_members(0).unwrap(), &BTreeSet::from([7]));
    assert!(host.observers.predicates[0].is_empty());
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::Logic { create: true, .. }))
            .count(),
        1
    );
    host.boolean(property(), 7, false, &mut inert).unwrap();
    assert!(host.logic_members(0).unwrap().is_empty());
    assert_eq!(host.observers.predicates[0].get(&7), Some(&true));
}

#[test]
fn scene_snapshot_rejects_local_membership_and_rolls_back_failed_activation() {
    assert!(matches!(
        Host::from_scene(
            plan(watched()),
            SceneInputs {
                members: vec![(1, 7, false)],
                ..Default::default()
            },
            &mut inert
        ),
        Err(Error::UnknownState)
    ));
    let mut p = (*plan(watched())).clone();
    p.external.insert(
        1,
        data::ImportedState {
            class: "ParticipantStateEntityData".into(),
            identity: identity(99),
            scene_content_key: 99,
            state: state(9, StateKind::Participant),
        },
    );
    let p = Arc::new(p);
    let scene = SceneInputs {
        members: vec![(1, 7, false)],
        ..Default::default()
    };
    assert!(matches!(
        Host::from_scene(p.clone(), scene.clone(), &mut |_, effect| {
            if matches!(effect, Effect::Logic { .. }) {
                Err(Error::Invalid)
            } else {
                Ok(vec![])
            }
        }),
        Err(Error::Invalid)
    ));
    let (host, _) = Host::from_scene(p, scene, &mut inert).unwrap();
    assert_eq!(host.logic_members(0).unwrap(), &BTreeSet::from([7]));
    assert!(host.machine.completed(1).unwrap().is_empty());
}

#[test]
fn synchronous_logic_start_finishes_before_the_next_listener_without_a_lifetime_gap() {
    let mut host = Host::new(plan(watched())).unwrap();
    let mut created = Vec::new();
    let output = host
        .apply(&[enter(1, 7)], &mut |machine, effect| {
            if let Effect::Logic {
                spawner,
                participant,
                create: true,
            } = *effect
            {
                created.push(spawner);
                assert!(machine.members(1)?.contains(&participant));
                if spawner == 0 {
                    assert!(!machine.completed(1)?.contains(&participant));
                    return Ok(vec![Action::Complete {
                        state: 1,
                        participant,
                    }]);
                }
                assert!(machine.completed(1)?.contains(&participant));
            }
            Ok(vec![])
        })
        .unwrap();
    assert_eq!(created, [0, 1]);
    assert_eq!(host.machine().members(2).unwrap(), &BTreeSet::from([7]));
    assert!(host.machine().members(1).unwrap().is_empty());
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::Logic { create: false, .. }))
            .count(),
        0
    );
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::Context { .. }))
            .count(),
        1
    );
    let output = host
        .apply(
            &[Action::Move {
                source: 2,
                participant: 7,
                target: Some(3),
            }],
            &mut inert,
        )
        .unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::Logic { create: false, .. }))
            .count(),
        2
    );
    assert!(host.logic_members(0).unwrap().is_empty());
    assert!(
        output
            .iter()
            .any(|e| matches!(e, Effect::Context { change, .. } if change.categories.is_empty()))
    );
    assert!(host.machine().completed(3).unwrap().contains(&7));
}

#[test]
fn failed_replication_or_logic_start_rolls_back_membership_and_all_lifetimes() {
    let mut host = Host::new(plan(watched())).unwrap();
    assert_eq!(
        host.apply(&[enter(1, 7)], &mut |_, e| {
            if matches!(e, Effect::Logic { spawner: 1, .. }) {
                Err(Error::Bound)
            } else {
                Ok(vec![])
            }
        }),
        Err(Error::Bound)
    );
    assert!(host.machine().members(1).unwrap().is_empty());
    assert!(host.logic_members(0).unwrap().is_empty());
    let output = host.apply(&[enter(1, 7)], &mut inert).unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::Logic { create: true, .. }))
            .count(),
        2
    );
}

#[test]
fn owned_sets_difference_player_adapters_and_two_sessions_are_isolated() {
    let mut d = definition();
    d.bindings.sets = vec![
        ParticipantSet {
            identity: identity(10),
            kind: SetKind::Owned,
        },
        ParticipantSet {
            identity: identity(11),
            kind: SetKind::Players {
                manager: property(),
            },
        },
        ParticipantSet {
            identity: identity(12),
            kind: SetKind::Predicate {
                operation: Operation::Difference,
                operands: vec![
                    SetSource::Local(identity(10)),
                    SetSource::Local(identity(11)),
                ],
            },
        },
    ];
    d.bindings.spawners.push(Spawner {
        identity: identity(20),
        blueprint: "logic".into(),
        occurrence: 0,
        controller: Some(Controller::Set(link(12))),
    });
    d.bindings.order = vec![
        Observer::Set(0),
        Observer::Set(1),
        Observer::Set(2),
        Observer::Spawner(0),
        Observer::Transition(4),
    ];
    let plan = plan(d);
    let mut a = Host::new(plan.clone()).unwrap();
    let b = Host::new(plan).unwrap();
    a.set(SetSource::Local(identity(10)), 7, true, &mut inert)
        .unwrap();
    assert_eq!(a.logic_members(0).unwrap(), &BTreeSet::from([7]));
    assert!(
        a.set(SetSource::Local(identity(10)), 7, true, &mut inert)
            .unwrap()
            .is_empty()
    );
    a.set(SetSource::Local(identity(10)), 8, true, &mut inert)
        .unwrap();
    a.players(property(), 7, true, &mut inert).unwrap();
    assert_eq!(a.logic_members(0).unwrap(), &BTreeSet::from([8]));
    a.players(property(), 7, false, &mut inert).unwrap();
    assert_eq!(a.logic_members(0).unwrap(), &BTreeSet::from([7, 8]));
    assert!(b.logic_members(0).unwrap().is_empty());
    assert_eq!(
        a.set(SetSource::Local(identity(12)), 7, true, &mut inert),
        Err(Error::Invalid)
    );
}

#[test]
fn wait_for_ai_and_filter_use_manager_membership_and_distinct_human_target() {
    let mut d = definition();
    d.states[0].kind = StateKind::WaitForAi;
    d.states[0].configuration = StateConfiguration::WaitForAi {
        manager: property(),
    };
    d.transitions[0].kind = TransitionKind::AiFilter;
    d.transitions[0].configuration = TransitionConfiguration::AiFilter {
        manager: property(),
        human_target: Some(endpoint(3)),
    };
    let mut h = Host::new(plan(d)).unwrap();
    h.ai_member(property(), 8, true).unwrap();
    h.apply(&[enter(1, 7), enter(1, 8)], &mut inert).unwrap();
    assert_eq!(h.machine().members(1).unwrap(), &BTreeSet::from([8]));
    assert_eq!(h.machine().members(3).unwrap(), &BTreeSet::from([7]));
    h.apply(
        &[Action::Complete {
            state: 1,
            participant: 8,
        }],
        &mut inert,
    )
    .unwrap();
    assert_eq!(h.machine().members(2).unwrap(), &BTreeSet::from([8]));
}

#[test]
fn channel_endpoints_must_resolve_and_finish_dnf_are_not_completion_transitions() {
    let mut d = definition();
    d.transitions[0].sources = vec![Endpoint {
        serializer: 99,
        port: 17,
    }];
    d.bindings.order = vec![Observer::Transition(4)];
    assert!(Plan::new(&d, |_| Err(Error::UnknownState)).is_err());
    assert!(Plan::new(&d, |_| Ok(99)).is_err());
    for kind in [
        TransitionKind::Triggered,
        TransitionKind::FinishLine,
        TransitionKind::Dnf,
        TransitionKind::DnfTriggered,
    ] {
        d.transitions[0].kind = kind;
        d.bindings.clocks.clear();
        d.transitions[0].configuration = if kind == TransitionKind::Dnf {
            d.bindings.clocks.insert(4, property());
            TransitionConfiguration::Dnf {
                timeout_seconds: 80.,
            }
        } else {
            TransitionConfiguration::Default
        };
        let p = Plan::new(&d, |e| {
            assert_eq!(e.port, 17);
            Ok(1)
        })
        .unwrap();
        let mut h = Host::new(Arc::new(p)).unwrap();
        h.clocks(clock(0., 0.)).unwrap();
        h.apply(&[enter(1, 7)], &mut inert).unwrap();
        assert!(h.machine().members(1).unwrap().contains(&7));
        assert!(h.machine().members(2).unwrap().is_empty());
    }
    d.bindings.order.push(Observer::Transition(4));
    assert!(Plan::new(&d, |_| Ok(1)).is_err());
}

pub(super) fn clock(receipt_seconds: f64, game_seconds: f64) -> timed::Clocks {
    timed::Clocks {
        receipt_seconds,
        channels: BTreeMap::from([(property(), game_seconds)]),
    }
}

pub(super) fn timed_definition() -> data::Lifecycle {
    let mut d = definition();
    d.transitions = vec![
        Transition {
            serializer: 4,
            kind: TransitionKind::Dnf,
            sources: vec![endpoint(2)],
            target: Some(endpoint(3)),
            configuration: TransitionConfiguration::Dnf {
                timeout_seconds: 10.,
            },
        },
        Transition {
            serializer: 5,
            kind: TransitionKind::DnfTriggered,
            sources: vec![endpoint(1), endpoint(2)],
            target: Some(endpoint(3)),
            configuration: TransitionConfiguration::Default,
        },
        Transition {
            serializer: 6,
            kind: TransitionKind::FinishLine,
            sources: vec![endpoint(2)],
            target: Some(endpoint(3)),
            configuration: TransitionConfiguration::FinishLine { laps: 2 },
        },
    ];
    d.bindings.clocks.insert(4, property());
    d.bindings.order = vec![
        Observer::Transition(4),
        Observer::Transition(5),
        Observer::Transition(6),
    ];
    d
}

#[test]
fn dnf_uses_owned_clock_refreshes_on_waypoints_and_cleans_departed_participants() {
    let mut h = Host::new(plan(timed_definition())).unwrap();
    h.clocks(clock(100., 5.)).unwrap();
    let out = h.apply(&[enter(2, 7), enter(2, 8)], &mut inert).unwrap();
    assert_eq!(
        out.iter()
            .filter(|e| matches!(
                e,
                Effect::Timing(timed::Effect::DnfDeadline { deadline: 15., .. })
            ))
            .count(),
        2
    );
    h.clocks(clock(110., 14.)).unwrap();
    assert!(h.update_transition(4, &mut inert).unwrap().is_empty());
    let out = h.waypoint(2, 7, 2, 999., &mut inert).unwrap();
    assert!(out.iter().any(|e| matches!(
        e,
        Effect::Timing(timed::Effect::DnfDeadline {
            participant: 7,
            deadline: 24.,
            ..
        })
    )));
    h.clocks(clock(111., 15.)).unwrap();
    let out = h.update_transition(4, &mut inert).unwrap();
    assert!(out.iter().any(|e| matches!(
        e,
        Effect::Timing(timed::Effect::Dnf {
            participant: 8,
            triggered: false,
            ..
        })
    )));
    assert_eq!(h.machine.members(2).unwrap(), &BTreeSet::from([7]));
    h.apply(
        &[Action::Move {
            source: 2,
            participant: 7,
            target: None,
        }],
        &mut inert,
    )
    .unwrap();
    h.clocks(clock(120., 30.)).unwrap();
    assert!(h.update_transition(4, &mut inert).unwrap().is_empty());
    assert!(h.clocks(clock(119., 31.)).is_err());
    // An owned game clock may restart; receipt time cannot go backwards.
    h.clocks(clock(121., 0.)).unwrap();
}

#[test]
fn triggered_dnf_is_participant_scoped_retry_safe_and_callback_atomic() {
    let mut h = Host::new(plan(timed_definition())).unwrap();
    h.clocks(clock(1., 1.)).unwrap();
    h.apply(&[enter(2, 7), enter(2, 8)], &mut inert).unwrap();
    assert!(
        h.dnf_event(5, 0x44b358f4, None, &mut inert)
            .unwrap()
            .is_empty()
    );
    assert!(h.dnf_event(5, 17, Some(7), &mut inert).unwrap().is_empty());
    assert_eq!(
        h.dnf_event(5, 0x44b358f4, Some(7), &mut |_, e| if matches!(
            e,
            Effect::Timing(timed::Effect::Dnf { .. })
        ) {
            Err(Error::Bound)
        } else {
            Ok(vec![])
        }),
        Err(Error::Bound)
    );
    assert_eq!(h.machine.members(2).unwrap(), &BTreeSet::from([7, 8]));
    let out = h.dnf_event(5, 0x44b358f4, Some(7), &mut inert).unwrap();
    assert_eq!(
        out.iter()
            .filter(|e| matches!(
                e,
                Effect::Timing(timed::Effect::Dnf {
                    triggered: true,
                    ..
                })
            ))
            .count(),
        1
    );
    assert!(
        h.dnf_event(5, 0x44b358f4, Some(7), &mut inert)
            .unwrap()
            .is_empty()
    );
    assert_eq!(h.machine.members(2).unwrap(), &BTreeSet::from([8]));
}

#[test]
fn finish_arbitration_waits_on_receipt_time_then_moves_before_publishing_remaining() {
    let mut h = Host::new(plan(timed_definition())).unwrap();
    h.clocks(clock(10., 1.)).unwrap();
    h.apply(&[enter(2, 7), enter(2, 8), enter(2, 9)], &mut inert)
        .unwrap();
    h.waypoint(2, 7, 2, 8., &mut inert).unwrap();
    assert!(h.update_transition(6, &mut inert).unwrap().is_empty());
    h.waypoint(2, 7, 3, 8., &mut inert).unwrap();
    h.waypoint(2, 8, 3, 9., &mut inert).unwrap();
    h.clocks(clock(10.49, 500.)).unwrap();
    assert!(h.update_transition(6, &mut inert).unwrap().is_empty());
    h.clocks(clock(10.5, 500.)).unwrap();
    let out = h
        .update_transition(6, &mut |machine, e| {
            if let Effect::Timing(timed::Effect::Finish {
                participant,
                source,
                ..
            }) = e
            {
                assert!(!machine.members(*source)?.contains(participant));
            }
            Ok(vec![])
        })
        .unwrap();
    let finishes = out
        .iter()
        .filter_map(|e| {
            if let Effect::Timing(timed::Effect::Finish {
                participant,
                seconds,
                position,
                remaining,
                ..
            }) = e
            {
                Some((*participant, *seconds, *position, remaining.clone()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(finishes, vec![(8, 9., 1, vec![7, 9]), (7, 9., 2, vec![9])]);
    assert!(h.update_transition(6, &mut inert).unwrap().is_empty());
}

#[test]
fn finish_queue_survives_source_departure_and_failed_publication_without_double_commit() {
    let mut h = Host::new(plan(timed_definition())).unwrap();
    h.clocks(clock(1., 1.)).unwrap();
    h.apply(&[enter(2, 7)], &mut inert).unwrap();
    h.waypoint(2, 7, 3, 5., &mut inert).unwrap();
    h.apply(
        &[Action::Move {
            source: 2,
            participant: 7,
            target: None,
        }],
        &mut inert,
    )
    .unwrap();
    h.clocks(clock(2., 2.)).unwrap();
    assert_eq!(
        h.update_transition(6, &mut |_, e| if matches!(
            e,
            Effect::Timing(timed::Effect::Finish { .. })
        ) {
            Err(Error::Bound)
        } else {
            Ok(vec![])
        }),
        Err(Error::Bound)
    );
    let out = h.update_transition(6, &mut inert).unwrap();
    assert!(out.iter().any(|e|matches!(e,Effect::Timing(timed::Effect::Finish {position:1,remaining,..}) if remaining.is_empty())));
    assert!(h.machine.members(3).unwrap().is_empty());
}

#[test]
fn countdown_strict_boundary_shared_deadline_and_stop_complete_event_order() {
    let mut d = definition();
    d.states[0].kind = StateKind::Countdown;
    d.states[0].configuration = StateConfiguration::Countdown {
        sequence: ("fixture-sequence".into(), 0),
        seconds: 0.,
    };
    d.bindings.clocks.insert(1, property());
    let mut h = Host::new(plan(d)).unwrap();
    h.clocks(clock(1., 4.)).unwrap();
    let out = h.apply(&[enter(1, 7), enter(1, 8)], &mut inert).unwrap();
    assert_eq!(
        out.iter()
            .filter(|e| matches!(
                e,
                Effect::Timing(timed::Effect::CountdownDeadline { deadline: 4., .. })
            ))
            .count(),
        1
    );
    h.clocks(clock(20., 4.)).unwrap();
    assert!(h.update_countdown(1, &mut inert).unwrap().is_empty());
    h.clocks(clock(21., 4.001)).unwrap();
    let out = h.update_countdown(1, &mut inert).unwrap();
    assert!(matches!(
        out.first(),
        Some(Effect::Timing(timed::Effect::CountdownStop { state: 1 }))
    ));
    assert!(matches!(
        out.last(),
        Some(Effect::Timing(timed::Effect::CountdownEvent {
            state: 1,
            event: 0x81fb265b
        }))
    ));
    assert_eq!(h.machine.members(2).unwrap(), &BTreeSet::from([7, 8]));
    assert!(h.update_countdown(1, &mut inert).unwrap().is_empty());
}

#[test]
fn imported_scene_state_aliases_share_membership_but_keep_scene_owned_hooks() {
    let mut d = definition();
    d.bindings.order = vec![Observer::Transition(4)];
    let source = Endpoint {
        serializer: 99,
        port: 17,
    };
    let target = Endpoint {
        serializer: 99,
        port: 18,
    };
    let imported = data::ImportedState {
        class: "ParticipantStateEntityData".into(),
        identity: identity(900),
        scene_content_key: 100,
        state: state(900, StateKind::Participant),
    };
    d.imports.insert(source, vec![imported.clone()]);
    d.imports.insert(target, vec![imported]);
    d.transitions[0].sources = vec![source];
    d.transitions[0].target = Some(target);
    let plan = Arc::new(Plan::installed(&d).unwrap());
    let alias = plan.data.transitions[0].sources[0].serializer as StateId;
    assert_eq!(
        plan.data.transitions[0].target.unwrap().serializer,
        usize::from(alias)
    );
    assert_eq!(plan.external_state(alias).unwrap().state.serializer, 900);
    let mut h = Host::new(plan).unwrap();
    h.apply(&[enter(alias, 7)], &mut inert).unwrap();
    assert!(h.machine().members(alias).unwrap().contains(&7));
    assert!(h.machine().completed(alias).unwrap().is_empty());
}

#[test]
fn imported_channel_lists_expand_in_order_and_single_targets_reject_multiple_states() {
    let mut d = definition();
    let channel = Endpoint {
        serializer: 99,
        port: 17,
    };
    let imported = |n| data::ImportedState {
        class: "ParticipantStateEntityData".into(),
        identity: identity(n),
        scene_content_key: 100,
        state: state(n, StateKind::Participant),
    };
    d.imports
        .insert(channel, vec![imported(901), imported(900), imported(901)]);
    d.bindings.reporters.push(StateReporter {
        identity: identity(70),
        compound: true,
        states: vec![channel],
    });
    d.bindings.order = vec![Observer::Reporter(0), Observer::Transition(4)];
    let p = Plan::installed(&d).unwrap();
    let ids = p.data.bindings.reporters[0]
        .states
        .iter()
        .map(|s| s.serializer as StateId)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    assert_eq!(p.external_state(ids[0]).unwrap().state.serializer, 901);
    assert_eq!(p.external_state(ids[1]).unwrap().state.serializer, 900);
    d.transitions[0].target = Some(channel);
    assert!(matches!(Plan::installed(&d), Err(Error::Invalid)));
}

#[test]
fn boolean_predicates_recompute_per_participant_and_triggered_values_keep_first_false() {
    let mut d = watched();
    d.bindings.predicates = vec![
        Predicate {
            identity: identity(40),
            kind: PredicateKind::Triggered,
        },
        Predicate {
            identity: identity(41),
            kind: PredicateKind::Boolean {
                operation: BooleanOperation::And,
                operands: vec![link(10), link(40)],
            },
        },
    ];
    d.bindings.spawners[0].controller = Some(Controller::Boolean(link(41)));
    d.bindings.spawners[1].controller = Some(Controller::Boolean(link(41)));
    d.bindings
        .order
        .extend([Observer::Predicate(0), Observer::Predicate(1)]);
    let mut h = Host::new(plan(d)).unwrap();
    h.apply(&[enter(1, 7), enter(1, 8)], &mut inert).unwrap();
    assert!(h.logic_members(0).unwrap().is_empty());
    let output = h
        .predicate_event(&identity(40), 0x27d4_023a, Some(7), &mut inert)
        .unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::Boolean { change, .. } if !change.value))
            .count(),
        1
    );
    assert!(
        h.predicate_event(&identity(40), 0x27d4_023a, Some(7), &mut inert)
            .unwrap()
            .is_empty()
    );
    assert!(
        h.predicate_event(&identity(40), 0xa425_3151, None, &mut inert)
            .unwrap()
            .is_empty()
    );
    assert!(
        h.predicate_event(&identity(40), 17, Some(7), &mut inert)
            .unwrap()
            .is_empty()
    );
    h.predicate_event(&identity(40), 0xa425_3151, Some(7), &mut inert)
        .unwrap();
    assert_eq!(h.logic_members(0).unwrap(), &BTreeSet::from([7]));
    h.apply(
        &[Action::Move {
            source: 2,
            participant: 7,
            target: None,
        }],
        &mut inert,
    )
    .unwrap();
    assert!(h.logic_members(0).unwrap().is_empty());
}

#[test]
fn updateable_states_wait_for_their_authored_update_set_and_skip_completed_members() {
    let mut d = definition();
    d.states[0].kind = StateKind::Updateable;
    d.states[0].configuration = StateConfiguration::Updateable {
        update_set: identity(50),
    };
    d.transitions[0].kind = TransitionKind::Triggered;
    let mut h = Host::new(plan(d)).unwrap();
    h.apply(&[enter(1, 7), enter(1, 8)], &mut inert).unwrap();
    assert!(h.machine().completed(1).unwrap().is_empty());
    assert_eq!(h.update_set(&identity(51), &mut inert), Err(Error::Invalid));
    let output = h.update_set(&identity(50), &mut inert).unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|e| matches!(e, Effect::State(Notification::Completed { state: 1, .. })))
            .count(),
        2
    );
    assert!(h.update_set(&identity(50), &mut inert).unwrap().is_empty());
}

#[test]
fn set_counter_seeds_and_recomputes_actual_membership_in_observer_order() {
    let mut d = definition();
    d.transitions.clear();
    let source = SetSource::Local(identity(80));
    d.bindings.sets = vec![ParticipantSet {
        identity: identity(80),
        kind: SetKind::Owned,
    }];
    d.bindings.set_counters = vec![
        SetCounter {
            identity: identity(81),
            source: Some(source.clone()),
        },
        SetCounter {
            identity: identity(82),
            source: None,
        },
    ];
    d.bindings.order = vec![
        Observer::Set(0),
        Observer::SetCounter(0),
        Observer::SetCounter(1),
    ];
    let (mut h, effects) = Host::from_scene(plan(d), SceneInputs::default(), &mut inert).unwrap();
    let counts = |effects: Vec<Effect>| {
        effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Reporter(reporters::Effect::Count { identity, value }) => {
                    Some((identity.index, value))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(counts(effects), [(81, 0), (82, 0)]);
    let write = |participant, member| commands::Command::Set {
        source: source.clone(),
        participant,
        member,
    };
    assert_eq!(
        counts(
            h.dispatch(
                vec![write(7, true), write(7, true), write(8, true)],
                &mut |_, _| Ok(vec![])
            )
            .unwrap()
        ),
        [(81, 1), (81, 2)]
    );
    let before = h.set_members(&identity(80)).unwrap().clone();
    assert!(
        h.dispatch(vec![write(7, false)], &mut |_, e| match e {
            Effect::Reporter(_) => Err(Error::Invalid),
            _ => Ok(vec![]),
        })
        .is_err()
    );
    assert_eq!(h.set_members(&identity(80)).unwrap(), &before);
    assert_eq!(
        counts(
            h.dispatch(vec![write(7, false), write(8, false)], &mut |_, _| Ok(
                vec![]
            ))
            .unwrap()
        ),
        [(81, 1), (81, 0)]
    );
}
