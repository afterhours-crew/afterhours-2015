// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;
const STABLE: u32 = 77;
const CHANNEL: u32 = 4_117_124_221;
fn en(n: &str) -> Value {
    Value::Enum {
        value: 0,
        name: Some(n.into()),
    }
}
fn ev(s: usize, sp: u32, t: usize, tp: u32) -> Value {
    Value::Struct(Fields(vec![
        ("Source".into(), local(s)),
        ("Target".into(), local(t)),
        (
            "SourceEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(sp))])),
        ),
        (
            "TargetEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(tp))])),
        ),
        ("TargetType".into(), en("EventConnectionTargetType_Server")),
    ]))
}
fn put(m: &mut Memory, i: usize, class: &str, fields: &[(&str, Value)]) {
    let mut fields = fields.to_vec();
    fields.push(("Realm", en("Realm_Server")));
    m.put("a", i, class, &fields);
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    let props = vec![
        edge(local(14), OUT, local(3), SEND),
        edge(local(11), VALUE, local(4), SEND),
        edge(local(12), VALUE, local(5), SEND),
        edge(local(10), VALUE, local(14), OR_INPUTS[0]),
        edge(local(15), BOOL_OUT, local(14), OR_INPUTS[1]),
        edge(local(1), ACTIVE, local(15), BOOL_IN),
        edge(local(1), ACTIVE, local(17), BOOL),
        edge(local(1), ACTIVE, local(31), BOOL),
        edge(local(13), VALUE, local(20), BOOL),
        edge(local(13), VALUE, local(21), BOOL),
        edge(local(28), RECOMMENDED, local(27), BOOL),
    ];
    let events = vec![
        ev(17, TRUE, 16, IN),
        ev(16, OUT, 15, OPEN),
        ev(18, OUT, 10, SET_TRUE),
        ev(19, OUT, 11, SET_TRUE),
        ev(20, TRUE, 18, OR_INPUTS[1]),
        ev(21, TRUE, 19, OR_INPUTS[1]),
        ev(22, MATCHED, 18, OR_INPUTS[0]),
        ev(23, MATCHED, 19, OR_INPUTS[0]),
        ev(28, COMPLETE, 27, IN),
        ev(27, TRUE, 12, SET_TRUE),
        ev(27, FALSE, 13, SET_TRUE),
        ev(30, OUT, 29, IN),
        ev(29, OUT, 13, SET_TRUE),
        ev(27, TRUE, 29, CLOSE),
        ev(27, FALSE, 29, CLOSE),
        ev(31, TRUE, 28, REQUEST),
        ev(31, TRUE, 30, IN),
        ev(24, REPORTED, 25, OBJECTIVE_COMPLETE),
    ];
    let mut links = vec![
        edge(local(1), STABLE, local(2), 0),
        edge(local(25), STABLE, local(26), 0),
        edge(local(7), CHANNEL, local(8), 0),
        edge(local(28), CONTEXT, local(6), CONTEXT),
        edge(local(24), CONTEXT, local(6), CONTEXT),
        edge(local(28), DETAILS, local(32), 0),
        edge(local(24), FILTERS, local(22), 0),
        edge(local(24), FILTERS, local(23), 0),
    ];
    for s in 3..=5 {
        links.extend([
            edge(local(s), ATTACHMENT, local(1), 0),
            edge(local(s), CONTEXT, local(6), CONTEXT),
            edge(local(s), CHANNEL, local(6), CHANNEL),
            edge(local(9), GROUP, local(s), 0),
        ]);
    }
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=36).map(local).collect())),
            ("PropertyConnections", Value::Array(props)),
            ("EventConnections", Value::Array(events)),
            ("LinkConnections", Value::Array(links)),
        ],
    );
    for i in [1, 25] {
        put(&mut m, i, "RaceObjectiveEntityData", &[]);
    }
    for (i, id) in [(2, 101), (26, 202)] {
        put(
            &mut m,
            i,
            "StableUniqueIdEntityData",
            &[("StableUniqueId64", Value::UInt64(id))],
        );
    }
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
    for (i, class) in [
        (6, "LinkedSchematicChannelReferenceEntityData"),
        (7, "LinkedSchematicChannelEntityData"),
    ] {
        put(&mut m, i, class, &[("ChannelAsset", ptr("c", 0))]);
    }
    put(&mut m, 8, "MessageManagerEntityData", &[]);
    put(&mut m, 9, "MessageRestrictorEntityData", &[]);
    for i in 10..=13 {
        put(
            &mut m,
            i,
            "BoolEntityData",
            &[("DefaultValue", Value::Bool(false))],
        );
    }
    put(
        &mut m,
        14,
        "OrEntityData",
        &[("InputCount", Value::UInt32(2))],
    );
    put(
        &mut m,
        15,
        "PropertyGateEntityData",
        &[
            ("Default", Value::Bool(false)),
            ("WritePropertyOnOpenGate", Value::Bool(true)),
        ],
    );
    for (i, seconds) in [(16, 7.), (30, 2.)] {
        put(
            &mut m,
            i,
            "DelayEntityData",
            &[
                ("Delay", Value::Float32(seconds)),
                ("TimeDeltaType", en("TimeDeltaType_World")),
                ("AutoStart", Value::Bool(false)),
                ("RunOnce", Value::Bool(false)),
                ("RemoveDuplicateEvents", Value::Bool(true)),
            ],
        );
    }
    for i in [17, 20, 21, 27, 31] {
        put(
            &mut m,
            i,
            "CompareBoolEntityData",
            &[
                ("Bool", Value::Bool(false)),
                ("TriggerOnPropertyChange", Value::Bool(i != 27)),
                ("TriggerOnStart", Value::Bool(false)),
                ("AlwaysSend", Value::Bool(false)),
            ],
        );
    }
    for i in [18, 19] {
        put(
            &mut m,
            i,
            "EventAndGateEntityData",
            &[("EventCount", Value::UInt32(2))],
        );
    }
    for (i, op) in [(22, "CompareOp_Equal"), (23, "CompareOp_NotEqual")] {
        put(
            &mut m,
            i,
            "ScoreResultFilterEntityData",
            &[
                ("CompareOperation", en(op)),
                ("Score", Value::Int32(1)),
                ("ScoreId", ptr("p", 0)),
            ],
        );
    }
    m.put(
        "p",
        0,
        "ScoreTag",
        &[("Name", Value::String(POSITION.into()))],
    );
    put(&mut m, 24, "ParticipantResultReporterEntityData", &[]);
    put(
        &mut m,
        28,
        "ServerIsEventRecommendedCheckerEntityData",
        &[("EventMetaData", local(34))],
    );
    put(
        &mut m,
        29,
        "EventGateEntityData",
        &[("Default", Value::Bool(true))],
    );
    put(
        &mut m,
        32,
        "DetailsRefererEntityData",
        &[
            ("Id", Value::Pointer(Pointer::Null)),
            ("Categories", Value::Array(vec![local(33)])),
        ],
    );
    put(
        &mut m,
        33,
        "GameModeCategory",
        &[("CategoryHash", Value::UInt32(123))],
    );
    put(
        &mut m,
        34,
        "EventMetaDataCollection",
        &[("Events", Value::Array(vec![local(35)]))],
    );
    put(
        &mut m,
        35,
        "EventMetaData",
        &[
            ("Categories", Value::Array(vec![local(33)])),
            ("Id", local(36)),
        ],
    );
    put(&mut m, 36, "GameModeId", &[("IdHash", Value::UInt32(987))]);
    m.put(
        "z",
        0,
        "NarrativeToStandardEventMapAsset",
        &[("Pairs", Value::Array(vec![]))],
    );
    m
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
fn field(m: &mut Memory, i: usize, name: &str, v: Value) {
    m.0.get_mut(&("a".into(), i))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == name)
        .unwrap()
        .1 = v;
}
fn compile(m: &mut Memory) -> Result<Invitation, Error> {
    let mut g = Graph::new(m);
    let map = crate::events::recommendations::read(&mut g, &Site::root("z"))?;
    let nodes = g.read(Site::root("a"))?;
    read(
        &mut g,
        &nodes,
        &nodes,
        &BTreeMap::from([(("a".into(), 8), 4)]),
        STABLE,
        ["l", "m", "h"],
        &map,
    )
}

