// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn machine() -> Machine {
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
fn enter(state: StateId, participant: Participant) -> Action {
    Action::Enter { state, participant }
}
fn complete(state: StateId, participant: Participant) -> Action {
    Action::Complete { state, participant }
}
fn move_to(source: StateId, participant: Participant, target: Option<StateId>) -> Action {
    Action::Move {
        source,
        participant,
        target,
    }
}
fn quiet(_: &Machine, _: Notification) -> Result<Vec<Action>, Error> {
    Ok(vec![])
}

#[test]
fn each_observer_sees_synchronous_actions_from_preceding_observers() {
    let mut m = machine();
    let mut seen = Vec::new();
    m.run_ordered(&[enter(1, 7)], 2, |m, event, observer| {
        seen.push((event, observer));
        if matches!(event, Notification::Entered { .. }) {
            if observer == 0 {
                return Ok(vec![complete(1, 7)]);
            }
            assert!(m.completed(1)?.contains(&7));
        }
        Ok(vec![])
    })
    .unwrap();
    assert_eq!(seen.len(), 4);
    assert!(matches!(seen[0], (Notification::Entered { .. }, 0)));
    assert!(matches!(seen[1], (Notification::Completed { .. }, 0)));
    assert!(matches!(seen[2], (Notification::Completed { .. }, 1)));
    assert!(matches!(seen[3], (Notification::Entered { .. }, 1)));
}

