// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

const STABLE: u32 = 77;
const CHANNEL: u32 = 4_117_124_221;
fn en(name: &str) -> Value {
    Value::Enum {
        value: 0,
        name: Some(name.into()),
    }
}
fn event_row(from: usize, output: u32, to: usize, input: u32) -> Value {
    Value::Struct(Fields(vec![
        ("Source".into(), local(from)),
        ("Target".into(), local(to)),
        (
            "SourceEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(output))])),
        ),
        (
            "TargetEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(input))])),
        ),
        ("TargetType".into(), en("EventConnectionTargetType_Server")),
    ]))
}
fn put(m: &mut Memory, i: usize, name: &str, fields: &[(&str, Value)]) {
    let mut f = fields.to_vec();
    f.push(("Realm", en("Realm_Server")));
    m.put("a", i, name, &f);
}
fn compare(m: &mut Memory, i: usize) {
    put(
        m,
        i,
        "CompareBoolEntityData",
        &[
            ("Bool", Value::Bool(false)),
            ("TriggerOnPropertyChange", Value::Bool(true)),
            ("TriggerOnStart", Value::Bool(false)),
            ("AlwaysSend", Value::Bool(false)),
        ],
    );
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    let mut props = vec![
        edge(local(32), OUT, local(29), BOOL),
        edge(local(33), OUT, local(30), BOOL),
        edge(local(34), OUT, local(31), BOOL),
        edge(local(11), VALUE, local(32), OR_INPUTS[0]),
        edge(local(12), VALUE, local(32), OR_INPUTS[1]),
        edge(local(10), VALUE, local(33), OR_INPUTS[0]),
        edge(local(12), VALUE, local(33), OR_INPUTS[1]),
        edge(local(10), VALUE, local(34), OR_INPUTS[0]),
        edge(local(11), VALUE, local(34), OR_INPUTS[1]),
        edge(local(25), OUT, local(3), SEND),
        edge(local(11), VALUE, local(4), SEND),
        edge(local(12), VALUE, local(5), SEND),
        edge(local(10), VALUE, local(25), OR_INPUTS[0]),
        edge(local(26), BOOL_OUT, local(25), OR_INPUTS[1]),
        edge(local(1), ACTIVE, local(26), BOOL_IN),
        edge(local(1), ACTIVE, local(28), BOOL),
        edge(local(1), ACTIVE, local(24), BOOL),
        edge(local(19), CURRENT, local(18), INT_IN),
    ];
    for (c, math, number) in [(16, 20, 22), (17, 21, 23)] {
        props.extend([
            edge(local(18), INT_OUT, local(c), A),
            edge(local(math), OUT, local(c), B),
            edge(local(19), STOCK, local(math), INPUTS[0]),
            edge(local(number), VALUE, local(math), INPUTS[1]),
        ]);
    }
    let mut events = vec![
        event_row(24, TRUE, 18, OPEN),
        event_row(16, LESS, 13, IN),
        event_row(16, GE, 17, IN),
        event_row(17, LESS, 14, IN),
        event_row(17, GE, 15, IN),
        event_row(28, TRUE, 27, IN),
        event_row(27, OUT, 26, OPEN),
    ];
    for (gate, value, closer) in [(13, 10, 29), (14, 11, 30), (15, 12, 31)] {
        events.extend([
            event_row(gate, OUT, value, SET_TRUE),
            event_row(closer, TRUE, gate, CLOSE),
        ]);
    }
    let mut links = vec![
        edge(local(1), STABLE, local(2), 0),
        edge(local(7), CHANNEL, local(8), 0),
        edge(local(19), CONTEXT, local(6), CONTEXT),
    ];
    for sender in 3..=5 {
        links.extend([
            edge(local(sender), ATTACHMENT, local(1), 0),
            edge(local(sender), CONTEXT, local(6), CONTEXT),
            edge(local(sender), CHANNEL, local(6), CHANNEL),
            edge(local(9), GROUP, local(sender), 0),
        ]);
    }
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=34).map(local).collect())),
            ("PropertyConnections", Value::Array(props)),
            ("LinkConnections", Value::Array(links)),
            ("EventConnections", Value::Array(events)),
        ],
    );
    put(&mut m, 1, "RaceObjectiveEntityData", &[]);
    put(
        &mut m,
        2,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(101))],
    );
    for (i, name, hash) in [(3, "l", 71), (4, "m", 72), (5, "h", 73)] {
        put(
            &mut m,
            i,
            "UniqueMessageSenderEntityData",
            &[
                ("MessageAsset", ptr(name, 0)),
                ("HasMessageRestrictor", Value::Bool(true)),
                ("SendMessage", Value::Bool(false)),
            ],
        );
        m.put(
            name,
            0,
            "MessageAsset",
            &[
                ("HashId", Value::UInt32(hash)),
                ("Persisted", Value::Bool(true)),
            ],
        );
    }
    m.put("c", 0, "ChannelAsset", &[]);
    put(
        &mut m,
        6,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    put(
        &mut m,
        7,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    put(&mut m, 8, "MessageManagerEntityData", &[]);
    put(&mut m, 9, "MessageRestrictorEntityData", &[]);
    for i in 10..=12 {
        put(
            &mut m,
            i,
            "BoolEntityData",
            &[("DefaultValue", Value::Bool(false))],
        );
    }
    for i in 13..=15 {
        put(
            &mut m,
            i,
            "EventGateEntityData",
            &[("Default", Value::Bool(true))],
        );
    }
    for i in [16, 17] {
        put(
            &mut m,
            i,
            "CompareIntEntityData",
            &[
                ("A", Value::Int32(0)),
                ("B", Value::Int32(0)),
                ("TriggerOnPropertyChange", Value::Bool(i == 16)),
                ("TriggerOnStart", Value::Bool(false)),
            ],
        );
    }
    for i in [18, 26] {
        put(
            &mut m,
            i,
            "PropertyGateEntityData",
            &[
                ("Default", Value::Bool(false)),
                ("WritePropertyOnOpenGate", Value::Bool(true)),
            ],
        );
    }
    put(&mut m, 19, "VehicleStatsProviderEntityData", &[]);
    for i in [20, 21] {
        put(
            &mut m,
            i,
            "MathIntOpEntityData",
            &[("Operators", Value::Array(vec![en("IntMathOp_Add")]))],
        );
    }
    for (i, n) in [(22, 10), (23, 30)] {
        put(
            &mut m,
            i,
            "IntEntityData",
            &[("DefaultValue", Value::Int32(n))],
        );
    }
    for i in [24, 28, 29, 30, 31] {
        compare(&mut m, i);
    }
    for i in [25, 32, 33, 34] {
        put(
            &mut m,
            i,
            "OrEntityData",
            &[("InputCount", Value::UInt32(2))],
        );
    }
    put(
        &mut m,
        27,
        "DelayEntityData",
        &[
            ("Delay", Value::Float32(7.)),
            ("TimeDeltaType", en("TimeDeltaType_World")),
            ("AutoStart", Value::Bool(false)),
            ("RunOnce", Value::Bool(false)),
            ("RemoveDuplicateEvents", Value::Bool(true)),
        ],
    );
    m
}
fn field(m: &mut Memory, i: usize, name: &str, value: Value) {
    m.0.get_mut(&("a".into(), i))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == name)
        .unwrap()
        .1 = value;
}
fn rows<'a>(m: &'a mut Memory, name: &str) -> &'a mut Vec<Value> {
    let v = &mut m
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == name)
        .unwrap()
        .1;
    let Value::Array(rows) = v else { panic!() };
    rows
}
fn compile(m: &mut Memory) -> Result<Invitation, Error> {
    let mut g = Graph::new(m);
    let nodes = g.read(Site::root("a"))?;
    read(
        &mut g,
        &nodes,
        &nodes,
        &BTreeMap::from([(("a".into(), 8), 4)]),
        STABLE,
        ["l", "m", "h"],
    )
}
#[test]
fn exclusive_power_group_preserves_boundaries_and_authored_default_delay() {
    let c = compile(&mut fixture()).unwrap();
    assert_eq!(
        c,
        Invitation::new(101, 4, [71, 72, 73], [10, 30], Duration::from_secs(7)).unwrap()
    );
    for (current, want) in [
        (99, 71),
        (109, 71),
        (110, 72),
        (129, 72),
        (130, 73),
        (1500, 73),
    ] {
        assert_eq!(c.choose(current, 100).unwrap(), want);
    }
    assert!(c.choose(1501, 100).is_err());
    assert!(c.choose(100, 1501).is_err());
    // A different authored threshold pair and connection order use the same
    // mechanism. No mission/object index or low-tier delay is hardcoded.
    let mut m = fixture();
    field(&mut m, 22, "DefaultValue", Value::Int32(40));
    field(&mut m, 23, "DefaultValue", Value::Int32(80));
    rows(&mut m, "EventConnections").reverse();
    rows(&mut m, "PropertyConnections").reverse();
    let c = compile(&mut m).unwrap();
    assert_eq!(c.choose(139, 100).unwrap(), 71);
    assert_eq!(c.choose(140, 100).unwrap(), 72);
}
#[test]
fn alternate_event_controls_and_unchecked_tiers_cannot_be_discarded() {
    for (target, input) in [
        (10, IN),
        (13, OPEN),
        (16, IN),
        (18, CLOSE),
        (22, IN),
        (27, CLOSE),
        (3, IN),
        (9, IN),
        (19, IN),
    ] {
        let mut m = fixture();
        rows(&mut m, "EventConnections").push(event_row(24, TRUE, target, input));
        assert!(compile(&mut m).is_err(), "target{target}");
    }
    for (i, field_name, value) in [
        (10, "DefaultValue", Value::Bool(true)),
        (13, "Default", Value::Bool(false)),
        (17, "TriggerOnPropertyChange", Value::Bool(true)),
        (18, "WritePropertyOnOpenGate", Value::Bool(false)),
        (23, "DefaultValue", Value::Int32(-1)),
        (27, "Delay", Value::Float32(f32::NAN)),
        (27, "RemoveDuplicateEvents", Value::Bool(false)),
        (31, "TriggerOnStart", Value::Bool(true)),
    ] {
        let mut m = fixture();
        field(&mut m, i, field_name, value);
        assert!(compile(&mut m).is_err(), "{i}/{field_name}");
    }
    let mut m = fixture();
    rows(&mut m, "EventConnections")[0] = event_row(24, TRUE, 18, OPEN);
    let Value::Struct(f) = &mut rows(&mut m, "EventConnections")[0] else {
        panic!()
    };
    f.0.iter_mut().find(|(n, _)| n == "TargetType").unwrap().1 =
        en("EventConnectionTargetType_Client");
    assert!(compile(&mut m).is_err());
}
#[test]
fn missing_or_ambiguous_members_and_foreign_providers_fail() {
    for table in ["EventConnections", "PropertyConnections", "LinkConnections"] {
        let count = rows(&mut fixture(), table).len();
        for index in 0..count {
            let mut m = fixture();
            rows(&mut m, table).remove(index);
            assert!(compile(&mut m).is_err(), "missing {table}:{index}");
        }
    }
    let mut m = fixture();
    rows(&mut m, "LinkConnections").push(edge(local(9), GROUP, local(3), 0));
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    let original = rows(&mut m, "EventConnections")[0].clone();
    rows(&mut m, "EventConnections").push(original);
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    rows(&mut m, "PropertyConnections").push(edge(local(19), CURRENT, local(18), INT_IN));
    assert!(compile(&mut m).is_err());
}

#[test]
fn altered_comparisons_closures_realms_and_boolean_slots_are_rejected() {
    let mut m = fixture();
    rows(&mut m, "EventConnections")[1] = event_row(16, GE, 13, IN);
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    rows(&mut m, "PropertyConnections")[3] = edge(local(10), VALUE, local(32), OR_INPUTS[0]);
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    rows(&mut m, "PropertyConnections")[3] = edge(local(11), VALUE, local(32), 12345);
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    field(&mut m, 19, "Realm", en("Realm_Client"));
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    rows(&mut m, "LinkConnections")[2] = edge(local(19), CONTEXT, local(7), CONTEXT);
    assert!(compile(&mut m).is_err());
}