fn nested() -> Memory {
    let mut m = fixture();
    let moved = BTreeSet::from([12, 13, 27, 28, 29, 30, 31]);
    for &i in &moved {
        let object = m.0[&("a".into(), i)].clone();
        m.0.insert(("f".into(), i), object);
    }
    let field = &mut m.0.get_mut(&("f".into(), 28)).unwrap().fields.0;
    field
        .iter_mut()
        .find(|(n, _)| n == "EventMetaData")
        .unwrap()
        .1 = ptr("a", 34);
    let index = |v: &Value| match v.as_pointer().unwrap() {
        Pointer::Internal(i) => i,
        _ => panic!(),
    };
    let mut parent_properties = Vec::new();
    let mut child_properties = Vec::new();
    for row in rows(&mut m, "PropertyConnections").drain(..) {
        let f = row.as_fields().unwrap();
        let s = index(f.get("Source").unwrap());
        let t = index(f.get("Target").unwrap());
        if moved.contains(&s) && moved.contains(&t) {
            child_properties.push(row);
        } else if s == 12 || s == 13 {
            parent_properties.push(edge(
                local(37),
                if s == 12 { 700 } else { 701 },
                local(t),
                bits32(f.get("TargetFieldId").unwrap()).unwrap(),
            ));
        } else if t == 31 {
            parent_properties.push(edge(local(s), ACTIVE, local(37), ACTIVE));
        } else {
            parent_properties.push(row);
        }
    }
    child_properties.extend([
        edge(local(12), VALUE, local(38), 700),
        edge(local(13), VALUE, local(38), 701),
        edge(local(38), ACTIVE, local(31), BOOL),
    ]);
    *rows(&mut m, "PropertyConnections") = parent_properties;
    let mut parent_events = Vec::new();
    let mut child_events = Vec::new();
    for row in rows(&mut m, "EventConnections").drain(..) {
        let f = row.as_fields().unwrap();
        if moved.contains(&index(f.get("Target").unwrap())) {
            child_events.push(row);
        } else {
            parent_events.push(row);
        }
    }
    child_events.extend([ev(38, REQUEST, 28, REQUEST), ev(38, REQUEST, 30, IN)]);
    *rows(&mut m, "EventConnections") = parent_events;
    for row in rows(&mut m, "LinkConnections") {
        let Value::Struct(f) = row else { panic!() };
        if index(f.get("Source").unwrap()) == 28 {
            f.0.iter_mut().find(|(n, _)| n == "Source").unwrap().1 = local(37);
        }
    }
    *rows(&mut m, "Objects") = (1..=37).filter(|i| !moved.contains(i)).map(local).collect();
    put(
        &mut m,
        37,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("f", 0))],
    );
    m.put("f", 38, "InterfaceDescriptorData", &[]);
    m.put(
        "f",
        0,
        "LogicPrefabBlueprint",
        &[
            (
                "Objects",
                Value::Array(moved.into_iter().map(local).collect()),
            ),
            ("Interface", local(38)),
            ("PropertyConnections", Value::Array(child_properties)),
            ("EventConnections", Value::Array(child_events)),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(28), CONTEXT, local(38), CONTEXT),
                    edge(local(28), DETAILS, local(38), DETAILS),
                ]),
            ),
        ],
    );
    m
}