#[test]
fn complete_all_defers_moves_and_updateable_skips_already_completed_members() {
    let mut m = machine();
    m.run(&[enter(1, 10), enter(1, 20), complete(1, 10)], quiet)
        .unwrap();
    let out = m
        .run(
            &[Action::CompleteAll {
                state: 1,
                only_incomplete: true,
            }],
            quiet,
        )
        .unwrap();
    assert_eq!(
        out,
        [Notification::Completed {
            state: 1,
            participant: 20
        }]
    );
    let out = m
        .run(
            &[Action::CompleteAll {
                state: 1,
                only_incomplete: false,
            }],
            |m, e| {
                if let Notification::Completed { state, participant } = e {
                    assert_eq!(m.members(state)?.len(), 2);
                    Ok(vec![move_to(state, participant, None)])
                } else {
                    Ok(vec![])
                }
            },
        )
        .unwrap();
    assert_eq!(
        &out[..2],
        &[
            Notification::Completed {
                state: 1,
                participant: 10
            },
            Notification::Completed {
                state: 1,
                participant: 20
            }
        ]
    );
    assert_eq!(
        out.iter()
            .filter_map(|e| match e {
                Notification::Left { participant, .. } => Some(*participant),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [20, 10]
    );
}

#[test]
fn activation_seeding_targets_one_observer_and_drains_before_the_next_source() {
    let mut m = machine();
    m.run(&[enter(1, 10), enter(1, 20)], quiet).unwrap();
    let mut seeded = Vec::new();
    m.run_ordered(
        &[
            Action::SeedMembers {
                state: 1,
                observer: 1,
            },
            Action::SeedMembers {
                state: 2,
                observer: 1,
            },
        ],
        2,
        |m, e, observer| {
            if let Notification::Existing { state, participant } = e {
                assert_eq!(observer, 1);
                seeded.push((state, participant));
                if state == 1 {
                    assert_eq!(m.members(1)?.len(), 2);
                    return Ok(vec![move_to(1, participant, Some(2))]);
                }
            }
            Ok(vec![])
        },
    )
    .unwrap();
    assert_eq!(seeded, [(1, 10), (1, 20), (2, 10), (2, 20)]);
    let before = m.clone();
    assert_eq!(
        m.run_ordered(
            &[Action::SeedMembers {
                state: 2,
                observer: 2
            }],
            2,
            |_, _, _| Ok(vec![])
        ),
        Err(Error::Invalid)
    );
    assert_eq!(m, before);
}

#[test]
fn trigger_all_includes_uncompleted_members_and_drains_each_source_lifo() {
    let mut m = machine();
    m.run(&[enter(1, 20), enter(1, 10), enter(2, 30)], quiet)
        .unwrap();
    let out = m
        .run(
            &[Action::MoveAll {
                sources: vec![1, 2],
                target: Some(3),
            }],
            quiet,
        )
        .unwrap();
    let moved = out
        .into_iter()
        .filter_map(|e| match e {
            Notification::Transitioned {
                source,
                participant,
                ..
            } => Some((source, participant)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(moved, vec![(1, 20), (1, 10), (2, 30)]);
    assert_eq!(m.members(3).unwrap(), &BTreeSet::from([10, 20, 30]));
    assert!(m.completed(3).unwrap().is_empty());
}

#[test]
fn completed_membership_is_separate_and_source_selection_retains_authored_order() {
    let mut m = machine();
    m.run(&[enter(1, 77), enter(2, 77), enter(1, 88)], quiet)
        .unwrap();
    let request = Action::MoveFirstCompleted {
        sources: vec![2, 1],
        participant: 77,
        target: Some(3),
    };
    assert!(
        m.run(std::slice::from_ref(&request), quiet)
            .unwrap()
            .is_empty()
    );
    m.run(&[complete(1, 77), complete(2, 77)], quiet).unwrap();
    assert_eq!(m.members(1).unwrap(), &BTreeSet::from([77, 88]));
    assert_eq!(m.completed(1).unwrap(), &BTreeSet::from([77]));
    let out = m.run(&[request], quiet).unwrap();
    assert_eq!(
        out,
        vec![
            Notification::Leaving {
                state: 2,
                participant: 77
            },
            Notification::Left {
                state: 2,
                participant: 77,
                send_rpc: true
            },
            Notification::Transitioned {
                source: 2,
                participant: 77,
                target: Some(3)
            },
            Notification::Entered {
                state: 3,
                participant: 77,
                send_rpc: true
            },
        ]
    );
    assert!(m.members(1).unwrap().contains(&77));
    assert!(m.completed(1).unwrap().contains(&77));
    assert!(m.completed(2).unwrap().is_empty());
}

#[test]
fn callbacks_defer_lifo_moves_and_stale_requests_cannot_enter_twice() {
    let mut m = machine();
    m.run(&[enter(1, 77)], quiet).unwrap();
    let out = m
        .run(&[complete(1, 77)], |view, event| match event {
            Notification::Completed {
                state: 1,
                participant: 77,
            } => {
                assert!(view.members(1)?.contains(&77));
                assert!(view.completed(1)?.contains(&77));
                Ok(vec![move_to(1, 77, Some(2)), move_to(1, 77, Some(3))])
            }
            Notification::Leaving {
                state: 1,
                participant: 77,
            } => {
                assert!(view.members(1)?.contains(&77));
                assert!(view.completed(1)?.contains(&77));
                Ok(vec![])
            }
            Notification::Left {
                state: 1,
                participant: 77,
                ..
            } => {
                assert!(!view.members(1)?.contains(&77));
                assert!(!view.completed(1)?.contains(&77));
                Ok(vec![])
            }
            _ => Ok(vec![]),
        })
        .unwrap();
    assert_eq!(out.len(), 5);
    assert!(m.members(2).unwrap().is_empty());
    assert_eq!(m.members(3).unwrap(), &BTreeSet::from([77]));
    assert!(
        m.run(&[complete(1, 77), move_to(1, 77, Some(2))], quiet)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn nested_target_callbacks_finish_before_source_queue_resumes() {
    let mut m = machine();
    m.run(&[enter(1, 77), enter(1, 88)], quiet).unwrap();
    let out = m
        .run(&[complete(1, 77)], |_, event| {
            Ok(match event {
                Notification::Completed {
                    state: 1,
                    participant: 77,
                } => vec![move_to(1, 88, None), move_to(1, 77, Some(2))],
                Notification::Entered {
                    state: 2,
                    participant: 77,
                    ..
                } => vec![complete(2, 77)],
                Notification::Completed {
                    state: 2,
                    participant: 77,
                } => vec![move_to(2, 77, Some(3))],
                _ => vec![],
            })
        })
        .unwrap();
    let entered = out
        .iter()
        .position(|e| matches!(e, Notification::Entered { state: 3, .. }))
        .unwrap();
    let removed = out
        .iter()
        .position(|e| {
            matches!(
                e,
                Notification::Leaving {
                    participant: 88,
                    ..
                }
            )
        })
        .unwrap();
    assert!(entered < removed);
    assert!(m.members(1).unwrap().is_empty());
    assert!(m.members(2).unwrap().is_empty());
    assert_eq!(m.members(3).unwrap(), &BTreeSet::from([77]));
}

#[test]
fn marked_participant_completes_only_states_that_do_not_handle_deletion() {
    let mut m = machine();
    m.run(&[enter(1, 77), enter(4, 77)], quiet).unwrap();
    let out = m.run(&[Action::MarkForDelete(77)], quiet).unwrap();
    assert_eq!(
        out,
        vec![Notification::Completed {
            state: 1,
            participant: 77
        }]
    );
    assert!(m.completed(4).unwrap().is_empty());
    assert!(
        m.run(&[Action::MarkForDelete(77)], quiet)
            .unwrap()
            .is_empty()
    );
    let out = m.run(&[enter(2, 77)], quiet).unwrap();
    assert_eq!(
        out.last(),
        Some(&Notification::Completed {
            state: 2,
            participant: 77
        })
    );
}

#[test]
fn repeated_completion_remains_a_callback_but_cycles_and_invalid_batches_roll_back() {
    let mut m = machine();
    m.run(&[enter(1, 77)], quiet).unwrap();
    assert_eq!(
        m.run(&[complete(1, 77), complete(1, 77)], quiet)
            .unwrap()
            .len(),
        2
    );
    let before = m.clone();
    assert_eq!(
        m.run(&[move_to(1, 77, Some(2)), move_to(2, 77, Some(999))], quiet),
        Err(Error::UnknownState)
    );
    assert_eq!(m, before);
    assert_eq!(
        m.run(&[complete(1, 77)], |_, e| Ok(match e {
            Notification::Completed { state, participant } => vec![complete(state, participant)],
            _ => vec![],
        })),
        Err(Error::Bound)
    );
    assert_eq!(m, before);
    let mut other = machine();
    assert!(other.run(&[complete(1, 77)], quiet).unwrap().is_empty());
}

#[test]
fn members_and_pending_requests_are_bounded_transactionally() {
    let mut m = machine();
    let actions = (0..MAX_PARTICIPANTS)
        .map(|p| enter(1, p as u64))
        .collect::<Vec<_>>();
    m.run(&actions, quiet).unwrap();
    let before = m.clone();
    assert_eq!(
        m.run(&[enter(1, MAX_PARTICIPANTS as u64)], quiet),
        Err(Error::Bound)
    );
    assert_eq!(m, before);
    assert_eq!(
        m.run(&[complete(1, 0)], |_, e| Ok(match e {
            Notification::Completed { .. } => vec![move_to(1, 0, None); MAX_PENDING + 1],
            _ => vec![],
        })),
        Err(Error::Bound)
    );
    assert_eq!(m, before);
}
