// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::participant_states::{Action, Definition as State};

fn machine() -> Machine {
    Machine::new(
        &(0..4)
            .map(|id| State {
                id,
                handles_marked_for_delete: false,
                send_rpc_to_client: true,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
fn enter(state: StateId) -> Action {
    Action::Enter {
        state,
        participant: 7,
    }
}
fn move_to(source: StateId, target: Option<StateId>) -> Action {
    Action::Move {
        source,
        participant: 7,
        target,
    }
}

#[test]
fn list_internal_transition_has_no_false_gap_and_completion_does_not_clear() {
    let mut m = machine();
    let mut p = Property::new(Definition::List(vec![0, 1]), &m).unwrap();
    let mut changes = Vec::new();
    m.run(
        &[
            enter(0),
            move_to(0, Some(1)),
            Action::Complete {
                state: 1,
                participant: 7,
            },
            move_to(1, Some(2)),
        ],
        |_, event| {
            if let Some(c) = p.observe(event)? {
                changes.push(c.value);
            }
            Ok(vec![])
        },
    )
    .unwrap();
    assert_eq!(changes, [true, false]);
    assert_eq!(p.value(7), Some(false));
}

#[test]
fn list_uses_transition_target_even_when_another_watched_membership_remains() {
    let mut m = machine();
    let mut p = Property::new(Definition::List(vec![0, 1]), &m).unwrap();
    m.run(&[enter(0), enter(1), move_to(0, None)], |_, event| {
        p.observe(event)?;
        Ok(vec![])
    })
    .unwrap();
    assert!(m.members(1).unwrap().contains(&7));
    assert_eq!(p.value(7), Some(false));
}

#[test]
fn range_latches_across_intermediate_states_and_clears_before_end_is_removed() {
    let mut m = machine();
    let mut p = Property::new(Definition::Range { begin: 0, end: 2 }, &m).unwrap();
    let mut seen = Vec::new();
    m.run(
        &[
            enter(0),
            move_to(0, Some(1)),
            move_to(1, Some(2)),
            move_to(2, Some(3)),
        ],
        |m, event| {
            if let Some(c) = p.observe(event)? {
                if !c.value {
                    assert!(m.members(2)?.contains(&7));
                }
                seen.push(c.value);
            }
            Ok(vec![])
        },
    )
    .unwrap();
    assert_eq!(seen, [true, false]);
    assert_eq!(p.seed_sources(), [0]);
}

#[test]
fn first_false_notifies_repeats_do_not_and_deletion_releases_property_capacity() {
    let m = machine();
    let mut p = Property::new(Definition::Range { begin: 0, end: 1 }, &m).unwrap();
    let leave = |participant| Notification::Leaving {
        state: 1,
        participant,
    };
    assert_eq!(p.value(7), None);
    assert_eq!(
        p.observe(leave(7)).unwrap(),
        Some(Change {
            participant: 7,
            value: false
        })
    );
    assert_eq!(p.observe(leave(7)).unwrap(), None);
    p.forget(7);
    for i in 0..MAX_PARTICIPANTS as u64 {
        p.observe(leave(i)).unwrap();
    }
    let before = p.clone();
    assert_eq!(p.observe(leave(999)), Err(Error::Bound));
    assert_eq!(p, before);
    p.forget(7);
    assert!(p.observe(leave(999)).unwrap().is_some());
}

#[test]
fn same_range_endpoint_reentry_and_authored_activation_order_are_preserved() {
    let mut m = machine();
    let mut p = Property::new(Definition::Range { begin: 1, end: 1 }, &m).unwrap();
    let mut changes = Vec::new();
    m.run(&[enter(1), move_to(1, Some(1))], |_, event| {
        if let Some(c) = p.observe(event)? {
            changes.push(c.value);
        }
        Ok(vec![])
    })
    .unwrap();
    assert_eq!(changes, [true, false, true]);
    let mut list = Property::new(Definition::List(vec![2, 0, 1]), &m).unwrap();
    assert_eq!(list.seed_sources(), [2, 0, 1]);
    for state in list.seed_sources() {
        for &participant in m.members(state).unwrap() {
            list.seed_member(participant).unwrap();
        }
    }
    assert_eq!(list.value(7), Some(true));
    assert_eq!(p.value(8), None);
    assert_eq!(
        Property::new(Definition::List(vec![9]), &m),
        Err(Error::UnknownState)
    );
    assert_eq!(
        Property::new(Definition::List(vec![0; MAX_STATES + 1]), &m),
        Err(Error::Bound)
    );
}
