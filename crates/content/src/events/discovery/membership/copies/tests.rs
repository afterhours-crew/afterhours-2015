// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::{
    Frame,
    tests::{Memory, edge as link, local, ptr},
};
use nfs_frostbite::ebx::{Fields, Pointer};

fn structure(fields: &[(&str, Value)]) -> Value {
    Value::Struct(Fields(
        fields
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    ))
}
fn realm() -> Value {
    Value::Enum {
        value: 1,
        name: Some("Realm_Server".into()),
    }
}
fn event(from: usize, out: u32, to: usize, input: u32) -> Value {
    structure(&[
        ("Source", local(from)),
        ("Target", local(to)),
        ("SourceEvent", structure(&[("Id", Value::UInt32(out))])),
        ("TargetEvent", structure(&[("Id", Value::UInt32(input))])),
        (
            "TargetType",
            Value::Enum {
                value: 3,
                name: Some("EventConnectionTargetType_Server".into()),
            },
        ),
    ])
}
fn set(m: &mut Memory, asset: &str, index: usize, field: &str, value: Value) {
    let f = &mut m.0.get_mut(&(asset.into(), index)).unwrap().fields.0;
    if let Some((_, v)) = f.iter_mut().find(|(k, _)| k == field) {
        *v = value;
    } else {
        f.push((field.into(), value));
    }
}
fn rows(m: &mut Memory, asset: &str, field: &str) -> Vec<Value> {
    m.0[&(asset.into(), 0)]
        .fields
        .get(field)
        .unwrap()
        .as_array()
        .unwrap()
        .to_vec()
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    // Deliberately use different indices and interface ports from installation
    // data. Roles and occurrence identities must come from wiring.
    m.put(
        "b",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((2..=7).map(local).collect())),
            ("Interface", local(1)),
            ("Flags", Value::UInt16(0)),
            ("PropertyConnections", Value::Array(vec![])),
            (
                "LinkConnections",
                Value::Array(vec![
                    link(local(4), name("ParticipantSet"), local(2), 0),
                    link(local(4), (-1_232_400_965i32) as u32, local(3), 0),
                    link(local(5), name("ParticipantSet"), local(1), 101),
                    link(local(2), name("ParticipantSet"), local(1), 101),
                    link(local(1), 102, local(2), 0),
                ]),
            ),
            (
                "EventConnections",
                Value::Array(vec![
                    event(5, (-414_835_110i32) as u32, 6, nodes::IN),
                    event(6, nodes::OUT, 3, nodes::IN),
                    event(6, nodes::OUT, 4, (-1_657_392_005i32) as u32),
                    event(5, (-2_103_682_472i32) as u32, 7, nodes::IN),
                    event(7, nodes::OUT, 3, nodes::IN),
                    event(7, nodes::OUT, 4, (-772_017_604i32) as u32),
                    event(1, 103, 2, 0x7c81_3ec0),
                    event(1, 103, 7, nodes::OPEN),
                    event(1, 103, 6, nodes::OPEN),
                    event(1, 104, 7, nodes::CLOSE),
                    event(1, 104, 6, nodes::CLOSE),
                    event(1, 104, 2, 0x0cac_9f9c),
                ]),
            ),
        ],
    );
    for (i, class) in [
        (1, "InterfaceDescriptorData"),
        (2, "ParticipantSetCopyEntityData"),
        (3, "ParticipantEventToParticipantProviderEntityData"),
        (4, "ParticipantSetAccessorEntityData"),
        (5, "ParticipantSetListenerEntityData"),
        (6, "EventGateEntityData"),
        (7, "EventGateEntityData"),
    ] {
        m.put(
            "b",
            i,
            class,
            &[
                ("Flags", Value::UInt32((2 << 25) | (2 << 27))),
                ("Realm", realm()),
                ("Default", Value::Bool(false)),
            ],
        );
    }
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=8).map(local).collect())),
            ("Interface", Value::Pointer(Pointer::Null)),
            ("Flags", Value::UInt16(0)),
            (
                "LinkConnections",
                Value::Array(vec![
                    link(local(1), 101, local(5), 0),
                    link(local(4), name("0"), local(1), 102),
                    link(local(5), name("ParticipantManager"), local(6), 0),
                ]),
            ),
            (
                "PropertyConnections",
                Value::Array(vec![link(local(3), 301, local(2), nodes::BOOL)]),
            ),
            (
                "EventConnections",
                Value::Array(vec![
                    event(2, nodes::ON_TRUE, 1, 103),
                    event(2, nodes::ON_FALSE, 1, 104),
                ]),
            ),
        ],
    );
    m.put(
        "a",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    m.put(
        "a",
        2,
        "CompareBoolEntityData",
        &[
            ("Flags", Value::UInt32(2 << 27)),
            ("Realm", realm()),
            ("Bool", Value::Bool(false)),
            ("TriggerOnPropertyChange", Value::Bool(true)),
            ("TriggerOnStart", Value::Bool(false)),
            ("AlwaysSend", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        3,
        "SchematicChannelEntityData",
        &[
            ("Channel", ptr("c", 0)),
            ("Realm", realm()),
            ("OutputProperties", Value::Array(vec![Value::UInt32(0)])),
        ],
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Properties",
            Value::Array(vec![structure(&[
                ("Id", Value::UInt32(301)),
                ("Realm", realm()),
            ])]),
        )],
    );
    m.put(
        "a",
        4,
        "ParticipantSetPredicateEntityData",
        &[("NumberOfOperands", Value::UInt32(2))],
    );
    m.put("a", 5, "ParticipantSetPlayerAdapterEntityData", &[]);
    m.put("a", 6, "ParticipantManagerEntityData", &[]);
    m.put("a", 7, "ParticipantSetAccessorEntityData", &[]);
    m.put("a", 8, "InterfaceDescriptorData", &[]);
    m
}
fn copy() -> Site {
    Site {
        key: ("b".into(), 2),
        bus: ("b".into(), 0),
        parents: vec![Frame {
            bus: ("a".into(), 0),
            reference: ("a".into(), 1),
        }],
    }
}
fn load(m: &mut Memory) -> Result<BTreeMap<Identity, Control>, Error> {
    read(
        &mut Graph::new(m),
        &[Site::root("a")],
        &BTreeSet::from([Identity::from(&copy())]),
        &BTreeMap::new(),
        &mut |_, _| Err(bad("unexpected state")),
    )
}

