// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, local, ptr};
use nfs_frostbite::ebx::Pointer;
use nfs_world::scoring::events::{Dispatch, Handler};

#[test]
fn scene_connection_inventory_preserves_executable_and_wire_index_bounds() {
    let mut memory = setup();
    memory
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(name, _)| name == "EventConnections")
        .unwrap()
        .1 = Value::Array(vec![edge(3, 10, 4, 12, 3); events::MAX_CONNECTIONS + 1]);
    assert!(read(&mut memory, "a").is_err());
    let inventory = read_scene(&mut memory, Site::root("a")).unwrap();
    assert!(inventory.connections.len() > events::MAX_CONNECTIONS);
    assert!(inventory.dispatcher().is_err());
    let mut budget = ConnectionBudget {
        visited: 0,
        limit: events::MAX_TOPOLOGY_CONNECTIONS,
    };
    for _ in 0..4 {
        budget.visit(65_536).unwrap();
    }
    assert!(budget.visit(1).is_err());
    assert!(
        ConnectionBudget {
            visited: 0,
            limit: events::MAX_TOPOLOGY_CONNECTIONS
        }
        .visit(65_537)
        .is_err()
    );
}

#[test]
fn scene_topology_inventory_has_a_separate_finite_bound_from_executable_graphs() {
    let mut memory = Memory::default();
    for index in 1..=MAX_SCENE_NODES {
        memory.put("scene", index, "OBBData", &[]);
    }
    let mut root = |count| {
        memory.put(
            "scene",
            0,
            "SubWorldData",
            &[
                ("Objects", Value::Array((1..count).map(local).collect())),
                ("Interface", Value::Pointer(Pointer::Null)),
                ("EventConnections", Value::Array(vec![])),
                ("PropertyConnections", Value::Array(vec![])),
                ("LinkConnections", Value::Array(vec![])),
                ("Flags", Value::UInt16(0)),
            ],
        );
    };
    root(MAX_SCENE_NODES);
    assert!(
        read(&mut memory, "scene")
            .unwrap_err()
            .to_string()
            .contains("event node bound")
    );
    let inventory = read_scene(&mut memory, Site::root("scene")).unwrap();
    assert_eq!(inventory.nodes.len(), MAX_SCENE_NODES);
    assert!(inventory.dispatcher().is_err());
    memory
        .0
        .get_mut(&("scene".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(name, _)| name == "Objects")
        .unwrap()
        .1 = Value::Array((1..=MAX_SCENE_NODES).map(local).collect());
    assert!(
        read_scene(&mut memory, Site::root("scene"))
            .unwrap_err()
            .to_string()
            .contains("bound")
    );
}

#[test]
fn client_widget_children_do_not_enter_the_server_scene_graph() {
    let mut memory = setup();
    // Two client references share a widget asset. Its authored Object graph
    // belongs to each client widget; neither copy belongs to the server scene.
    for index in [1, 2] {
        memory.put(
            "a",
            index,
            "UIElementWidgetReferenceEntityData",
            &[("Blueprint", ptr("w", 0))],
        );
    }
    memory.put("w", 0, "UIWidgetBlueprint", &[("Object", local(1))]);
    memory.put(
        "w",
        1,
        "ProgressionTrackStageButtonWidgetData",
        &[("Components", Value::Array(vec![local(2)]))],
    );
    memory.put(
        "w",
        2,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    let graph = read_scene(&mut memory, Site::root("a")).unwrap();
    assert_eq!(graph.nodes.len(), 5);
    assert_eq!(graph.buses.len(), 1);
    assert_eq!(graph.connections.len(), 4);
    assert!(graph.nodes.iter().all(|n| n.identity.asset == "a"));
    // The raw constructor census remains available and is not pruned globally.
    assert!(
        Graph::new(&mut memory)
            .read(Site::root("a"))
            .unwrap()
            .iter()
            .any(|row| row.site.key.0 == "w")
    );
}

#[test]
fn scene_linked_shape_keeps_each_prefab_occurrence_without_inventing_a_constructor() {
    let mut memory = setup();
    memory.put("b", 4, "OBBData", &[]);
    let fields = &mut memory.0.get_mut(&("b".into(), 0)).unwrap().fields.0;
    fields
        .iter_mut()
        .find(|(name, _)| name == "LinkConnections")
        .unwrap()
        .1 = Value::Array(vec![crate::graph::tests::edge(local(1), 17, local(4), 0)]);
    assert!(read(&mut memory, "a").is_err());
    let graph = read_scene(&mut memory, Site::root("a")).unwrap();
    let shapes: Vec<_> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.class == "OBBData")
        .collect();
    assert_eq!(shapes.len(), 2);
    assert_ne!(shapes[0].1.identity, shapes[1].1.identity);
    for (index, shape) in shapes {
        assert_eq!(shape.forward, None);
        assert!(
            graph
                .links
                .iter()
                .any(|link| usize::from(link.target.node) == index && link.target.field == 0)
        );
    }
    let constructor_rows = Graph::new(&mut memory).read(Site::root("a")).unwrap();
    assert!(
        constructor_rows
            .iter()
            .all(|row| row.object.class != "OBBData")
    );
}

#[test]
fn prefab_without_an_interface_keeps_internal_wiring_and_rejects_dangling_rows() {
    let mut memory = setup();
    let fields = &mut memory.0.get_mut(&("b".into(), 0)).unwrap().fields.0;
    fields
        .iter_mut()
        .find(|(name, _)| name == "Interface")
        .unwrap()
        .1 = Value::Pointer(Pointer::Null);
    // Existing rows reference the interface that was removed.
    assert!(read(&mut memory, "a").is_err());
    let fields = &mut memory.0.get_mut(&("b".into(), 0)).unwrap().fields.0;
    fields
        .iter_mut()
        .find(|(name, _)| name == "EventConnections")
        .unwrap()
        .1 = Value::Array(vec![edge(1, 13, 3, 14, 3)]);
    let graph = read(&mut memory, "a").unwrap();
    assert_eq!(graph.connections.len(), 6);
    assert!(
        graph
            .nodes
            .iter()
            .filter(|n| n.class == "LogicPrefabReferenceObjectData")
            .all(|n| n.forward.is_none())
    );
}

#[test]
fn separately_owned_scene_preserves_ancestor_identity_without_a_local_parent() {
    let mut memory = setup();
    let original = read(&mut memory, "a").unwrap();
    let mut root = Site::root("a");
    root.parents.push(Frame {
        bus: ("level".into(), 0),
        reference: ("level".into(), 7),
    });
    let scoped = read_at(&mut memory, root).unwrap();
    assert_eq!(scoped.connections, original.connections);
    assert_eq!(scoped.properties, original.properties);
    assert_eq!(scoped.links, original.links);
    assert_eq!(scoped.bus_entities, original.bus_entities);
    for (actual, expected) in scoped.nodes.iter().zip(&original.nodes) {
        let mut identity = expected.identity.clone();
        identity.instances.insert(0, ("level".into(), 7));
        assert_eq!(actual.identity, identity);
        assert_eq!(actual.forward, expected.forward);
    }
    // A real root interface remains an endpoint of this separately owned
    // scene; it must not forward to an unrelated local node with the same ID.
    memory.put("a", 8, "InterfaceDescriptorData", &[]);
    memory
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(name, _)| name == "Interface")
        .unwrap()
        .1 = local(8);
    let mut root = Site::root("a");
    root.parents.push(Frame {
        bus: ("level".into(), 0),
        reference: ("level".into(), 7),
    });
    let scoped = read_at(&mut memory, root).unwrap();
    let interface = scoped
        .nodes
        .iter()
        .find(|n| n.identity.asset == "a" && n.identity.index == 8)
        .unwrap();
    assert_eq!(interface.forward, None);
}

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
            ("Flags", Value::UInt16(1)),
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
            ("Flags", Value::UInt16(15)),
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
    // Every bus here is networked (Flags bit 0): entity indexes 1..=3 in order.
    assert_eq!(composition.bus_entities, vec![Some(1), Some(2), Some(3)]);
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

