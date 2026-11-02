// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    commands::Command,
    tests::{definition, identity, plan},
};
use super::*;

fn input() -> SetSource {
    SetSource::Local(identity(80))
}
fn copied() -> SetSource {
    SetSource::Local(identity(81))
}
fn host(source: Option<SetSource>) -> Host {
    let mut d = definition();
    d.transitions.clear();
    d.bindings.sets = vec![
        data::bindings::ParticipantSet {
            identity: identity(80),
            kind: SetKind::Owned,
        },
        data::bindings::ParticipantSet {
            identity: identity(81),
            kind: SetKind::Copy { source },
        },
    ];
    d.bindings.set_counters = vec![data::bindings::SetCounter {
        identity: identity(82),
        source: Some(copied()),
    }];
    d.bindings.order = vec![Observer::Set(0), Observer::Set(1), Observer::SetCounter(0)];
    Host::prepare(plan(d), SceneInputs::default()).unwrap()
}
fn write(source: SetSource, participant: u64, member: bool) -> Command {
    Command::Set {
        source,
        participant,
        member,
    }
}
fn activate(binding: bool) -> Command {
    Command::Activate {
        observer: Observer::Set(1),
        binding,
    }
}
fn copy() -> Command {
    Command::CopySet {
        identity: identity(81),
        event: 0x7c81_3ec0,
    }
}
fn clear() -> Command {
    Command::CopySet {
        identity: identity(81),
        event: 0x0cac_9f9c,
    }
}
fn observe(subscription: Subscription) -> Command {
    Command::SetObserver {
        source: copied(),
        subscription,
        observer: 1,
        subscribe: true,
    }
}
fn run(h: &mut Host, commands: Vec<Command>) -> Vec<Effect> {
    h.dispatch(commands, &mut |_, _| Ok(vec![])).unwrap()
}
fn changes(effects: &[Effect]) -> Vec<Change> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::SetCallback(n) => Some(n.change),
            _ => None,
        })
        .collect()
}

#[test]
fn copy_is_event_owned_activation_does_not_seed_and_source_changes_do_not_leak() {
    let mut h = host(Some(input()));
    run(
        &mut h,
        vec![write(input(), 7, true), copy(), activate(true), copy()],
    );
    assert!(h.source_members(&copied()).unwrap().is_empty());
    run(&mut h, vec![activate(false)]);
    assert!(h.source_members(&copied()).unwrap().is_empty());
    let separate = h.clone();
    run(
        &mut h,
        vec![copy(), write(input(), 7, false), write(input(), 8, true)],
    );
    assert_eq!(h.source_members(&copied()).unwrap(), BTreeSet::from([7]));
    assert!(separate.source_members(&copied()).unwrap().is_empty());
    run(&mut h, vec![copy()]);
    assert_eq!(h.source_members(&copied()).unwrap(), BTreeSet::from([8]));
    assert!(run(&mut h, vec![copy()]).is_empty());
    let mut disconnected = host(None);
    run(
        &mut disconnected,
        vec![activate(false), write(copied(), 9, true), copy()],
    );
    assert_eq!(
        disconnected.source_members(&copied()).unwrap(),
        BTreeSet::from([9])
    );
    run(&mut disconnected, vec![clear()]);
    assert!(disconnected.source_members(&copied()).unwrap().is_empty());
}

