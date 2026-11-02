// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::progression::tests::{combined, local, tables};
use nfs_storage::{Batch, InventoryRepository, Op, Timestamp};

fn owners() -> StateOwners {
    StateOwners {
        objectives: BTreeMap::from([
            (101, data::Expr::Constant(true)),
            (102, data::Expr::Completed(101)),
        ]),
        integers: BTreeSet::from([103]),
    }
}
fn facts() -> BTreeSet<Fact> {
    BTreeSet::from([
        Fact::Active(10),
        Fact::Completed(20),
        Fact::PositiveInt(42),
        Fact::Active(101),
        Fact::Completed(101),
        Fact::Active(102),
        Fact::PositiveInt(103),
    ])
}

#[test]
fn auxiliary_projection_uses_authored_initials_then_durable_rows_across_restart() {
    let content = Content::from_json(&combined()).unwrap();
    let owners = owners();
    let requested = facts();
    let (repo, who, snapshot) = local();
    let initial = content
        .condition_facts(&requested, &owners, &tables().view(&snapshot).unwrap())
        .unwrap();
    assert_eq!(
        initial,
        BTreeMap::from([
            (Fact::Active(10), true),
            (Fact::Completed(20), false),
            (Fact::PositiveInt(42), false),
            (Fact::Active(101), true),
            (Fact::Completed(101), false),
            (Fact::Active(102), false),
            (Fact::PositiveInt(103), false),
        ])
    );
    let mut objectives = snapshot.tables[&OBJECTIVES].clone();
    objectives.rows.insert(
        101,
        BTreeMap::from([(ACTIVE, Value::Int(1)), (COMPLETED, Value::Int(1))]),
    );
    objectives.rows.insert(
        102,
        BTreeMap::from([(ACTIVE, Value::Int(1)), (COMPLETED, Value::Int(0))]),
    );
    let mut integers = snapshot.tables[&VALUES].clone();
    integers
        .rows
        .insert(103, BTreeMap::from([(INTS, Value::Int(7))]));
    repo.apply(
        who,
        &Batch {
            id: 900,
            expected_generation: snapshot.generation,
            ops: vec![
                Op::SetTable(OBJECTIVES, objectives),
                Op::SetTable(VALUES, integers),
            ],
        },
        Timestamp(3),
    )
    .unwrap();
    let saved = repo.snapshot(who).unwrap();
    let restarted = Content::from_json(&combined()).unwrap();
    let actual = restarted
        .condition_facts(&requested, &owners, &tables().view(&saved).unwrap())
        .unwrap();
    assert!(!actual[&Fact::Active(101)]);
    assert!(actual[&Fact::Completed(101)]);
    assert!(actual[&Fact::Active(102)]);
    assert!(actual[&Fact::PositiveInt(103)]);
    // A separate empty account view must not inherit these auxiliary states.
    let (_, _, other) = local();
    assert_eq!(
        restarted
            .condition_facts(&requested, &owners, &tables().view(&other).unwrap())
            .unwrap(),
        initial
    );
    assert_eq!(repo.snapshot(who).unwrap(), saved);
}

#[test]
fn partial_objective_rows_keep_shared_restore_semantics_and_signed_integers() {
    let content = Content::from_json(&combined()).unwrap();
    let (_, _, mut snapshot) = local();
    snapshot
        .tables
        .get_mut(&OBJECTIVES)
        .unwrap()
        .rows
        .insert(101, BTreeMap::from([(ACTIVE, Value::Int(0))]));
    for integer in [-9, 0, 5] {
        snapshot
            .tables
            .get_mut(&VALUES)
            .unwrap()
            .rows
            .insert(103, BTreeMap::from([(INTS, Value::Int(integer))]));
        let actual = content
            .condition_facts(&facts(), &owners(), &tables().view(&snapshot).unwrap())
            .unwrap();
        assert!(actual[&Fact::Active(101)]);
        assert!(!actual[&Fact::Completed(101)]);
        assert_eq!(actual[&Fact::PositiveInt(103)], integer > 0);
    }
}

#[test]
fn unknown_or_conflicting_ownership_is_never_a_false_fact() {
    let content = Content::from_json(&combined()).unwrap();
    let (_, _, snapshot) = local();
    let loaded = tables().view(&snapshot).unwrap();
    for fact in [
        Fact::Active(999),
        Fact::Completed(999),
        Fact::PositiveInt(999),
        Fact::MessageRead {
            hash: 81,
            manager: 4,
        },
    ] {
        assert!(
            content
                .condition_facts(&BTreeSet::from([fact]), &owners(), &loaded)
                .is_err()
        );
    }
    for id in [0, 10] {
        let mut ambiguous = owners();
        ambiguous.objectives.insert(id, data::Expr::Constant(false));
        assert!(
            content
                .condition_facts(&facts(), &ambiguous, &loaded)
                .is_err()
        );
    }
    let mut ambiguous = owners();
    ambiguous.integers.insert(42);
    assert!(
        content
            .condition_facts(&facts(), &ambiguous, &loaded)
            .is_err()
    );
}
