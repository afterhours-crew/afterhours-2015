// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::events::{self, Connection, Delivery, Dispatch, Endpoint, Handler};
use super::*;

fn range_inputs(min: i32, max: i32, value: i32) -> impl FnMut(u32) -> Option<Value> {
    move |port| {
        Some(Value::Integer(match port {
            RANGE_MIN => min,
            RANGE_MAX => max,
            FLOAT_VALUE => value,
            _ => panic!("unexpected range input"),
        }))
    }
}

#[test]
fn integer_range_defers_reads_and_uses_inclusive_signed_limits() {
    let mut node = Node::new(Definition::CompareIntegerRange {
        on_change: true,
        on_start: false,
    })
    .unwrap();
    assert!(
        node.initial_outputs(|_| panic!("no constructor read"))
            .unwrap()
            .is_empty()
    );
    assert!(
        node.apply(Action::PropertyChanged(RANGE_MIN), |_| panic!("not active"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        node.apply(Action::Activate, range_inputs(-4, -2, -4))
            .unwrap(),
        [Effect::Emit(RANGE_WITHIN)]
    );
    for (value, expected) in [
        (-2, vec![]),
        (-1, vec![Effect::Emit(RANGE_ABOVE)]),
        (i32::MAX, vec![]),
        (i32::MIN, vec![Effect::Emit(RANGE_BELOW)]),
        (-4, vec![Effect::Emit(RANGE_WITHIN)]),
    ] {
        assert_eq!(
            node.apply(
                Action::PropertyChanged(FLOAT_VALUE),
                range_inputs(-4, -2, value)
            )
            .unwrap(),
            expected
        );
    }
    let before = node.clone();
    assert_eq!(
        node.apply(Action::PropertyChanged(RANGE_MAX), |_| None),
        Err(Error::MissingInput)
    );
    assert_eq!(node, before);
    assert_eq!(
        node.apply(Action::PropertyChanged(RANGE_MAX), |_| Some(Value::Float(
            0.0
        ))),
        Err(Error::InvalidGraph)
    );
    assert_eq!(node, before);
}

#[test]
fn integer_range_explicit_events_repeat_without_changing_automatic_history() {
    let mut node = Node::new(Definition::CompareIntegerRange {
        on_change: true,
        on_start: true,
    })
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            node.apply(Action::Event(0), range_inputs(3, 4, 3)).unwrap(),
            [Effect::Emit(RANGE_WITHIN)]
        );
    }
    assert_eq!(
        node.apply(Action::Activate, range_inputs(3, 4, 3)).unwrap(),
        [Effect::Emit(RANGE_WITHIN)]
    );
    assert_eq!(
        node.apply(Action::Event(IN), range_inputs(3, 4, 5))
            .unwrap(),
        [Effect::Emit(RANGE_ABOVE)]
    );
    assert!(
        node.apply(Action::PropertyChanged(FLOAT_VALUE), range_inputs(3, 4, 4))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        node.apply(Action::PropertyChanged(FLOAT_VALUE), range_inputs(3, 4, 5))
            .unwrap(),
        [Effect::Emit(RANGE_ABOVE)]
    );
    let mut disabled = Node::new(Definition::CompareIntegerRange {
        on_change: false,
        on_start: false,
    })
    .unwrap();
    for action in [Action::PropertyChanged(FLOAT_VALUE), Action::Activate] {
        assert!(
            disabled
                .apply(action, |_| panic!("disabled read"))
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        disabled
            .apply(Action::Event(IN), range_inputs(3, 4, 0))
            .unwrap(),
        [Effect::Emit(RANGE_BELOW)]
    );
}

#[test]
fn integer_range_reversed_limits_keep_native_order_and_last_category() {
    let mut node = Node::new(Definition::CompareIntegerRange {
        on_change: true,
        on_start: true,
    })
    .unwrap();
    let both = [Effect::Emit(RANGE_ABOVE), Effect::Emit(RANGE_BELOW)];
    assert_eq!(
        node.apply(Action::Activate, range_inputs(10, 0, 5))
            .unwrap(),
        both
    );
    // LastResult is Below, so Above is emitted again even for equal inputs.
    assert_eq!(
        node.apply(Action::PropertyChanged(RANGE_MIN), range_inputs(10, 0, 5))
            .unwrap(),
        [Effect::Emit(RANGE_ABOVE)]
    );
    assert_eq!(
        node.apply(Action::Event(IN), range_inputs(10, 0, 5))
            .unwrap(),
        both
    );
    let mut fresh = Node::new(Definition::CompareIntegerRange {
        on_change: true,
        on_start: true,
    })
    .unwrap();
    let before = fresh.clone();
    assert_eq!(
        fresh.apply(Action::Activate, |_| None),
        Err(Error::MissingInput)
    );
    assert_eq!(fresh, before);
    assert_eq!(
        fresh
            .apply(Action::Activate, range_inputs(0, 0, 0))
            .unwrap(),
        [Effect::Emit(RANGE_WITHIN)]
    );
}

#[test]
fn not_and_clamp_follow_native_change_and_boundary_rules() {
    let mut not = Node::new(Definition::Not).unwrap();
    assert!(
        not.initial_outputs(|_| panic!("no bind-time read"))
            .unwrap()
            .is_empty()
    );
    assert!(
        not.apply(Action::Activate, |_| panic!("no activation read"))
            .unwrap()
            .is_empty()
    );
    for value in [true, false, false] {
        assert_eq!(
            not.apply(Action::PropertyChanged(IN), |_| Some(Value::Boolean(value)))
                .unwrap(),
            [Effect::Publish(OUT, Value::Boolean(!value))]
        );
    }
    for (min, max, input, expected) in [
        (1.0, 5.0, 3.0, 3.0),
        (1.0, 5.0, -2.0, 1.0),
        (1.0, 5.0, 8.0, 5.0),
        (5.0, 1.0, 3.0, 5.0),
        (-0.0, 0.0, 0.0, -0.0),
    ] {
        let mut clamp = Node::new(Definition::Clamp { min, max }).unwrap();
        assert_eq!(
            clamp
                .initial_outputs(|_| panic!("constructor seeds zero"))
                .unwrap(),
            [Effect::Publish(OUT, Value::Float(0.0))]
        );
        let effects = clamp
            .apply(Action::PropertyChanged(IN), |_| Some(Value::Float(input)))
            .unwrap();
        let [Effect::Publish(OUT, Value::Float(value))] = effects.as_slice() else {
            panic!("{effects:?}")
        };
        assert_eq!(value.to_bits(), (expected as f32).to_bits());
        assert!(
            clamp
                .apply(Action::PropertyChanged(IN), |_| Some(Value::Float(
                    f32::NAN
                )))
                .is_err()
        );
    }
}

#[test]
fn boolean_cache_seeds_only_when_requested_and_loads_before_its_event() {
    for load_on_create in [false, true] {
        let mut node = Node::new(Definition::BooleanCache { load_on_create }).unwrap();
        let mut reads = 0;
        let initial = node
            .initial_outputs(|port| {
                assert_eq!(port, CACHE_INPUT);
                reads += 1;
                Some(Value::Boolean(false))
            })
            .unwrap();
        assert_eq!(reads, usize::from(load_on_create));
        assert_eq!(
            initial,
            if load_on_create {
                vec![Effect::Publish(CACHE_OUTPUT, Value::Boolean(false))]
            } else {
                vec![]
            }
        );
        for value in [true, true, false, false] {
            assert_eq!(
                node.apply(Action::PropertyChanged(CACHE_INPUT), |_| panic!(
                    "input change must not load"
                ))
                .unwrap(),
                []
            );
            assert_eq!(
                node.apply(Action::Activate, |_| panic!("activation must not load"))
                    .unwrap(),
                []
            );
            assert_eq!(
                node.apply(Action::Event(LOAD), |port| {
                    assert_eq!(port, CACHE_INPUT);
                    Some(Value::Boolean(value))
                })
                .unwrap(),
                [
                    Effect::Publish(CACHE_OUTPUT, Value::Boolean(value)),
                    Effect::Emit(CACHE_LOADED)
                ]
            );
        }
        let before = node.clone();
        assert_eq!(
            node.apply(Action::Event(LOAD), |_| None),
            Err(Error::MissingInput)
        );
        assert_eq!(
            node.apply(Action::Event(LOAD), |_| Some(Value::Integer(1))),
            Err(Error::InvalidGraph)
        );
        assert_eq!(node, before);
    }
}
use std::collections::BTreeMap;

fn emits(events: &[u32]) -> Vec<Effect> {
    events.iter().copied().map(Effect::Emit).collect()
}

#[test]
fn uint64_comparison_preserves_high_bits_and_transition_history() {
    let mut node = compare(Comparison::Unsigned64, true, true);
    let values = |a, b| move |p| Some(Value::Unsigned64(if p == A { a } else { b }));
    let high = 1u64 << 63;
    assert_eq!(
        node.apply(Action::Activate, values(high, 7)).unwrap(),
        emits(&[GREATER, GREATER_EQUAL, NOT_EQUAL])
    );
    assert!(
        node.apply(Action::PropertyChanged(A), values(u64::MAX, 7))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        node.apply(Action::PropertyChanged(B), values(u64::MAX, u64::MAX))
            .unwrap(),
        emits(&[EQUAL, LESS_EQUAL])
    );
    for _ in 0..2 {
        assert_eq!(
            node.apply(Action::Event(IN), values(high + 1, high))
                .unwrap(),
            emits(&[GREATER, GREATER_EQUAL, NOT_EQUAL])
        );
    }
    // Explicit evaluation did not replace the equal property state.
    assert_eq!(
        node.apply(Action::PropertyChanged(A), values(high, high + 1))
            .unwrap(),
        emits(&[NOT_EQUAL, LESS])
    );
    let before = node.clone();
    assert_eq!(
        node.apply(Action::PropertyChanged(A), |_| Some(Value::Integer(-1))),
        Err(Error::InvalidGraph)
    );
    assert_eq!(node, before);
}
fn pair(kind: Comparison, value: i32) -> impl FnMut(u32) -> Option<Value> {
    move |port| {
        let n = if port == A { value } else { 0 };
        Some(match kind {
            Comparison::Float => Value::Float(n as f32),
            Comparison::Integer => Value::Integer(n),
            Comparison::Unsigned64 => Value::Unsigned64(n as u64),
        })
    }
}
fn compare(kind: Comparison, on_change: bool, on_start: bool) -> Node {
    Node::new(Definition::Compare {
        kind,
        on_change,
        on_start,
    })
    .unwrap()
}

#[test]
fn comparison_property_transitions_emit_only_new_predicates_in_native_order() {
    // Columns/rows are less, equal, greater. Not-equal stays true across a
    // direct less->greater transition; it must not emit again there.
    let float: [[&[u32]; 3]; 3] = [
        [&[], &[GREATER_EQUAL], &[GREATER, GREATER_EQUAL]],
        [&[LESS], &[], &[GREATER]],
        [&[LESS_EQUAL, LESS], &[LESS_EQUAL], &[]],
    ];
    let integer: [[&[u32]; 3]; 3] = [
        [&[], &[GREATER_EQUAL, EQUAL], &[GREATER, GREATER_EQUAL]],
        [&[NOT_EQUAL, LESS], &[], &[GREATER, NOT_EQUAL]],
        [&[LESS_EQUAL, LESS], &[EQUAL, LESS_EQUAL], &[]],
    ];
    for (kind, matrix) in [(Comparison::Float, float), (Comparison::Integer, integer)] {
        for (i, old) in [-1, 0, 1].into_iter().enumerate() {
            for (j, new) in [-1, 0, 1].into_iter().enumerate() {
                let mut node = compare(kind, true, false);
                node.apply(Action::Activate, pair(kind, old)).unwrap();
                node.apply(Action::PropertyChanged(A), pair(kind, old))
                    .unwrap();
                assert_eq!(
                    node.apply(Action::PropertyChanged(A), pair(kind, new))
                        .unwrap(),
                    emits(matrix[i][j])
                );
                assert!(
                    node.apply(Action::PropertyChanged(A), pair(kind, new))
                        .unwrap()
                        .is_empty()
                );
            }
        }
    }
}

#[test]
fn explicit_comparison_repeats_without_changing_property_relation() {
    for (kind, expected) in [
        (
            Comparison::Float,
            [
                &[LESS_EQUAL, LESS][..],
                &[GREATER_EQUAL, LESS_EQUAL],
                &[GREATER, GREATER_EQUAL],
            ],
        ),
        (
            Comparison::Integer,
            [
                &[NOT_EQUAL, LESS_EQUAL, LESS][..],
                &[GREATER_EQUAL, EQUAL, LESS_EQUAL],
                &[GREATER, GREATER_EQUAL, NOT_EQUAL],
            ],
        ),
    ] {
        let mut node = compare(kind, true, false);
        node.apply(Action::Activate, |_| None).unwrap();
        for (value, events) in [-1, 0, 1].into_iter().zip(expected) {
            for event in [IN, 0, IN] {
                assert_eq!(
                    node.apply(Action::Event(event), pair(kind, value)).unwrap(),
                    emits(events)
                );
            }
        }
        // Explicit comparisons above never established an old relation.
        assert_eq!(
            node.apply(Action::PropertyChanged(A), pair(kind, 1))
                .unwrap(),
            emits(expected[2])
        );
    }
}

#[test]
fn startup_and_pre_activation_changes_use_latest_inputs_once() {
    for kind in [Comparison::Integer, Comparison::Float] {
        let mut node = compare(kind, true, false);
        assert!(
            node.apply(Action::PropertyChanged(A), pair(kind, -1))
                .unwrap()
                .is_empty()
        );
        let expected = if kind == Comparison::Integer {
            emits(&[GREATER_EQUAL, EQUAL, LESS_EQUAL])
        } else {
            emits(&[GREATER_EQUAL, LESS_EQUAL])
        };
        assert_eq!(
            node.apply(Action::Activate, pair(kind, 0)).unwrap(),
            expected
        );
        assert!(node.apply(Action::Activate, |_| None).unwrap().is_empty());
        let mut start = compare(kind, false, true);
        assert!(
            start
                .apply(Action::PropertyChanged(A), |_| None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            start.apply(Action::Activate, pair(kind, 0)).unwrap(),
            expected
        );
        assert!(
            start
                .apply(Action::PropertyChanged(A), pair(kind, 1))
                .unwrap()
                .is_empty()
        );
        let mut silent = compare(kind, false, false);
        assert!(silent.apply(Action::Activate, |_| None).unwrap().is_empty());
    }
}

#[test]
fn cache_and_accumulator_publish_before_notification_even_when_unchanged() {
    let mut cache = Node::new(Definition::Cache).unwrap();
    assert_eq!(
        cache.initial_outputs(|_| Some(Value::Float(8.0))).unwrap(),
        [Effect::Publish(CACHE_OUTPUT, Value::Float(8.0))]
    );
    assert!(
        cache
            .apply(Action::PropertyChanged(A), |_| None)
            .unwrap()
            .is_empty()
    );
    for _ in 0..2 {
        assert_eq!(
            cache
                .apply(Action::Event(LOAD), |_| Some(Value::Float(3.0)))
                .unwrap(),
            [
                Effect::Publish(CACHE_OUTPUT, Value::Float(3.0)),
                Effect::Emit(CACHE_LOADED)
            ]
        );
    }
    let mut acc = Node::new(Definition::Accumulator).unwrap();
    for (event, input, expected) in [
        (ACCUMULATE, 1.25, 1.25),
        (ACCUMULATE, -0.5, 0.75),
        (RESET, f32::NAN, 0.0),
        (RESET, f32::NAN, 0.0),
    ] {
        assert_eq!(
            acc.apply(Action::Event(event), |_| Some(Value::Float(input)))
                .unwrap(),
            [
                Effect::Publish(OUTPUT, Value::Float(expected)),
                Effect::Emit(OUTPUT_CHANGED)
            ]
        );
    }
    assert!(acc.apply(Action::Event(LOAD), |_| None).unwrap().is_empty());
}

#[test]
fn accumulator_output_tracks_input_between_events_without_notification() {
    // E914: the official category score follows its gated input every frame
    // (sum 0 + input), folds on Accumulate, then holds the sum.
    let mut acc = Node::new(Definition::Accumulator).unwrap();
    assert_eq!(
        acc.apply(Action::PropertyChanged(INPUT), |_| Some(Value::Float(
            26.42
        )))
        .unwrap(),
        [Effect::Publish(OUTPUT, Value::Float(26.42))]
    );
    assert_eq!(
        acc.apply(Action::Event(ACCUMULATE), |_| Some(Value::Float(60.65)))
            .unwrap(),
        [
            Effect::Publish(OUTPUT, Value::Float(60.65)),
            Effect::Emit(OUTPUT_CHANGED)
        ]
    );
    // A later input change adds to the stored sum.
    assert_eq!(
        acc.apply(Action::PropertyChanged(INPUT), |_| Some(Value::Float(1.5)))
            .unwrap(),
        [Effect::Publish(OUTPUT, Value::Float(62.15))]
    );
    // Other property changes are ignored; invalid input leaves the node alone.
    assert!(
        acc.apply(Action::PropertyChanged(A), |_| Some(Value::Float(1.0)))
            .unwrap()
            .is_empty()
    );
    let original = acc.clone();
    assert!(
        acc.apply(Action::PropertyChanged(INPUT), |_| Some(Value::Float(
            f32::NAN
        )))
        .is_err()
    );
    assert_eq!(acc, original);
}

#[test]
fn float_reads_current_output_and_reset_uses_authored_default() {
    let mut node = Node::new(Definition::Float {
        default: -2.0,
        step: 0.25,
    })
    .unwrap();
    assert_eq!(
        node.initial_outputs(|_| None).unwrap(),
        [Effect::Publish(FLOAT_VALUE, Value::Float(-2.0))]
    );
    for (event, expected) in [(INCREMENT, 9.25), (DECREMENT, 8.75), (RESET, -2.0)] {
        assert_eq!(
            node.apply(Action::Event(event), |_| Some(Value::Float(9.0)))
                .unwrap(),
            [Effect::Publish(FLOAT_VALUE, Value::Float(expected))]
        );
    }
}

#[test]
fn gates_and_run_once_splitters_update_before_synchronous_reentry() {
    let mut gate = Node::new(Definition::Gate { open: false }).unwrap();
    assert!(gate.apply(Action::Event(IN), |_| None).unwrap().is_empty());
    gate.apply(Action::Event(OPEN), |_| None).unwrap();
    assert_eq!(
        gate.apply(Action::Event(IN), |_| None).unwrap(),
        emits(&[OUT])
    );
    gate.apply(Action::Event(CLOSE), |_| None).unwrap();
    assert!(gate.apply(Action::Event(IN), |_| None).unwrap().is_empty());
    for once in [false, true] {
        let mut split = Node::new(Definition::Splitter { run_once: once }).unwrap();
        assert_eq!(
            split.apply(Action::Event(999), |_| None).unwrap(),
            emits(&[SPLITTER_OUTPUT])
        );
        assert_eq!(
            split.apply(Action::Event(IN), |_| None).unwrap(),
            if once {
                vec![]
            } else {
                emits(&[SPLITTER_OUTPUT])
            }
        );
    }
}

#[test]
fn invalid_input_and_overflow_do_not_partially_mutate_nodes() {
    let mut acc = Node::new(Definition::Accumulator).unwrap();
    acc.apply(Action::Event(ACCUMULATE), |_| Some(Value::Float(f32::MAX)))
        .unwrap();
    let original = acc.clone();
    for value in [
        Value::Float(f32::MAX),
        Value::Float(f32::NAN),
        Value::Integer(4),
    ] {
        assert!(
            acc.apply(Action::Event(ACCUMULATE), |_| Some(value.clone()))
                .is_err()
        );
        assert_eq!(acc, original);
    }
    let mut node = compare(Comparison::Float, true, true);
    let original = node.clone();
    for value in [
        None,
        Some(Value::Float(f32::INFINITY)),
        Some(Value::Boolean(false)),
    ] {
        assert!(node.apply(Action::Activate, |_| value.clone()).is_err());
        assert_eq!(node, original);
    }
    assert!(
        Node::new(Definition::Float {
            default: 0.0,
            step: f32::NAN
        })
        .is_err()
    );
}

#[test]
fn boolean_comparison_always_send_and_explicit_events_preserve_relation() {
    for always_send in [false, true] {
        let mut node = Node::new(Definition::CompareBoolean {
            on_change: true,
            on_start: false,
            always_send,
        })
        .unwrap();
        // Pre-activation changes are deferred, even before a value is ready.
        assert!(
            node.apply(Action::PropertyChanged(BOOL), |_| None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            node.apply(Action::Activate, |_| Some(Value::Boolean(true)))
                .unwrap(),
            emits(&[ON_TRUE])
        );
        assert_eq!(
            node.apply(Action::PropertyChanged(BOOL), |_| Some(Value::Boolean(
                true
            )))
            .unwrap(),
            if always_send {
                emits(&[ON_TRUE])
            } else {
                vec![]
            }
        );
        assert_eq!(
            node.apply(Action::Event(IN), |_| Some(Value::Boolean(false)))
                .unwrap(),
            emits(&[ON_FALSE])
        );
        assert_eq!(
            node.apply(Action::PropertyChanged(BOOL), |_| Some(Value::Boolean(
                false
            )))
            .unwrap(),
            emits(&[ON_FALSE])
        );
    }
    let mut start = Node::new(Definition::CompareBoolean {
        on_change: false,
        on_start: true,
        always_send: true,
    })
    .unwrap();
    assert!(
        start
            .apply(Action::PropertyChanged(BOOL), |_| None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        start
            .apply(Action::Activate, |_| Some(Value::Boolean(false)))
            .unwrap(),
        emits(&[ON_FALSE])
    );
    assert!(start.apply(Action::Activate, |_| None).unwrap().is_empty());
}

#[test]
fn boolean_and_integer_values_publish_without_explicit_notifications() {
    let mut b = Node::new(Definition::Boolean { default: true }).unwrap();
    assert_eq!(
        b.initial_outputs(|_| None).unwrap(),
        [Effect::Publish(FLOAT_VALUE, Value::Boolean(true))]
    );
    for (event, current, output) in [
        (SET_FALSE, true, false),
        (SET_TRUE, false, true),
        (TOGGLE, true, false),
        (TOGGLE, false, true),
    ] {
        assert_eq!(
            b.apply(Action::Event(event), |_| Some(Value::Boolean(current)))
                .unwrap(),
            [Effect::Publish(FLOAT_VALUE, Value::Boolean(output))]
        );
    }
    let mut i = Node::new(Definition::Integer {
        default: -3,
        step: 1,
    })
    .unwrap();
    assert_eq!(
        i.initial_outputs(|_| None).unwrap(),
        [Effect::Publish(FLOAT_VALUE, Value::Integer(-3))]
    );
    for (event, current, output) in [
        (INCREMENT, i32::MAX, i32::MIN),
        (DECREMENT, i32::MIN, i32::MAX),
        (RESET, 7, -3),
    ] {
        assert_eq!(
            i.apply(Action::Event(event), |_| Some(Value::Integer(current)))
                .unwrap(),
            [Effect::Publish(FLOAT_VALUE, Value::Integer(output))]
        );
    }
    let mut cache = Node::new(Definition::IntegerCache {
        load_on_create: true,
    })
    .unwrap();
    assert_eq!(
        cache.initial_outputs(|_| Some(Value::Integer(7))).unwrap(),
        [Effect::Publish(CACHE_OUTPUT, Value::Integer(7))]
    );
    for _ in 0..2 {
        assert_eq!(
            cache
                .apply(Action::Event(LOAD), |_| Some(Value::Integer(i32::MIN)))
                .unwrap(),
            [
                Effect::Publish(CACHE_OUTPUT, Value::Integer(i32::MIN)),
                Effect::Emit(CACHE_LOADED)
            ]
        );
    }
}

#[test]
fn conditionals_read_and_publish_only_the_selected_branch() {
    for (definition, zero, value) in [
        (
            Definition::ConditionalFloat,
            Value::Float(0.0),
            Value::Float(8.25),
        ),
        (
            Definition::ConditionalInteger,
            Value::Integer(0),
            Value::Integer(17),
        ),
    ] {
        let mut node = Node::new(definition).unwrap();
        assert_eq!(
            node.initial_outputs(|_| None).unwrap(),
            [Effect::Publish(OUTPUT, zero)]
        );
        for condition in [true, false] {
            let chosen = if condition {
                VALUE_IF_TRUE
            } else {
                VALUE_IF_FALSE
            };
            let other = if condition {
                VALUE_IF_FALSE
            } else {
                VALUE_IF_TRUE
            };
            let read = |port| {
                if port == CONDITION {
                    Some(Value::Boolean(condition))
                } else if port == chosen {
                    Some(value.clone())
                } else {
                    panic!("read inactive/unknown branch")
                }
            };
            for action in [
                Action::Activate,
                Action::PropertyChanged(CONDITION),
                Action::PropertyChanged(chosen),
            ] {
                assert_eq!(
                    node.apply(action, read).unwrap(),
                    [Effect::Publish(OUTPUT, value.clone())]
                );
            }
            assert!(
                node.apply(Action::PropertyChanged(other), read)
                    .unwrap()
                    .is_empty()
            );
            assert!(node.apply(Action::Event(IN), |_| None).unwrap().is_empty());
        }
        assert!(
            node.apply(Action::Activate, |p| Some(if p == CONDITION {
                Value::Boolean(true)
            } else {
                Value::Boolean(false)
            }))
            .is_err()
        );
    }
}

#[test]
fn integer_cache_without_initial_load_stays_unready_until_load_event() {
    let mut cache = Node::new(Definition::IntegerCache {
        load_on_create: false,
    })
    .unwrap();
    assert!(
        cache
            .initial_outputs(|_| panic!("no initial input read"))
            .unwrap()
            .is_empty()
    );
    assert!(
        cache
            .apply(Action::Activate, |_| panic!())
            .unwrap()
            .is_empty()
    );
    for value in [7, 7, -5] {
        assert_eq!(
            cache
                .apply(Action::Event(LOAD), |_| Some(Value::Integer(value)))
                .unwrap(),
            [
                Effect::Publish(CACHE_OUTPUT, Value::Integer(value)),
                Effect::Emit(CACHE_LOADED)
            ]
        );
    }
}

#[test]
fn event_and_gate_waits_for_distinct_inputs_and_requires_explicit_reset() {
    for count in [1, 2, 15, 16] {
        let mut n = Node::new(Definition::AllEvents { count }).unwrap();
        assert!(n.apply(Action::Activate, |_| None).unwrap().is_empty());
        assert!(
            n.apply(Action::Event(super::super::integers::port("In", 0)), |_| {
                None
            })
            .unwrap()
            .is_empty()
        );
        for round in 0..2 {
            for input in (1..=u16::from(count)).rev() {
                let event = Action::Event(super::super::integers::port("In", input));
                assert_eq!(
                    n.apply(event, |_| None).unwrap(),
                    if input == 1 {
                        vec![Effect::Emit(OUT)]
                    } else {
                        vec![]
                    },
                    "round {round}"
                );
                assert!(n.apply(event, |_| None).unwrap().is_empty());
            }
            assert!(n.apply(Action::Event(RESET), |_| None).unwrap().is_empty());
        }
    }
    assert!(Node::new(Definition::AllEvents { count: 0 }).is_err());
    assert!(Node::new(Definition::AllEvents { count: 17 }).is_err());
}

#[test]
fn boolean_folds_distinguish_unavailable_bound_inputs_and_short_circuit() {
    for (inputs, all, any) in [
        (vec![], Some(true), Some(false)),
        (vec![None], None, Some(false)),
        (vec![Some(false), None], Some(false), Some(false)),
        (vec![None, Some(false)], None, Some(false)),
        (vec![Some(true), None], None, Some(true)),
        (vec![None, Some(true)], None, Some(true)),
        (vec![Some(true), Some(true)], Some(true), Some(true)),
    ] {
        assert_eq!(fold_boolean(true, &inputs), Ok(all));
        assert_eq!(fold_boolean(false, &inputs), Ok(any));
    }
    assert_eq!(fold_boolean(true, &[Some(true); 257]), Err(Error::Bound));
}

#[test]
fn numeric_cast_preserves_publication_order_signed_truncation_and_zero() {
    for (input, float, integer, boolean) in [
        (Value::Integer(16_777_217), 16_777_216.0, 16_777_217, true),
        (Value::Float(-2.9), -2.9, -2, true),
        (Value::Float(-0.0), -0.0, 0, false),
        (
            Value::Float(-2_147_483_648.0),
            -2_147_483_648.0,
            i32::MIN,
            true,
        ),
        (Value::Boolean(true), 1.0, 1, true),
        (Value::Boolean(false), 0.0, 0, false),
    ] {
        let effects = cast_numeric(input).unwrap();
        assert_eq!(
            effects,
            [
                Effect::Publish(936_870_923, Value::Float(float)),
                Effect::Publish(245_372_264, Value::Integer(integer)),
                Effect::Publish(3_802_618_965, Value::Boolean(boolean))
            ]
        );
        let Effect::Publish(_, Value::Float(actual)) = effects[0] else {
            panic!()
        };
        assert_eq!(actual.to_bits(), float.to_bits());
    }
    for value in [f32::NAN, f32::INFINITY, 2_147_483_648.0, -2_147_483_904.0] {
        assert!(cast_numeric(Value::Float(value)).is_err());
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Bank {
    nodes: Vec<Node>,
    properties: BTreeMap<(u16, u32), Value>,
    banked: Vec<f32>,
}
impl Handler for Bank {
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        // Synthetic sink consumes a cached amount only after all prior effects.
        if target.node == 3 {
            let Some(Value::Float(amount)) = self.properties.get(&(2, CACHE_OUTPUT)) else {
                return Err(Error::MissingInput);
            };
            self.banked.push(*amount);
            return Ok(());
        }
        let props = &self.properties;
        let effects =
            self.nodes[target.node as usize].apply(Action::Event(target.event), |port| {
                if target.node == 2 && port == CACHE_INPUT {
                    props.get(&(1, OUTPUT)).cloned()
                } else {
                    props.get(&(target.node, port)).cloned()
                }
            })?;
        for effect in effects {
            match effect {
                Effect::Publish(port, value) => {
                    self.properties.insert((target.node, port), value);
                }
                Effect::Emit(event) => dispatch.emit(
                    self,
                    Endpoint {
                        node: target.node,
                        event,
                    },
                )?,
            }
        }
        Ok(())
    }
}
fn ep(node: u16, event: u32) -> Endpoint {
    Endpoint { node, event }
}
fn edge(source: Endpoint, target: Endpoint) -> Connection {
    Connection {
        source,
        target,
        delivery: Delivery::Local,
    }
}

#[test]
fn bank_cache_observes_accumulated_value_before_reset_with_session_isolation() {
    let source = ep(0, 71);
    let edges = vec![
        edge(source, ep(0, IN)),
        edge(ep(0, SPLITTER_OUTPUT), ep(2, LOAD)),
        edge(ep(2, CACHE_LOADED), ep(3, IN)),
        edge(ep(0, SPLITTER_OUTPUT), ep(1, RESET)),
    ];
    let graph = events::Graph::new(4, edges.clone()).unwrap();
    for amount in [3.0, 17.0] {
        let mut state = Bank {
            nodes: vec![
                Node::new(Definition::Splitter { run_once: true }).unwrap(),
                Node::new(Definition::Accumulator).unwrap(),
                Node::new(Definition::Cache).unwrap(),
            ],
            properties: BTreeMap::from([
                ((1, INPUT), Value::Float(amount)),
                ((1, OUTPUT), Value::Float(amount)),
            ]),
            banked: vec![],
        };
        state.nodes[1]
            .apply(Action::Event(ACCUMULATE), |_| Some(Value::Float(amount)))
            .unwrap();
        let original = state.clone();
        let (next, output) = graph.apply(&state, source).unwrap();
        assert!(output.is_empty());
        assert_eq!(next.banked, [amount]);
        assert_eq!(next.properties[&(1, OUTPUT)], Value::Float(0.0));
        assert_eq!(next.properties[&(2, CACHE_OUTPUT)], Value::Float(amount));
        assert_eq!(graph.apply(&next, source).unwrap().0, next);
        assert_eq!(state, original);
        // A later failing sibling must roll back the earlier bank and reset.
        let mut failing = edges.clone();
        failing.push(edge(source, ep(1, ACCUMULATE)));
        let mut state = original.clone();
        state.properties.remove(&(1, INPUT));
        assert!(
            events::Graph::new(4, failing)
                .unwrap()
                .apply(&state, source)
                .is_err()
        );
        assert!(state.banked.is_empty());
        assert_eq!(state.nodes, original.nodes);
    }
}

#[test]
fn integer_participant_property_mirrors_each_written_value() {
    let mut node = Node::new(Definition::IntegerParticipantProperty).unwrap();
    // Binding writes the bound Value; the output mirrors it.
    assert_eq!(
        node.initial_outputs(|port| {
            assert_eq!(port, FLOAT_VALUE);
            Some(Value::Integer(0))
        })
        .unwrap(),
        vec![Effect::Publish(OUTPUT_VALUE, Value::Integer(0))]
    );
    for value in [1284, 1284, -3, 0] {
        assert_eq!(
            node.apply(Action::PropertyChanged(FLOAT_VALUE), |port| {
                assert_eq!(port, FLOAT_VALUE);
                Some(Value::Integer(value))
            })
            .unwrap(),
            [Effect::Publish(OUTPUT_VALUE, Value::Integer(value))]
        );
    }
    // Events, activation and other ports do nothing; a wrong type or a missing
    // value fails without a write.
    assert_eq!(node.apply(Action::Activate, |_| None).unwrap(), []);
    assert_eq!(node.apply(Action::Event(LOAD), |_| None).unwrap(), []);
    assert_eq!(
        node.apply(Action::PropertyChanged(OUTPUT_VALUE), |_| None)
            .unwrap(),
        []
    );
    assert_eq!(
        node.apply(Action::PropertyChanged(FLOAT_VALUE), |_| Some(
            Value::Float(1.0)
        )),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        node.apply(Action::PropertyChanged(FLOAT_VALUE), |_| None),
        Err(Error::MissingInput)
    );
}