#[test]
fn full_replacement_precedes_callbacks_and_additions_precede_removals() {
    let mut h = host(Some(input()));
    run(
        &mut h,
        vec![
            activate(false),
            write(copied(), 7, true),
            write(copied(), 8, true),
            write(input(), 8, true),
            write(input(), 9, true),
            observe(Subscription::Membership),
            observe(Subscription::Empty),
            Command::Activate {
                observer: Observer::SetCounter(0),
                binding: false,
            },
        ],
    );
    let mut tx = h.begin(vec![copy()]).unwrap();
    while let Some(step) = tx.advance().unwrap() {
        if let execution::Yield::Effect(_) = step {
            assert_eq!(
                tx.source_members(&copied()).unwrap(),
                BTreeSet::from([8, 9])
            );
        }
        tx.resume(vec![]).unwrap();
    }
    let (next, effects) = tx.finish().unwrap();
    assert_eq!(h.source_members(&copied()).unwrap(), BTreeSet::from([7, 8]));
    assert_eq!(
        changes(&effects),
        [
            Change::Membership {
                participant: 9,
                member: true
            },
            Change::Membership {
                participant: 7,
                member: false
            },
        ]
    );
    let counts: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Reporter(reporters::Effect::Count { value, .. }) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(counts, [2, 2]);
    h = next;
    run(
        &mut h,
        vec![write(input(), 8, false), write(input(), 9, false)],
    );
    assert_eq!(
        changes(&run(&mut h, vec![copy()])),
        [
            Change::Membership {
                participant: 8,
                member: false
            },
            Change::Membership {
                participant: 9,
                member: false
            },
        ]
    );
    assert!(h.source_members(&copied()).unwrap().is_empty());
}

#[test]
fn clear_publishes_empty_even_on_repeats_or_members_added_by_removal_callbacks() {
    let mut h = host(Some(input()));
    run(
        &mut h,
        vec![
            write(copied(), 7, true),
            observe(Subscription::Membership),
            observe(Subscription::Empty),
        ],
    );
    let effects = h
        .dispatch(vec![clear()], &mut |_, e| {
            Ok(
                if matches!(
                    e,
                    Effect::SetCallback(Notification {
                        change: Change::Membership {
                            participant: 7,
                            member: false
                        },
                        ..
                    })
                ) {
                    vec![write(copied(), 8, true)]
                } else {
                    vec![]
                },
            )
        })
        .unwrap();
    assert_eq!(
        changes(&effects),
        [
            Change::Membership {
                participant: 7,
                member: false
            },
            Change::Membership {
                participant: 8,
                member: true
            },
            Change::Empty,
        ]
    );
    assert_eq!(h.source_members(&copied()).unwrap(), BTreeSet::from([8]));
    let effects = run(&mut h, vec![clear(), clear()]);
    assert_eq!(
        changes(&effects),
        [
            Change::Membership {
                participant: 8,
                member: false
            },
            Change::Empty,
            Change::Empty
        ]
    );
}

#[test]
fn failed_or_unbounded_copy_callbacks_restore_membership_and_observer_state() {
    let mut h = host(Some(input()));
    run(
        &mut h,
        vec![
            activate(false),
            write(input(), 9, true),
            write(copied(), 7, true),
            observe(Subscription::Membership),
        ],
    );
    assert_eq!(
        h.dispatch(vec![copy()], &mut |_, e| {
            if matches!(e, Effect::SetCallback(_)) {
                Err(Error::Invalid)
            } else {
                Ok(vec![])
            }
        }),
        Err(Error::Invalid)
    );
    assert_eq!(h.source_members(&copied()).unwrap(), BTreeSet::from([7]));
    assert_eq!(changes(&run(&mut h, vec![copy()])).len(), 2);
    run(&mut h, vec![observe(Subscription::Empty)]);
    assert_eq!(
        h.dispatch(vec![clear()], &mut |_, e| Ok(
            if matches!(
                e,
                Effect::SetCallback(Notification {
                    change: Change::Empty,
                    ..
                })
            ) {
                vec![clear()]
            } else {
                vec![]
            }
        )),
        Err(Error::Bound)
    );
    assert_eq!(h.source_members(&copied()).unwrap(), BTreeSet::from([9]));
    assert_eq!(
        h.dispatch(
            vec![Command::CopySet {
                identity: identity(80),
                event: 0x7c81_3ec0
            }],
            &mut |_, _| Ok(vec![])
        ),
        Err(Error::Invalid)
    );
    assert!(
        run(
            &mut h,
            vec![Command::CopySet {
                identity: identity(81),
                event: 123
            }]
        )
        .is_empty()
    );
}
