// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::events::{self, Connection, Delivery, Dispatch, Endpoint, Handler};
use super::*;

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
fn pair(kind: Comparison, value: i32) -> impl FnMut(u32) -> Option<Value> {
    move |port| {
        let n = if port == A { value } else { 0 };
        Some(match kind {
            Comparison::Float => Value::Float(n as f32),
            Comparison::Integer => Value::Integer(n),
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
            acc.apply(Action::Event(ACCUMULATE), |_| Some(value))
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
        assert!(node.apply(Action::Activate, |_| value).is_err());
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
    let mut cache = Node::new(Definition::IntegerCache).unwrap();
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
fn conditional_float_reads_and_publishes_only_the_selected_branch() {
    let mut node = Node::new(Definition::ConditionalFloat).unwrap();
    assert_eq!(
        node.initial_outputs(|_| None).unwrap(),
        [Effect::Publish(OUTPUT, Value::Float(0.0))]
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
                Some(Value::Float(8.25))
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
                [Effect::Publish(OUTPUT, Value::Float(8.25))]
            );
        }
        assert!(
            node.apply(Action::PropertyChanged(other), read)
                .unwrap()
                .is_empty()
        );
        assert!(node.apply(Action::Event(IN), |_| None).unwrap().is_empty());
    }
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
                    props.get(&(1, OUTPUT)).copied()
                } else {
                    props.get(&(target.node, port)).copied()
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
