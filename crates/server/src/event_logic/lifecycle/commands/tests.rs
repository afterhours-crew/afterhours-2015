// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{identity, ranked};
use super::*;

fn write(value: i32) -> Command {
    Command::Integer {
        source: IntegerSource::Local(identity(80)),
        participant: 7,
        value,
    }
}

fn observe(observer: u64, subscribe: bool) -> Command {
    Command::IntegerObserver {
        source: IntegerSource::Local(identity(80)),
        observer,
        subscribe,
    }
}

#[test]
fn graph_listeners_join_native_sorter_order_and_mutate_the_live_subscription_list() {
    let mut h = Host::prepare(ranked(), SceneInputs::default()).unwrap();
    let mut calls = Vec::new();
    h.dispatch(
        vec![
            write(50), // Registration does not publish an old value by itself.
            observe(1, true),
            Command::Activate {
                observer: Observer::Sorter(0),
                binding: false,
            },
            observe(2, true),
            Command::Activate {
                observer: Observer::Sorter(1),
                binding: false,
            },
            observe(3, true),
            write(100),
        ],
        &mut |_, effect| {
            match effect {
                Effect::Integer(n) => {
                    calls.push((n.observer as usize, n.value));
                    if n.observer == 1 {
                        return Ok(vec![observe(2, false), observe(4, true)]);
                    }
                }
                Effect::Ranking(r) => calls.push((r.sorter.index, r.rows[0].value)),
                _ => {}
            }
            Ok(vec![])
        },
    )
    .unwrap();
    // Removal leaves a hole. The newly appended observer replaces it when
    // native iteration reaches that slot, before the second sorter.
    assert_eq!(calls, [(1, 100), (81, 100), (4, 100), (82, 100), (3, 100)]);
    assert_eq!(
        h.integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(100)
    );
    assert!(
        h.dispatch(vec![write(100)], &mut |_, _| panic!("equal write notified"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn listener_removal_is_first_occurrence_and_failure_restores_subscriptions_and_values() {
    let mut h = Host::prepare(ranked(), SceneInputs::default()).unwrap();
    h.dispatch(
        vec![observe(1, true), observe(1, true), observe(1, false)],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    assert_eq!(
        h.dispatch(vec![write(100)], &mut |_, effect| {
            if matches!(effect, Effect::Integer(_)) {
                Ok(vec![observe(1, false), observe(2, true), observe(0, true)])
            } else {
                Ok(vec![])
            }
        }),
        Err(Error::Invalid)
    );
    assert_eq!(
        h.integer_value(&IntegerSource::Local(identity(80)), 7),
        None
    );
    let effects = h
        .dispatch(vec![write(200)], &mut |_, _| Ok(vec![]))
        .unwrap();
    let notifications: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Integer(n) => Some((n.observer, n.value)),
            _ => None,
        })
        .collect();
    assert_eq!(notifications, [(1, 200)]);
}

#[test]
fn adding_listeners_from_callbacks_is_bounded_and_rolls_back() {
    let mut h = Host::prepare(ranked(), SceneInputs::default()).unwrap();
    assert_eq!(
        h.dispatch(vec![observe(1, true), write(100)], &mut |_, effect| {
            Ok(if matches!(effect, Effect::Integer(_)) {
                vec![observe(1, true)]
            } else {
                vec![]
            })
        }),
        Err(Error::Bound)
    );
    assert_eq!(
        h.integer_value(&IntegerSource::Local(identity(80)), 7),
        None
    );
    assert!(
        h.dispatch(vec![write(200)], &mut |_, _| panic!(
            "rolled-back listener retained"
        ))
        .unwrap()
        .is_empty()
    );
}

#[test]
fn nested_graph_write_completes_before_the_next_native_observer() {
    let mut h = Host::new(ranked()).unwrap();
    let mut calls = Vec::new();
    h.dispatch(vec![write(100)], &mut |_, effect| {
        let Effect::Ranking(row) = effect else {
            return Ok(vec![]);
        };
        calls.push((row.sorter.index, row.rows[0].value));
        if row.sorter == identity(81) && row.rows[0].value == 100 {
            Ok(vec![write(200)])
        } else {
            Ok(vec![])
        }
    })
    .unwrap();
    assert_eq!(calls, [(81, 100), (81, 200), (82, 200), (82, 100)]);
    assert_eq!(
        h.integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(200)
    );
    // The outer native callback still carries its original value argument.
    assert_eq!(h.ranking(0).unwrap()[0].value, 200);
    assert_eq!(h.ranking(1).unwrap()[0].value, 100);
}

#[test]
fn invalid_or_forged_nested_commands_roll_back_the_complete_host_transaction() {
    let mut h = Host::new(ranked()).unwrap();
    let invalid = Command::Integer {
        source: IntegerSource::Local(identity(900)),
        participant: 7,
        value: 1,
    };
    assert_eq!(
        h.dispatch(vec![write(100)], &mut |_, _| Ok(vec![invalid.clone()])),
        Err(Error::Invalid)
    );
    assert!(h.ranking(0).unwrap().is_empty());
    assert_eq!(
        h.integer_value(&IntegerSource::Local(identity(80)), 7),
        None
    );
    assert_eq!(
        h.dispatch(
            vec![Command::State(Action::Invoke {
                observer: 0,
                token: COMMAND_BIT | 1
            })],
            &mut |_, _| Ok(vec![])
        ),
        Err(Error::Invalid)
    );
    h.dispatch(vec![write(100)], &mut |_, _| Ok(vec![]))
        .unwrap();
    assert_eq!(h.ranking(1).unwrap()[0].value, 100);
}

#[test]
fn command_cycles_share_the_state_machine_budget_and_roll_back() {
    let mut h = Host::new(ranked()).unwrap();
    let result = h.dispatch(vec![write(1)], &mut |_, effect| {
        if let Effect::Ranking(row) = effect {
            Ok(vec![write(if row.rows[0].value == 1 { 2 } else { 1 })])
        } else {
            Ok(vec![])
        }
    });
    assert_eq!(result, Err(Error::Bound));
    assert!(h.ranking(0).unwrap().is_empty());
}

#[test]
fn results_observe_earlier_commands_and_complete_before_following_commands() {
    use data::bindings::results::{Definition, TRIGGER, Tag};
    let mut d = ranked().data().clone();
    let tag = Tag {
        asset: "tag".into(),
        object: 0,
        name: "score".into(),
        hash: 3,
    };
    d.bindings.results.push(Definition {
        identity: identity(90),
        scores: vec![(tag.clone(), Some(IntegerSource::Local(identity(80))))],
        collector: Link {
            identity: identity(91),
            port: 0,
            class: "ResultCollectorEntityData".into(),
            channel: None,
        },
    });
    let mut h = Host::new(Arc::new(Plan::scene(&d).unwrap())).unwrap();
    let mut seen = Vec::new();
    h.dispatch(
        vec![
            write(100),
            Command::Results {
                identity: identity(90),
                event: TRIGGER,
                context: Some(Context::Participant(7)),
            },
            write(300),
        ],
        &mut |_, effect| {
            if let Effect::Results(result) = effect {
                seen.push(result.values[&7][&tag]);
                Ok(vec![write(200)])
            } else {
                Ok(vec![])
            }
        },
    )
    .unwrap();
    assert_eq!(seen, [100]);
    assert_eq!(
        h.integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(300)
    );
}
