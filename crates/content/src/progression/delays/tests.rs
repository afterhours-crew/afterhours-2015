// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;

const STABLE: u32 = 77;
fn structure(rows: &[(&str, Value)]) -> Value {
    Value::Struct(Fields(
        rows.iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    ))
}
fn enumeration(name: &str) -> Value {
    Value::Enum {
        value: 0,
        name: Some(name.into()),
    }
}
fn event(source: usize, output: u32, target: usize, input: u32) -> Value {
    structure(&[
        ("Source", local(source)),
        ("Target", local(target)),
        ("SourceEvent", structure(&[("Id", Value::UInt32(output))])),
        ("TargetEvent", structure(&[("Id", Value::UInt32(input))])),
        (
            "TargetType",
            enumeration("EventConnectionTargetType_Server"),
        ),
    ])
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=7).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), STABLE, local(3), 0),
                    edge(local(4), STABLE, local(6), 0),
                ]),
            ),
            ("PropertyConnections", Value::Array(vec![])),
            (
                "EventConnections",
                Value::Array(vec![
                    event(1, ACTIVATED, 2, IN),
                    event(2, OUT, 7, IN),
                    event(7, OUT, 1, COMPLETE),
                    event(4, ACTIVATED, 5, IN),
                    event(5, OUT, 4, COMPLETE),
                ]),
            ),
        ],
    );
    for (i, seconds) in [(2, 2.25), (5, 0.), (7, 0.5)] {
        m.put(
            "a",
            i,
            "DelayEntityData",
            &[
                ("Delay", Value::Float32(seconds)),
                ("Realm", enumeration("Realm_Server")),
                ("TimeDeltaType", enumeration("TimeDeltaType_World")),
                ("AutoStart", Value::Bool(false)),
                ("RunOnce", Value::Bool(false)),
                ("RemoveDuplicateEvents", Value::Bool(true)),
            ],
        );
    }
    for (node, key, id) in [(1, 3, 100), (4, 6, 200)] {
        m.put("a", node, OBJECTIVE, &[]);
        m.put(
            "a",
            key,
            "StableUniqueIdEntityData",
            &[("StableUniqueId64", Value::UInt64(id))],
        );
    }
    m
}
fn fields(m: &mut Memory, node: usize) -> &mut Vec<(String, Value)> {
    &mut m.0.get_mut(&("a".into(), node)).unwrap().fields.0
}
fn set(m: &mut Memory, node: usize, key: &str, value: Value) {
    fields(m, node)
        .iter_mut()
        .find(|(k, _)| k == key)
        .unwrap()
        .1 = value;
}
fn events(m: &mut Memory) -> &mut Vec<Value> {
    let Value::Array(rows) = &mut fields(m, 0)
        .iter_mut()
        .find(|(k, _)| k == "EventConnections")
        .unwrap()
        .1
    else {
        panic!()
    };
    rows
}

#[test]
fn selected_objectives_keep_independent_chains_and_fractional_world_time() {
    let all = read(&mut fixture(), "a", STABLE, &BTreeSet::from([100, 200])).unwrap();
    assert_eq!(
        all,
        vec![
            Delay {
                objective: 100,
                duration: Duration::from_millis(2750)
            },
            Delay {
                objective: 200,
                duration: Duration::ZERO
            }
        ]
    );
    let mut m = fixture();
    // An unselected race result is not interpreted as a timer.
    events(&mut m)[4] = event(5, 99, 4, COMPLETE);
    assert_eq!(
        read(&mut m, "a", STABLE, &BTreeSet::from([100])).unwrap(),
        all[..1]
    );
    assert!(read(&mut m, "a", STABLE, &BTreeSet::from([200])).is_err());
    assert!(read(&mut m, "a", STABLE, &BTreeSet::from([300])).is_err());
}

#[test]
fn timer_does_not_ignore_gates_other_objectives_resets_or_client_delivery() {
    for replacement in [
        event(4, ACTIVATED, 2, IN),
        event(7, OUT, 2, IN),
        event(1, COMPLETE, 2, IN),
    ] {
        let mut m = fixture();
        events(&mut m)[0] = replacement;
        assert!(read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err());
    }
    for extra in [
        event(4, ACTIVATED, 2, IN),
        event(1, ACTIVATED, 2, 123),
        event(4, ACTIVATED, 1, COMPLETE),
    ] {
        let mut m = fixture();
        events(&mut m).push(extra);
        assert!(read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err());
    }
    for index in [0, 1, 2] {
        let mut m = fixture();
        let Value::Struct(row) = &mut events(&mut m)[index] else {
            panic!()
        };
        row.0.iter_mut().find(|(k, _)| k == "TargetType").unwrap().1 =
            enumeration("EventConnectionTargetType_Client");
        assert!(read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err());
    }
    let mut m = fixture();
    m.0.get_mut(&("a".into(), 2)).unwrap().class = "EventGateEntityData".into();
    assert!(read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err());
}

#[test]
fn unsupported_clock_policy_dynamic_duration_and_bounds_fail_closed() {
    for (key, value) in [
        ("Realm", enumeration("Realm_Client")),
        ("TimeDeltaType", enumeration("TimeDeltaType_Real")),
        ("AutoStart", Value::Bool(true)),
        ("RunOnce", Value::Bool(true)),
        ("RemoveDuplicateEvents", Value::Bool(false)),
        ("Delay", Value::Float32(f32::NAN)),
        ("Delay", Value::Float32(-1.)),
        ("Delay", Value::Float32(601.)),
    ] {
        let mut m = fixture();
        set(&mut m, 2, key, value);
        assert!(
            read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err(),
            "{key}"
        );
    }
    let mut m = fixture();
    set(
        &mut m,
        0,
        "PropertyConnections",
        Value::Array(vec![edge(local(1), 99, local(2), 101)]),
    );
    assert!(read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err());
    for ids in [BTreeSet::new(), BTreeSet::from([0]), (1..=65).collect()] {
        assert!(read(&mut fixture(), "a", STABLE, &ids).is_err());
    }
    let mut m = fixture();
    set(&mut m, 6, "StableUniqueId64", Value::UInt64(100));
    assert!(read(&mut m, "a", STABLE, &BTreeSet::from([100])).is_err());
}
