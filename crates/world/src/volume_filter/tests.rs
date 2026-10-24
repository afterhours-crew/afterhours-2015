// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn assign(owner: u64, participant: u64, token: u32, inside: bool) -> Effect {
    Effect::Send {
        owner,
        participant,
        call: Call::Subscribe { token, inside },
    }
}
fn revoke(owner: u64, participant: u64) -> Effect {
    Effect::Send {
        owner,
        participant,
        call: Call::Unsubscribe,
    }
}
fn membership(participant: u64, inside: bool) -> Effect {
    Effect::Membership {
        participant,
        inside,
    }
}

#[test]
fn ownership_transfers_preserve_inside_and_reject_stale_generations() {
    let mut filter = Filter::default();
    assert_eq!(
        filter.add(7, Some((70, Some(1)))).unwrap(),
        [assign(1, 7, 0, false)]
    );
    assert_eq!(filter.report(7, 0, true), Some(membership(7, true)));
    assert_eq!(filter.report(7, 0, true), None);
    assert_eq!(
        filter.owner_changed(70, Some(2), |_| true).unwrap(),
        [revoke(1, 7), assign(2, 7, 1, true)]
    );
    assert_eq!(filter.report(7, 0, false), None);
    assert_eq!(filter.members(), &BTreeSet::from([7]));
    assert_eq!(filter.owner_changed(70, Some(2), |_| true).unwrap(), []);
    assert_eq!(
        filter.owner_changed(70, None, |_| true).unwrap(),
        [revoke(2, 7)]
    );
    assert_eq!(filter.generation(7), Some(1));
    assert_eq!(filter.members(), &BTreeSet::from([7]));
    assert_eq!(
        filter.owner_changed(70, Some(3), |_| true).unwrap(),
        [assign(3, 7, 2, true)]
    );
    assert_eq!(filter.report(7, 2, false), Some(membership(7, false)));
    filter.participants.get_mut(&7).unwrap().generation = u32::MAX;
    assert_eq!(
        filter.owner_changed(70, Some(4), |_| false).unwrap(),
        [assign(4, 7, 0, false)]
    );
}

#[test]
fn replacing_a_controllable_and_destroying_it_have_distinct_callbacks() {
    let mut filter = Filter::default();
    filter.add(7, Some((70, Some(1)))).unwrap();
    filter.report(7, 0, true);
    assert_eq!(
        filter
            .controllable_changed(7, Some((71, Some(2))), |_| true)
            .unwrap(),
        [assign(2, 7, 0, true)]
    );
    assert_eq!(filter.controllable_destroyed(70), []);
    assert_eq!(
        filter.controllable_destroyed(71),
        [revoke(2, 7), membership(7, false)]
    );
    assert_eq!(filter.generation(7), Some(0));
    assert_eq!(
        filter
            .controllable_changed(7, Some((72, Some(3))), |_| true)
            .unwrap(),
        [assign(3, 7, 0, true)]
    );
    assert!(filter.members().is_empty());
    // The cached inside flag does not suppress a new insertion into the set.
    assert_eq!(filter.report(7, 0, true), Some(membership(7, true)));
    assert_eq!(filter.controllable_changed(7, None, |_| true).unwrap(), []);
    assert_eq!(filter.owner(7), None);
    assert_eq!(filter.controllable_destroyed(72), []);
    assert_eq!(filter.members(), &BTreeSet::from([7]));
    assert_eq!(filter.remove(7, None), [membership(7, false)]);
}

#[test]
fn missing_owners_and_weak_destruction_do_not_invent_membership_changes() {
    let mut filter = Filter::default();
    assert_eq!(filter.add(7, None).unwrap(), []);
    assert_eq!(
        filter
            .controllable_changed(7, Some((70, None)), |_| true)
            .unwrap(),
        []
    );
    assert_eq!(
        filter.owner_changed(70, Some(1), |_| true).unwrap(),
        [assign(1, 7, 1, false)]
    );
    filter.report(7, 1, true);
    filter.owner_destroyed(1);
    assert_eq!(filter.owner(7), None);
    assert_eq!(filter.controllable_destroyed(70), []);
    assert_eq!(filter.members(), &BTreeSet::from([7]));
    assert_eq!(
        filter.remove(7, Some(2)),
        [revoke(2, 7), membership(7, false)]
    );
    assert_eq!(filter.report(7, 1, true), None);
    assert_eq!(filter.remove(7, Some(2)), []);
}

#[test]
fn network_boundary_rejects_unassigned_senders_but_ignores_retired_reports() {
    let mut filter = Filter::default();
    filter.add(7, Some((70, Some(1)))).unwrap();
    assert_eq!(filter.report_from(2, 7, 0, true), Err(Error::UnknownObject));
    assert!(filter.members().is_empty());
    assert_eq!(
        filter.report_from(1, 7, 0, true).unwrap(),
        Some(membership(7, true))
    );
    filter.owner_changed(70, Some(2), |_| true).unwrap();
    assert_eq!(filter.report_from(1, 7, 0, false), Ok(None));
    assert_eq!(
        filter.report_from(1, 7, 1, false),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        filter.report_from(2, 7, 1, false).unwrap(),
        Some(membership(7, false))
    );
    filter.remove(7, Some(2));
    assert_eq!(filter.report_from(2, 7, 1, true), Ok(None));
}

#[test]
fn bounded_and_isolated_lifetimes_cleanup_in_source_order() {
    let mut filter = Filter::default();
    for participant in 1..=MAX_PARTICIPANTS as u64 {
        filter
            .add(participant, Some((participant + 1000, Some(1))))
            .unwrap();
    }
    let before = filter.clone();
    assert_eq!(filter.add(129, None), Err(Error::Bound));
    assert_eq!(filter.add(0, None), Err(Error::Bound));
    assert_eq!(
        filter.controllable_changed(2, Some((1001, Some(1))), |_| true),
        Err(Error::DuplicateObject)
    );
    assert_eq!(filter.retire(&[(1, Some(1))]), Err(Error::Shape));
    assert_eq!(filter, before);
    let mut other = Filter::default();
    other.add(1, None).unwrap();
    other.report(1, 0, true);
    assert!(filter.members().is_empty());
    assert_eq!(other.members(), &BTreeSet::from([1]));
    filter.report(1, 0, true);
    let source: Vec<_> = (1..=MAX_PARTICIPANTS as u64)
        .rev()
        .map(|p| (p, Some(1)))
        .collect();
    let effects = filter.retire(&source).unwrap();
    assert_eq!(effects[0], revoke(1, 128));
    assert_eq!(&effects[127..], &[revoke(1, 1), membership(1, false)]);
    assert_eq!(filter, Filter::default());
    assert_eq!(filter.report(1, 0, true), None);
}