#[test]
fn controller_uses_authored_ports_and_keeps_the_copy_distinct_from_its_input() {
    let c = load(&mut fixture()).unwrap();
    let c = &c[&Identity::from(&copy())];
    assert_eq!(
        c.channel,
        Property {
            channel: "c".into(),
            port: 301
        }
    );
    assert_eq!(
        c.comparison,
        Identity {
            asset: "a".into(),
            index: 2,
            instances: vec![]
        }
    );
    assert_eq!(
        c.comparison_config.definition,
        nodes::Definition::CompareBoolean {
            on_change: true,
            on_start: false,
            always_send: false
        }
    );
    assert_eq!(
        c.input.facts().collect::<Vec<_>>(),
        vec![&Fact::Player(Identity {
            asset: "a".into(),
            index: 6,
            instances: vec![]
        })]
    );
    assert!(c.input.evaluate(|_| Some(true)).unwrap());
    assert!(c.input.evaluate(|_| None).is_err());
}

#[test]
fn changed_command_order_initial_gate_and_listener_realm_are_rejected() {
    let mut m = fixture();
    let mut events = rows(&mut m, "b", "EventConnections");
    events.swap(1, 2); // Accessor would observe the previous event participant.
    set(&mut m, "b", 0, "EventConnections", Value::Array(events));
    assert!(
        load(&mut m)
            .unwrap_err()
            .to_string()
            .contains("event order")
    );
    let mut m = fixture();
    set(&mut m, "b", 6, "Default", Value::Bool(true));
    assert!(load(&mut m).is_err());
    let mut m = fixture();
    set(&mut m, "b", 4, "Flags", Value::UInt32(1 << 25));
    assert!(
        load(&mut m)
            .unwrap_err()
            .to_string()
            .contains("listener realm")
    );
    let mut m = fixture();
    set(&mut m, "a", 2, "Flags", Value::UInt32(1 << 27));
    assert!(
        load(&mut m)
            .unwrap_err()
            .to_string()
            .contains("comparison realm")
    );
}

#[test]
fn extra_writers_and_unbound_exports_cannot_be_certified_as_a_mirror() {
    for consumer in [7, 8] {
        let mut m = fixture();
        let mut links = rows(&mut m, "a", "LinkConnections");
        links.push(link(local(consumer), name("ParticipantSet"), local(1), 102));
        set(&mut m, "a", 0, "LinkConnections", Value::Array(links));
        assert!(
            load(&mut m)
                .unwrap_err()
                .to_string()
                .contains("external writer")
        );
    }
    let mut m = fixture();
    let mut events = rows(&mut m, "b", "EventConnections");
    events.push(event(1, 105, 2, 0x0cac_9f9c));
    set(&mut m, "b", 0, "EventConnections", Value::Array(events));
    assert!(load(&mut m).is_err());
}

#[test]
fn channel_declarations_and_both_comparison_edges_must_resolve_exactly() {
    let mut m = fixture();
    set(&mut m, "a", 3, "OutputProperties", Value::Array(vec![]));
    assert!(
        load(&mut m)
            .unwrap_err()
            .to_string()
            .contains("output declaration")
    );
    let mut m = fixture();
    set(
        &mut m,
        "a",
        0,
        "EventConnections",
        Value::Array(vec![
            event(2, nodes::ON_TRUE, 1, 103),
            event(2, nodes::ON_TRUE, 1, 104),
        ]),
    );
    assert!(
        load(&mut m)
            .unwrap_err()
            .to_string()
            .contains("comparison outputs")
    );
    let mut m = fixture();
    set(
        &mut m,
        "a",
        0,
        "PropertyConnections",
        Value::Array(vec![
            link(local(3), 301, local(2), nodes::BOOL),
            link(local(3), 301, local(2), 99),
        ]),
    );
    assert!(
        load(&mut m)
            .unwrap_err()
            .to_string()
            .contains("unexpected comparison input")
    );
    let mut m = fixture();
    assert!(
        read(
            &mut Graph::new(&mut m),
            &[],
            &BTreeSet::from([Identity::from(&copy())]),
            &BTreeMap::new(),
            &mut |_, _| Err(bad("unused"))
        )
        .is_err()
    );
}
