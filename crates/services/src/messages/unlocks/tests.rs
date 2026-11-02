// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[test]
fn versioned_pending_table_rejects_ambiguous_or_unbounded_state() {
    let row = Pending {
        hash: u32::MAX,
        signal: u64::MAX,
        part: Some(u32::MAX),
    };
    let table = store([row]).unwrap();
    assert_eq!(pending(Some(&table)).unwrap(), [row]);
    for (column, value) in [
        ("Version", Value::Int(0)),
        ("Version", Value::Int(2)),
        ("Signal", Value::String("01".into())),
        ("Signal", Value::String("0".into())),
        ("Signal", Value::String("18446744073709551616".into())),
        ("Kind", Value::Int(0)),
        ("Kind", Value::Int(2)),
        ("Part", Value::Int(0)),
        ("Extra", Value::Int(1)),
    ] {
        let mut bad = table.clone();
        bad.rows
            .get_mut(&u64::from(row.hash))
            .unwrap()
            .insert(key(column), value);
        assert!(pending(Some(&bad)).is_err(), "{column}");
    }
    for hash in [0, u64::from(u32::MAX) + 1] {
        let mut bad = table.clone();
        let cells = bad.rows.pop_first().unwrap().1;
        bad.rows.insert(hash, cells);
        assert!(pending(Some(&bad)).is_err());
    }
    assert!(store((1..=65).map(|hash| Pending { hash, ..row })).is_err());
}

#[test]
fn configuration_and_restored_rows_must_describe_the_same_senders_and_parts() {
    let rule = Trigger::Part {
        include: false,
        ids: BTreeSet::new(),
    };
    let policy = Policy::new(None, [([2; 16], 200)].into(), [(100, rule.clone())].into()).unwrap();
    let row = Pending {
        hash: 100,
        signal: 5,
        part: Some(200),
    };
    assert_eq!(policy.pending(Some(&store([row]).unwrap())).unwrap(), [row]);
    for bad in [
        Pending { hash: 101, ..row },
        Pending { part: None, ..row },
        Pending {
            part: Some(201),
            ..row
        },
    ] {
        assert!(policy.pending(Some(&store([bad]).unwrap())).is_err());
    }
    assert!(Policy::new(None, BTreeMap::new(), [(100, rule.clone())].into()).is_err());
    assert!(
        Policy::new(
            None,
            [([2; 16], 200), ([3; 16], 200)].into(),
            [(100, rule)].into()
        )
        .is_err()
    );
    assert!(
        Policy::new(
            None,
            BTreeMap::new(),
            [(100, Trigger::Categories { mask: 1 })].into()
        )
        .is_err()
    );
}
