// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;
fn event(target: usize, source: u32, port: u32) -> Value {
    Value::Struct(Fields(vec![
        ("Source".into(), local(2)),
        ("Target".into(), local(target)),
        (
            "SourceEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(source))])),
        ),
        (
            "TargetEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::UInt32(port))])),
        ),
        (
            "TargetType".into(),
            Value::Enum {
                value: 3,
                name: Some("EventConnectionTargetType_Server".into()),
            },
        ),
    ]))
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    let provider = (-1_232_400_965i32) as u32;
    let mut property = edge(local(3), scalar::FLOAT_VALUE, local(2), scalar::BOOL);
    let Value::Struct(fields) = &mut property else {
        panic!()
    };
    fields.0.push(("Flags".into(), Value::UInt32(3)));
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array((1..=10).map(local).collect())),
            (
                "EventConnections",
                Value::Array(vec![
                    event(1, scalar::ON_TRUE, 230_748_069),
                    event(4, scalar::ON_FALSE, 283_106_254),
                    event(5, scalar::ON_FALSE, 135_864_506),
                    event(3, scalar::ON_FALSE, scalar::SET_TRUE),
                ]),
            ),
            ("PropertyConnections", Value::Array(vec![property])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), provider, local(7), 0),
                    edge(local(4), provider, local(7), 0),
                    edge(local(5), provider, local(7), 0),
                    edge(local(4), (-621_692_900i32) as u32, local(6), 0),
                ]),
            ),
        ],
    );
    m.put("a", 1, "SimpleGameModeLauncherEntityData", &[]);
    m.put(
        "a",
        2,
        "CompareBoolEntityData",
        &[
            ("Bool", Value::Bool(false)),
            ("TriggerOnStart", Value::Bool(false)),
            ("TriggerOnPropertyChange", Value::Bool(false)),
            ("AlwaysSend", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        3,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(false))],
    );
    m.put(
        "a",
        4,
        "StateSequenceSelectorEntityData",
        &[("OverrideSequenceQueue", Value::Bool(true))],
    );
    m.put("a", 5, "SetTeleportLocationEntityData", &[]);
    m.put("a", 6, "StateSequenceRefererEntityData", &[]);
    m.put("a", 7, "ParticipantLogicParticipantProviderEntityData", &[]);
    // Movie/retry data in this capability must not make this branch ambiguous.
    m.put("a", 8, "StateSequenceRefererEntityData", &[]);
    m.put("a", 9, "SetTeleportLocationEntityData", &[]);
    m.put(
        "a",
        10,
        "ParticipantLogicParticipantProviderEntityData",
        &[],
    );
    m.put("a", 11, "InterfaceDescriptorData", &[]);
    m.0.get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .push(("Interface".into(), local(11)));
    m
}
fn read(m: &mut Memory) -> Result<(Site, Site), Error> {
    let mut graph = Graph::new(m);
    let nodes = graph.read(Site::root("a"))?;
    before_launcher(&mut graph, &nodes)
}
#[test]
fn arrival_follows_the_launchers_latch_instead_of_other_movie_and_retry_refs() {
    let (referer, setter) = read(&mut fixture()).unwrap();
    assert_eq!(referer.key, ("a".into(), 6));
    assert_eq!(setter.key, ("a".into(), 5));
}
#[test]
fn arrival_rejects_unowned_or_nonserver_or_ambiguous_conditional_paths() {
    for case in 0..5 {
        let mut m = fixture();
        match case {
            0 => m.0.get_mut(&("a".into(), 3)).unwrap().fields.0[0].1 = Value::Bool(true),
            1 => m.0.get_mut(&("a".into(), 4)).unwrap().fields.0[0].1 = Value::Bool(false),
            2 => {
                let Value::Array(links) = &mut m.0.get_mut(&("a".into(), 0)).unwrap().fields.0[4].1
                else {
                    panic!()
                };
                links[2] = edge(local(5), (-1_232_400_965i32) as u32, local(10), 0);
            }
            3 => {
                let Value::Array(events) =
                    &mut m.0.get_mut(&("a".into(), 0)).unwrap().fields.0[2].1
                else {
                    panic!()
                };
                events.push(event(9, scalar::ON_FALSE, 135_864_506));
            }
            _ => {
                let Value::Array(events) =
                    &mut m.0.get_mut(&("a".into(), 0)).unwrap().fields.0[2].1
                else {
                    panic!()
                };
                let Value::Struct(e) = &mut events[1] else {
                    panic!()
                };
                e.0[4].1 = Value::Enum {
                    value: 5,
                    name: Some("EventConnectionTargetType_NetworkedClient".into()),
                };
            }
        }
        assert!(read(&mut m).is_err(), "case {case}");
    }
}
