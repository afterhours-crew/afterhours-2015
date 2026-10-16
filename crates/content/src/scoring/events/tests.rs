// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, local, ptr};
use nfs_frostbite::ebx::Pointer;
use nfs_world::scoring::events::{Dispatch, Handler};

fn structure(fields: &[(&str, Value)]) -> Value {
    Value::Struct(Fields(
        fields
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    ))
}
fn edge(source: usize, sp: u32, target: usize, tp: u32, kind: i32) -> Value {
    let name = [
        "Invalid",
        "ClientAndServer",
        "Client",
        "Server",
        "NetworkedClient",
        "NetworkedClientAndServer",
    ][kind as usize];
    structure(&[
        ("Source", local(source)),
        ("Target", local(target)),
        ("SourceEvent", structure(&[("Id", Value::Int32(sp as i32))])),
        ("TargetEvent", structure(&[("Id", Value::UInt32(tp))])),
        (
            "TargetType",
            Value::Enum {
                value: kind,
                name: Some(format!("EventConnectionTargetType_{name}")),
            },
        ),
    ])
}
fn setup() -> Memory {
    let mut memory = Memory::default();
    memory.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array((1..=4).map(local).collect())),
            ("Interface", Value::Pointer(Pointer::Null)),
            ("PropertyConnections", Value::Array(vec![])),
            ("LinkConnections", Value::Array(vec![])),
            (
                "EventConnections",
                Value::Array(vec![
                    edge(3, 10, 1, 11, 3),
                    edge(3, 10, 2, 11, 1),
                    edge(1, 14, 4, 15, 3),
                    edge(2, 14, 4, 16, 3),
                ]),
            ),
        ],
    );
    for i in [1, 2] {
        memory.put(
            "a",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("b", 0))],
        );
    }
    memory.put("a", 3, "Source", &[]);
    memory.put("a", 4, "Sink", &[]);
    memory.put(
        "b",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(3)])),
            ("Interface", local(2)),
            ("PropertyConnections", Value::Array(vec![])),
            ("LinkConnections", Value::Array(vec![])),
            (
                "EventConnections",
                Value::Array(vec![
                    edge(2, 11, 1, 12, 3),
                    edge(1, 13, 3, 99, 2),
                    edge(1, 13, 3, 98, 4),
                    edge(1, 13, 2, 14, 3),
                ]),
            ),
        ],
    );
    memory.put("b", 1, "Leaf", &[]);
    memory.put("b", 2, "InterfaceDescriptorData", &[]);
    memory.put("b", 3, "Presentation", &[]);
    memory
}
#[derive(Clone)]
struct State {
    topology: Composition,
    log: Vec<(u16, u32)>,
}
impl Handler for State {
    fn handle(
        &mut self,
        target: Endpoint,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), nfs_world::scoring::Error> {
        let node = &self.topology.nodes[target.node as usize];
        if let Some(forward) = node.forward {
            dispatch.emit(
                self,
                Endpoint {
                    node: forward,
                    event: target.event,
                },
            )
        } else {
            let leaf = node.class == "Leaf";
            self.log.push((target.node, target.event));
            if leaf {
                dispatch.emit(
                    self,
                    Endpoint {
                        event: 13,
                        ..target
                    },
                )?;
            }
            Ok(())
        }
    }
}
#[test]
fn prefab_roundtrip_is_synchronous_and_repeated_instances_keep_distinct_remote_rows() {
    let composition = read(&mut setup(), "a").unwrap();
    assert_eq!(composition.buses.len(), 3);
    let find = |asset: &str, index, instance: Option<usize>| -> u16 {
        composition
            .nodes
            .iter()
            .position(|node| {
                node.identity.asset == asset
                    && node.identity.index == index
                    && node.identity.instances.last().map(|f| f.1) == instance
            })
            .unwrap() as u16
    };
    let source = find("a", 3, None);
    let sink = find("a", 4, None);
    let first = find("b", 1, Some(1));
    let second = find("b", 1, Some(2));
    assert_ne!(first, second);
    let graph = composition.dispatcher().unwrap();
    let initial = State {
        topology: composition,
        log: vec![],
    };
    let (next, remote) = graph
        .apply(
            &initial,
            Endpoint {
                node: source,
                event: 10,
            },
        )
        .unwrap();
    assert_eq!(
        next.log,
        [(first, 12), (sink, 15), (second, 12), (sink, 16)]
    );
    assert!(initial.log.is_empty());
    assert_eq!(remote.len(), 2);
    assert_eq!(remote[0].connections, [Remote { bus: 1, index: 2 }]);
    assert_eq!(remote[1].connections, [Remote { bus: 2, index: 2 }]);
    assert_eq!(
        graph
            .apply(
                &initial,
                Endpoint {
                    node: source,
                    event: 10
                }
            )
            .unwrap()
            .0
            .log,
        next.log
    );
}
#[test]
fn invalid_realm_and_endpoints_fail_even_on_client_only_rows() {
    for row in [
        edge(3, 10, 4, 12, 0),
        edge(3, 10, 99, 12, 2),
        structure(&[
            ("Source", Value::Pointer(Pointer::Null)),
            ("Target", local(4)),
            ("SourceEvent", structure(&[("Id", Value::Int32(10))])),
            ("TargetEvent", structure(&[("Id", Value::Int32(12))])),
            (
                "TargetType",
                Value::Enum {
                    value: 2,
                    name: Some("EventConnectionTargetType_Client".into()),
                },
            ),
        ]),
    ] {
        let mut memory = setup();
        memory
            .0
            .get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "EventConnections")
            .unwrap()
            .1 = Value::Array(vec![row]);
        assert!(read(&mut memory, "a").is_err());
    }
    // An object can exist in the asset without belonging to this composition.
    let mut memory = setup();
    memory.put("a", 99, "Outside", &[]);
    memory
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "EventConnections")
        .unwrap()
        .1 = Value::Array(vec![edge(3, 10, 99, 12, 2)]);
    assert!(read(&mut memory, "a").is_err());
}
#[test]
fn malformed_prefabs_and_filtered_connection_flood_are_bounded() {
    let mut memory = setup();
    memory
        .0
        .get_mut(&("b".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "Interface")
        .unwrap()
        .1 = Value::Pointer(Pointer::Null);
    assert!(read(&mut memory, "a").is_err());
    let mut memory = setup();
    memory
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "EventConnections")
        .unwrap()
        .1 = Value::Array(vec![edge(3, 10, 4, 12, 2); events::MAX_CONNECTIONS + 1]);
    assert!(
        read(&mut memory, "a")
            .unwrap_err()
            .to_string()
            .contains("bound")
    );
    let mut memory = setup();
    memory.put(
        "b",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("a", 0))],
    );
    assert!(
        read(&mut memory, "a")
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

fn set_data_rows(memory: &mut Memory, asset: &str, field: &str, rows: Vec<Value>) {
    memory
        .0
        .get_mut(&(asset.into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(key, _)| key == field)
        .unwrap()
        .1 = Value::Array(rows);
}

#[test]
fn property_and_link_rows_preserve_instance_boundary_order_and_signed_ids() {
    use crate::graph::tests::edge as data;
    let mut memory = setup();
    set_data_rows(
        &mut memory,
        "a",
        "PropertyConnections",
        vec![
            data(local(3), u32::MAX, local(2), 91),
            data(local(3), u32::MAX, local(1), 91),
        ],
    );
    set_data_rows(
        &mut memory,
        "b",
        "PropertyConnections",
        vec![data(local(2), 91, local(1), 92)],
    );
    set_data_rows(
        &mut memory,
        "b",
        "LinkConnections",
        vec![data(local(1), 93, local(3), 0)],
    );
    let composition = read(&mut memory, "a").unwrap();
    assert_eq!(composition.properties.len(), 4);
    assert_eq!(composition.links.len(), 2);
    let first = composition.properties[0];
    assert_eq!(
        (
            first.bus,
            first.index,
            first.source.field,
            first.target.field
        ),
        (0, 0, u32::MAX, 91)
    );
    assert_eq!(
        composition.nodes[first.target.node as usize].identity.index,
        2
    );
    assert_eq!(
        composition.nodes[composition.properties[1].target.node as usize]
            .identity
            .index,
        1
    );
    let leaf_a = composition.properties[2].target.node;
    let leaf_b = composition.properties[3].target.node;
    assert_ne!(leaf_a, leaf_b);
    assert_eq!(composition.links[0].source.node, leaf_a);
    assert_eq!(composition.links[1].source.node, leaf_b);
    assert_eq!(composition.links[0].target.field, 0);
    assert_eq!(composition.links[1].bus, 2);
    assert_eq!(
        composition.nodes[leaf_a as usize].identity.instances,
        [("a".into(), 1)]
    );
    assert_eq!(
        composition.nodes[leaf_b as usize].identity.instances,
        [("a".into(), 2)]
    );
}

#[test]
fn malformed_data_connections_and_combined_table_bounds_are_rejected() {
    use crate::graph::tests::edge as data;
    for field in ["PropertyConnections", "LinkConnections"] {
        for row in [
            data(Value::Pointer(Pointer::Null), 1, local(4), 2),
            data(local(99), 1, local(4), 2),
            structure(&[
                ("Source", local(3)),
                ("Target", local(4)),
                ("SourceFieldId", Value::Float32(1.0)),
                ("TargetFieldId", Value::Int32(2)),
            ]),
        ] {
            let mut memory = setup();
            memory.put("a", 99, "Outside", &[]);
            set_data_rows(&mut memory, "a", field, vec![row]);
            assert!(read(&mut memory, "a").is_err());
        }
        let mut memory = setup();
        set_data_rows(
            &mut memory,
            "a",
            field,
            vec![data(local(3), 1, local(4), 2); events::MAX_CONNECTIONS],
        );
        // Each table alone fits; the event rows push the composition over its bound.
        assert!(
            read(&mut memory, "a")
                .unwrap_err()
                .to_string()
                .contains("bound")
        );
    }
}
