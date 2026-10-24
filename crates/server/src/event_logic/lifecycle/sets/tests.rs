// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    commands::Command,
    tests::{definition, identity, plan},
};
use super::*;

fn source() -> SetSource {
    SetSource::Local(identity(80))
}
fn host() -> Host {
    let mut d = definition();
    d.transitions.clear();
    d.bindings.sets = vec![data::bindings::ParticipantSet {
        identity: identity(80),
        kind: SetKind::Owned,
    }];
    d.bindings.set_counters = vec![data::bindings::SetCounter {
        identity: identity(81),
        source: Some(source()),
    }];
    d.bindings.order = vec![Observer::Set(0), Observer::SetCounter(0)];
    Host::prepare(plan(d), SceneInputs::default()).unwrap()
}
fn observe(observer: u64, subscription: Subscription, subscribe: bool) -> Command {
    Command::SetObserver {
        source: source(),
        subscription,
        observer,
        subscribe,
    }
}
fn write(participant: u64, member: bool) -> Command {
    Command::Set {
        source: source(),
        participant,
        member,
    }
}

#[test]
fn native_list_order_includes_new_observers_and_counters_read_current_membership() {
    let mut h = host();
    h.dispatch(
        vec![
            observe(1, Subscription::Membership, true),
            Command::Activate {
                observer: Observer::SetCounter(0),
                binding: false,
            },
            observe(2, Subscription::Membership, true),
        ],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    let mut trace = vec![];
    h.dispatch(vec![write(7, true)], &mut |_, effect| {
        match effect {
            Effect::SetCallback(Notification {
                observer,
                change:
                    Change::Membership {
                        participant,
                        member,
                    },
                ..
            }) => {
                trace.push((*observer, *participant, i32::from(*member)));
                if *observer == 1 && *participant == 7 {
                    return Ok(vec![
                        observe(2, Subscription::Membership, false),
                        observe(3, Subscription::Membership, true),
                        write(8, true),
                    ]);
                }
            }
            Effect::Reporter(reporters::Effect::Count { value, .. }) => trace.push((81, 0, *value)),
            _ => {}
        }
        Ok(vec![])
    })
    .unwrap();
    assert_eq!(
        trace,
        [
            (1, 7, 1),
            (1, 8, 1),
            (81, 0, 2),
            (3, 8, 1),
            (81, 0, 2),
            (3, 7, 1)
        ]
    );
    assert_eq!(h.source_members(&source()).unwrap(), BTreeSet::from([7, 8]));
}

#[test]
fn empty_is_checked_after_removal_callbacks_and_delivered_to_its_own_live_list() {
    let mut h = host();
    h.dispatch(
        vec![
            write(7, true),
            observe(1, Subscription::Membership, true),
            observe(10, Subscription::Empty, true),
            observe(11, Subscription::Empty, true),
        ],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    let output = h
        .dispatch(vec![write(7, false)], &mut |_, effect| {
            Ok(
                if matches!(
                    effect,
                    Effect::SetCallback(Notification {
                        observer: 1,
                        change: Change::Membership { member: false, .. },
                        ..
                    })
                ) {
                    vec![write(8, true)]
                } else {
                    vec![]
                },
            )
        })
        .unwrap();
    assert!(!output.iter().any(|e| matches!(
        e,
        Effect::SetCallback(Notification {
            change: Change::Empty,
            ..
        })
    )));
    let mut empty_trace = vec![];
    h.dispatch(
        vec![observe(1, Subscription::Membership, false), write(8, false)],
        &mut |_, effect| {
            if let Effect::SetCallback(Notification {
                observer,
                change: Change::Empty,
                ..
            }) = effect
            {
                empty_trace.push(*observer);
                if *observer == 10 {
                    // Once empty dispatch starts, a nested addition does not cancel
                    // the remaining empty callbacks (0405e160).
                    return Ok(vec![
                        write(9, true),
                        observe(11, Subscription::Empty, false),
                        observe(12, Subscription::Empty, true),
                    ]);
                }
            }
            Ok(vec![])
        },
    )
    .unwrap();
    assert_eq!(empty_trace, [10, 12]);
    assert_eq!(h.source_members(&source()).unwrap(), BTreeSet::from([9]));
    assert!(
        h.dispatch(vec![write(8, false), write(9, true)], &mut |_, _| panic!(
            "duplicate mutation notified"
        ))
        .unwrap()
        .is_empty()
    );
}

#[test]
fn removal_unsubscribes_one_occurrence_and_failed_callbacks_roll_back_membership() {
    let mut h = host();
    h.dispatch(
        vec![
            observe(1, Subscription::Membership, true),
            observe(1, Subscription::Membership, true),
            observe(1, Subscription::Membership, false),
        ],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    assert_eq!(
        h.dispatch(vec![write(7, true)], &mut |_, effect| {
            Ok(if matches!(effect, Effect::SetCallback(_)) {
                vec![
                    observe(1, Subscription::Membership, false),
                    observe(0, Subscription::Empty, true),
                ]
            } else {
                vec![]
            })
        }),
        Err(Error::Invalid)
    );
    assert!(h.source_members(&source()).unwrap().is_empty());
    let effects = h
        .dispatch(vec![write(7, true)], &mut |_, _| Ok(vec![]))
        .unwrap();
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, Effect::SetCallback(_)))
            .count(),
        1
    );
    assert_eq!(
        h.source_members(&SetSource::Local(identity(999))),
        Err(Error::Invalid)
    );
}

#[test]
fn unbounded_listener_append_fails_without_committing_the_source_or_subscriptions() {
    let mut h = host();
    assert_eq!(
        h.dispatch(
            vec![observe(1, Subscription::Membership, true), write(7, true)],
            &mut |_, effect| {
                Ok(if matches!(effect, Effect::SetCallback(_)) {
                    vec![observe(1, Subscription::Membership, true)]
                } else {
                    vec![]
                })
            }
        ),
        Err(Error::Bound)
    );
    assert!(h.source_members(&source()).unwrap().is_empty());
    let effects = h
        .dispatch(vec![write(7, true)], &mut |_, _| Ok(vec![]))
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::Set { .. }));
}