fn property(
    source: usize,
    source_field: u32,
    target: usize,
    target_field: u32,
    flags: Value,
) -> Value {
    structure(&[
        ("Source", local(source)),
        ("Target", local(target)),
        ("SourceFieldId", Value::UInt32(source_field)),
        ("TargetFieldId", Value::UInt32(target_field)),
        ("Flags", flags),
    ])
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
            property(3, u32::MAX, 2, 91, Value::UInt32(0x8000_0013)),
            property(3, u32::MAX, 1, 91, Value::Int32(2)),
        ],
    );
    set_data_rows(
        &mut memory,
        "b",
        "PropertyConnections",
        vec![property(2, 91, 1, 92, Value::UInt32(17))],
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
    assert_eq!(
        composition
            .properties
            .iter()
            .map(|r| r.flags)
            .collect::<Vec<_>>(),
        [Some(0x8000_0013), Some(2), Some(17), Some(17)]
    );
    assert!(composition.links.iter().all(|r| r.flags.is_none()));
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
            vec![property(3, 1, 4, 2, Value::UInt32(3)); events::MAX_CONNECTIONS],
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

#[test]
fn property_flags_are_required_typed_and_never_defaulted() {
    for flags in [Value::Bool(true), Value::Float32(3.0), Value::UInt16(3)] {
        let mut memory = setup();
        set_data_rows(
            &mut memory,
            "a",
            "PropertyConnections",
            vec![property(3, 1, 4, 2, flags)],
        );
        assert!(read(&mut memory, "a").is_err());
    }
    let mut memory = setup();
    set_data_rows(
        &mut memory,
        "a",
        "PropertyConnections",
        vec![crate::graph::tests::edge(local(3), 1, local(4), 2)],
    );
    assert!(
        read(&mut memory, "a")
            .unwrap_err()
            .to_string()
            .contains("field missing")
    );
}