#[test]
fn prefab_request_interface_is_grounded_and_a_new_caller_cannot_be_ignored() {
    assert_eq!(
        compile(&mut nested()).unwrap(),
        compile(&mut fixture()).unwrap()
    );
    let mut m = nested();
    rows(&mut m, "EventConnections").push(ev(17, TRUE, 37, REQUEST));
    assert!(compile(&mut m).is_err());
    let mut m = nested();
    let Value::Array(events) = &mut m
        .0
        .get_mut(&("f".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "EventConnections")
        .unwrap()
        .1
    else {
        panic!()
    };
    events.push(ev(38, REQUEST, 28, REQUEST));
    assert!(compile(&mut m).is_err());
}
#[test]
fn whole_result_group_resolves_source_objective_category_key_and_delays() {
    let plan = compile(&mut fixture()).unwrap();
    assert_eq!(
        plan,
        Invitation::new(
            101,
            202,
            4,
            [71, 72, 73],
            987,
            Duration::from_secs(2),
            Duration::from_secs(7)
        )
        .unwrap()
    );
    let mut m = fixture();
    for table in ["PropertyConnections", "EventConnections", "LinkConnections"] {
        rows(&mut m, table).reverse();
    }
    field(&mut m, 30, "Delay", Value::Float32(3.));
    assert_eq!(
        compile(&mut m).unwrap().recommendation_after(),
        Duration::from_secs(3)
    );
}
#[test]
fn every_required_connection_is_checked_and_duplicate_events_do_not_vanish() {
    for table in ["PropertyConnections", "EventConnections", "LinkConnections"] {
        let count = rows(&mut fixture(), table).len();
        for i in 0..count {
            let mut m = fixture();
            rows(&mut m, table).remove(i);
            assert!(compile(&mut m).is_err(), "{table}:{i}");
        }
    }
    for i in 0..rows(&mut fixture(), "EventConnections").len() {
        let mut m = fixture();
        let row = rows(&mut m, "EventConnections")[i].clone();
        rows(&mut m, "EventConnections").push(row);
        assert!(compile(&mut m).is_err(), "duplicate {i}");
    }
}
#[test]
fn alternate_controls_reversed_result_and_foreign_recommendations_fail() {
    for (target, input) in [
        (10, IN),
        (12, IN),
        (13, CLOSE),
        (18, CLOSE),
        (22, IN),
        (28, IN),
        (29, OPEN),
        (30, CLOSE),
        (32, IN),
    ] {
        let mut m = fixture();
        rows(&mut m, "EventConnections").push(ev(31, TRUE, target, input));
        assert!(compile(&mut m).is_err(), "{target}");
    }
    for (i, n, v) in [
        (18, "EventCount", Value::UInt32(3)),
        (22, "Score", Value::Int32(0)),
        (23, "CompareOperation", en("CompareOp_Equal")),
        (27, "AlwaysSend", Value::Bool(true)),
        (30, "TimeDeltaType", en("TimeDeltaType_Real")),
        (13, "DefaultValue", Value::Bool(true)),
        (29, "Default", Value::Bool(false)),
        (32, "Categories", Value::Array(vec![local(33), local(33)])),
    ] {
        let mut m = fixture();
        field(&mut m, i, n, v);
        assert!(compile(&mut m).is_err(), "{i}:{n}");
    }
    let mut m = fixture();
    rows(&mut m, "LinkConnections")[3] = edge(local(28), CONTEXT, local(7), CONTEXT);
    assert!(compile(&mut m).is_err());
    let mut m = fixture();
    m.put(
        "p",
        0,
        "ScoreTag",
        &[("Name", Value::String("unrelated/score".into()))],
    );
    assert!(compile(&mut m).is_err());
}
