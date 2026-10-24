// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn fixture() -> Machine {
    Machine::new(
        &(1..=4)
            .map(|id| Definition {
                id,
                handles_marked_for_delete: id == 4,
                send_rpc_to_client: id != 4,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn routed_notifications_keep_seed_targets_without_broadcasting_to_inactive_slots() {
    let machine = fixture();
    let mut run = machine
        .begin_routed(
            &[
                Action::Enter {
                    state: 1,
                    participant: 7,
                },
                Action::SeedMembers {
                    state: 1,
                    observer: 4000,
                },
            ],
            MAX_PENDING,
        )
        .unwrap();
    let mut calls = Vec::new();
    while let Some(call) = run.advance().unwrap() {
        calls.push(call);
        run.resume(vec![]).unwrap();
    }
    let (machine, _) = run.finish().unwrap();
    assert_eq!(
        calls,
        vec![
            Call {
                observer: 0,
                notification: Notification::Entered {
                    state: 1,
                    participant: 7,
                    send_rpc: true
                }
            },
            Call {
                observer: 4000,
                notification: Notification::Existing {
                    state: 1,
                    participant: 7
                }
            },
        ]
    );
    assert!(machine.members(1).unwrap().contains(&7));
}

#[test]
fn world_definition_install_and_storage_retirement_are_atomic_and_bounded() {
    let mut machine = Machine::empty();
    let definition = |id| Definition {
        id,
        handles_marked_for_delete: false,
        send_rpc_to_client: true,
    };
    machine.install(&[definition(1)]).unwrap();
    let before = machine.clone();
    assert_eq!(
        machine.install(&[definition(2), definition(1)]),
        Err(Error::Invalid)
    );
    assert_eq!(machine, before);
    machine
        .run(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            |_, _| Ok(vec![]),
        )
        .unwrap();
    let before = machine.clone();
    assert_eq!(machine.retire(&[1]), Err(Error::Invalid));
    assert_eq!(machine, before);
    machine
        .run(&[Action::RemoveAll { state: 1 }], |_, _| Ok(vec![]))
        .unwrap();
    machine.retire(&[1]).unwrap();
    for chunk in (0..MAX_WORLD_STATES as StateId)
        .collect::<Vec<_>>()
        .chunks(MAX_STATES)
    {
        machine
            .install(&chunk.iter().copied().map(definition).collect::<Vec<_>>())
            .unwrap();
    }
    let before = machine.clone();
    assert_eq!(
        machine.install(&[definition(MAX_WORLD_STATES as StateId)]),
        Err(Error::Bound)
    );
    assert_eq!(machine, before);
}

#[test]
fn first_member_move_uses_authored_order_without_requiring_completion() {
    let mut machine = fixture();
    machine
        .run(
            &[
                Action::Enter {
                    state: 1,
                    participant: 7,
                },
                Action::Enter {
                    state: 2,
                    participant: 7,
                },
            ],
            |_, _| Ok(vec![]),
        )
        .unwrap();
    let select = Action::MoveFirstMember {
        sources: vec![2, 1],
        participant: 7,
        target: Some(3),
    };
    let before = machine.clone();
    let mut reference_machine = before.clone();
    let mut callback = |_: &Machine, _: Notification, _: usize| Ok(vec![]);
    let mut reference = super::super::Run {
        machine: &mut reference_machine,
        observer: &mut callback,
        output: vec![],
        steps: 0,
        depth: 0,
        observer_count: 1,
    };
    reference.apply(select.clone()).unwrap();
    let expected = reference.output;
    let output = machine.run(&[select], |_, _| Ok(vec![])).unwrap();
    assert_eq!(output, expected);
    assert_eq!(machine, reference_machine);
    assert!(machine.members(1).unwrap().contains(&7));
    assert!(machine.members(2).unwrap().is_empty());
    assert!(machine.members(3).unwrap().contains(&7));
    assert!(
        !output
            .iter()
            .any(|n| matches!(n, Notification::Completed { .. }))
    );
    for invalid in [
        Action::MoveFirstMember {
            sources: vec![1, 99],
            participant: 7,
            target: Some(3),
        },
        Action::MoveFirstMember {
            sources: vec![1; MAX_STATES + 1],
            participant: 7,
            target: Some(3),
        },
    ] {
        let mut candidate = before.clone();
        assert!(candidate.run(&[invalid], |_, _| Ok(vec![])).is_err());
        assert_eq!(candidate, before);
    }
}

#[test]
fn first_member_move_waits_for_source_callbacks_and_rolls_back_failed_target_entry() {
    let mut machine = fixture();
    machine
        .run(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            |_, _| Ok(vec![]),
        )
        .unwrap();
    let before = machine.clone();
    for fail in [true, false] {
        let result = machine.run_ordered(
            &[Action::Complete {
                state: 1,
                participant: 7,
            }],
            2,
            |m, notification, observer| {
                if let Notification::Completed { .. } = notification {
                    assert!(m.members(1)?.contains(&7));
                    assert!(m.members(3)?.is_empty());
                    if observer == 0 {
                        return Ok(vec![Action::MoveFirstMember {
                            sources: vec![1, 2],
                            participant: 7,
                            target: Some(3),
                        }]);
                    }
                }
                if fail && matches!(notification, Notification::Entered { state: 3, .. }) {
                    return Err(Error::Invalid);
                }
                Ok(vec![])
            },
        );
        if fail {
            assert!(result.is_err());
            assert_eq!(machine, before);
        } else {
            result.unwrap();
            assert!(machine.members(1).unwrap().is_empty());
            assert!(machine.members(3).unwrap().contains(&7));
        }
    }
}

#[test]
fn direct_removal_retains_leaving_visibility_and_clears_members_without_transitioning() {
    let mut machine = fixture();
    machine
        .run(
            &[
                Action::Enter {
                    state: 1,
                    participant: 7,
                },
                Action::Enter {
                    state: 1,
                    participant: 8,
                },
                Action::Complete {
                    state: 1,
                    participant: 8,
                },
            ],
            |_, _| Ok(vec![]),
        )
        .unwrap();
    let output = machine
        .run(&[Action::RemoveAll { state: 1 }], |m, n| {
            match n {
                Notification::Leaving { state, participant } => {
                    assert!(m.members(state)?.contains(&participant));
                    if participant == 8 {
                        assert!(m.completed(state)?.contains(&8));
                    }
                }
                Notification::Left {
                    state,
                    participant,
                    send_rpc,
                } => {
                    assert!(send_rpc);
                    assert!(!m.members(state)?.contains(&participant));
                    assert!(!m.completed(state)?.contains(&participant));
                    if participant == 7 {
                        return Ok(vec![Action::Enter {
                            state: 1,
                            participant: 9,
                        }]);
                    }
                }
                Notification::Transitioned { .. } => panic!("direct removal invented a transition"),
                _ => {}
            }
            Ok(vec![])
        })
        .unwrap();
    assert!(machine.members(1).unwrap().is_empty());
    let removed: Vec<_> = output
        .iter()
        .filter_map(|n| match n {
            Notification::Left { participant, .. } => Some(*participant),
            _ => None,
        })
        .collect();
    assert_eq!(removed, [7, 8, 9]);
}

#[test]
fn removing_from_a_suspended_completion_callback_precedes_later_observers_and_pending_moves() {
    let mut machine = fixture();
    machine
        .run(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            |_, _| Ok(vec![]),
        )
        .unwrap();
    let output = machine
        .run_ordered(
            &[Action::Complete {
                state: 1,
                participant: 7,
            }],
            2,
            |m, n, observer| {
                if let Notification::Completed { .. } = n {
                    if observer == 0 {
                        return Ok(vec![
                            Action::Move {
                                source: 1,
                                participant: 7,
                                target: Some(2),
                            },
                            Action::RemoveAll { state: 1 },
                        ]);
                    }
                    assert!(m.members(1)?.is_empty());
                }
                Ok(vec![])
            },
        )
        .unwrap();
    assert!(machine.members(2).unwrap().is_empty());
    assert!(
        !output
            .iter()
            .any(|n| matches!(n, Notification::Transitioned { .. }))
    );
    assert_eq!(
        output
            .iter()
            .filter(|n| matches!(n, Notification::Left { .. }))
            .count(),
        1
    );
}

#[test]
fn removal_failure_and_unbounded_exit_reentry_roll_back() {
    let mut machine = fixture();
    machine
        .run(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            |_, _| Ok(vec![]),
        )
        .unwrap();
    let before = machine.clone();
    assert_eq!(
        machine.run(
            &[
                Action::RemoveAll { state: 1 },
                Action::RemoveAll { state: 999 }
            ],
            |_, _| Ok(vec![])
        ),
        Err(Error::UnknownState)
    );
    assert_eq!(machine, before);
    assert_eq!(
        machine.run(&[Action::RemoveAll { state: 1 }], |_, n| Ok(match n {
            Notification::Left {
                state, participant, ..
            } => vec![Action::Enter { state, participant }],
            _ => vec![],
        })),
        Err(Error::Bound)
    );
    assert_eq!(machine, before);
}

#[test]
fn direct_removal_matches_recursive_reference_and_silent_rpc_state_policy() {
    let actions = [
        Action::Enter {
            state: 4,
            participant: 7,
        },
        Action::Complete {
            state: 4,
            participant: 7,
        },
        Action::Remove {
            state: 4,
            participant: 7,
        },
        Action::Remove {
            state: 4,
            participant: 7,
        },
        Action::RemoveAll { state: 4 },
    ];
    let mut expected = fixture();
    let mut reference = super::super::Run {
        machine: &mut expected,
        observer: |_: &Machine, _: Notification, _: usize| Ok(vec![]),
        output: vec![],
        steps: 0,
        depth: 0,
        observer_count: 1,
    };
    for action in &actions {
        reference.apply(action.clone()).unwrap();
    }
    let expected_events = reference.output;
    let mut actual = fixture();
    let actual_events = actual.run(&actions, |_, _| Ok(vec![])).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual_events, expected_events);
    assert!(actual_events.iter().any(|n| matches!(
        n,
        Notification::Left {
            send_rpc: false,
            ..
        }
    )));
    assert!(
        !actual_events
            .iter()
            .any(|n| matches!(n, Notification::Transitioned { .. }))
    );
}

fn callback(m: &Machine, e: Notification, observer: usize) -> Vec<Action> {
    match (e, observer) {
        (
            Notification::Entered {
                state: 1,
                participant,
                ..
            },
            0,
        ) => vec![Action::Complete {
            state: 1,
            participant,
        }],
        (
            Notification::Completed {
                state: 1,
                participant,
            },
            1,
        ) => vec![
            Action::Move {
                source: 1,
                participant,
                target: Some(2),
            },
            Action::Move {
                source: 1,
                participant,
                target: Some(3),
            },
        ],
        (
            Notification::Existing {
                state: 3,
                participant,
            },
            0,
        ) => vec![Action::Move {
            source: 3,
            participant,
            target: Some(4),
        }],
        (Notification::Invoked { token: 77 }, 1) => vec![Action::CompleteAll {
            state: 4,
            only_incomplete: true,
        }],
        (
            Notification::Leaving {
                state: 3,
                participant,
            },
            1,
        ) if m.members(3).unwrap().contains(&participant) => {
            vec![Action::MarkForDelete(participant)]
        }
        _ => vec![],
    }
}

#[test]
fn suspended_dispatch_matches_recursive_membership_notifications_and_observer_snapshots() {
    for count in 1..=16 {
        let mut actions = (1..=count)
            .map(|participant| Action::Enter {
                state: 1,
                participant,
            })
            .collect::<Vec<_>>();
        actions.extend([
            Action::SeedMembers {
                state: 3,
                observer: 0,
            },
            Action::Invoke {
                observer: 1,
                token: 77,
            },
            Action::CompleteAll {
                state: 4,
                only_incomplete: false,
            },
            Action::MoveFirstCompleted {
                sources: vec![2, 4],
                participant: 1,
                target: Some(2),
            },
            Action::MoveAll {
                sources: vec![3, 4],
                target: Some(2),
            },
            Action::MarkForDelete(1),
        ]);
        let mut expected = fixture();
        let mut seen = Vec::new();
        let mut reference = super::super::Run {
            machine: &mut expected,
            observer: |m: &Machine, e, o| {
                seen.push((m.clone(), e, o));
                Ok(callback(m, e, o))
            },
            output: vec![],
            steps: 0,
            depth: 0,
            observer_count: 2,
        };
        for action in &actions {
            reference.apply(action.clone()).unwrap();
        }
        let expected_events = reference.output;
        let original = fixture();
        let mut execution = original.begin(&actions, 2).unwrap();
        let mut actual = Vec::new();
        while let Some(call) = execution.advance().unwrap() {
            actual.push((
                execution.machine().clone(),
                call.notification,
                call.observer,
            ));
            execution
                .resume(callback(
                    execution.machine(),
                    call.notification,
                    call.observer,
                ))
                .unwrap();
        }
        let (machine, events) = execution.finish().unwrap();
        assert_eq!(actual, seen);
        assert_eq!(events, expected_events);
        assert_eq!(machine, expected);
        assert_eq!(original, fixture());
    }
}

#[test]
fn yield_requires_exactly_one_response_and_faults_cannot_commit() {
    let mut execution = fixture()
        .begin(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            2,
        )
        .unwrap();
    assert!(execution.advance().unwrap().is_some());
    assert!(execution.clone().finish().is_err());
    assert_eq!(execution.advance(), Err(Error::Invalid));
    assert_eq!(execution.resume(vec![]), Err(Error::Invalid));
    assert_eq!(execution.finish(), Err(Error::Invalid));
    let mut execution = fixture().begin(&[], 1).unwrap();
    assert_eq!(execution.resume(vec![]), Err(Error::Invalid));
    assert_eq!(execution.finish(), Err(Error::Invalid));
}

#[test]
fn pending_and_recursive_limits_are_shared_across_every_resume() {
    let mut execution = fixture()
        .begin(
            &[Action::Invoke {
                observer: 0,
                token: 1,
            }],
            1,
        )
        .unwrap();
    loop {
        match execution.advance() {
            Ok(Some(_)) => execution
                .resume(vec![Action::Invoke {
                    observer: 0,
                    token: 1,
                }])
                .unwrap(),
            Err(error) => {
                assert_eq!(error, Error::Bound);
                break;
            }
            _ => panic!("recursive observer unexpectedly completed"),
        }
    }
    assert_eq!(execution.finish(), Err(Error::Bound));
    let mut execution = fixture()
        .begin(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            1,
        )
        .unwrap();
    execution.advance().unwrap().unwrap();
    assert_eq!(
        execution.resume(vec![Action::MarkForDelete(7); MAX_PENDING + 1]),
        Err(Error::Bound)
    );
    assert_eq!(execution.finish(), Err(Error::Bound));
}
