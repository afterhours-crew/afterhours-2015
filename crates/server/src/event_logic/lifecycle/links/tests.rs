// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::scoring::events::DataConnection;

#[test]
fn loaded_scene_boundaries_resolve_in_both_directions_and_require_detach() {
    let graph = |classes: &[&str], edges: Vec<DataConnection>| {
        Arc::new(Composition {
            nodes: classes
                .iter()
                .enumerate()
                .map(|(index, class)| Node {
                    identity: identity(index),
                    class: (*class).into(),
                    listeners: None,
                    forward: None,
                })
                .collect(),
            buses: vec![],
            bus_entities: vec![],
            connections: vec![],
            properties: vec![],
            links: edges,
        })
    };
    let mut registry = Registry::default();
    registry
        .install(
            1,
            graph(
                &["Consumer", "SubWorldReferenceObjectData", "ParentProvider"],
                vec![link(0, 11, 1, 20), link(1, 21, 2, 0)],
            ),
            &BTreeMap::new(),
        )
        .unwrap();
    let child = graph(
        &["InterfaceDescriptorData", "ChildProvider", "Consumer"],
        vec![link(0, 20, 1, 0), link(2, 12, 0, 21)],
    );
    registry
        .install(2, child.clone(), &BTreeMap::new())
        .unwrap();
    registry.install(3, child, &BTreeMap::new()).unwrap();
    assert_eq!(registry.input(endpoint(1, 0, 11)), Err(Error::InvalidGraph));
    registry.bind_boundary(object(1, 1), object(2, 0)).unwrap();
    assert_eq!(
        registry.input(endpoint(1, 0, 11)).unwrap(),
        vec![object(2, 1)]
    );
    assert_eq!(
        registry.input(endpoint(2, 2, 12)).unwrap(),
        vec![object(1, 2)]
    );
    assert_eq!(
        registry.bind_boundary(object(1, 1), object(3, 0)),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        registry.input(endpoint(1, 0, 11)).unwrap(),
        vec![object(2, 1)]
    );
    assert_eq!(registry.remove(1), Err(Error::InvalidGraph));
    assert_eq!(registry.remove(2), Err(Error::InvalidGraph));
    registry.unbind_boundary(object(2, 0)).unwrap();
    registry.remove(2).unwrap();
    registry.bind_boundary(object(1, 1), object(3, 0)).unwrap();
    assert_eq!(
        registry.input(endpoint(1, 0, 11)).unwrap(),
        vec![object(3, 1)]
    );
}

#[test]
fn cross_scene_cycle_is_bounded_and_invalid_endpoint_types_do_not_bind() {
    let mut registry = Registry::default();
    for (instance, classes, edges) in [
        (
            1,
            ["Consumer", "SubWorldReferenceObjectData"],
            vec![link(0, 10, 1, 20), link(1, 21, 1, 20)],
        ),
        (
            2,
            ["Provider", "InterfaceDescriptorData"],
            vec![link(1, 20, 1, 21)],
        ),
    ] {
        registry
            .install(
                instance,
                Arc::new(Composition {
                    nodes: classes
                        .iter()
                        .enumerate()
                        .map(|(index, class)| Node {
                            identity: identity(index),
                            class: (*class).into(),
                            listeners: None,
                            forward: None,
                        })
                        .collect(),
                    buses: vec![],
                    bus_entities: vec![],
                    connections: vec![],
                    properties: vec![],
                    links: edges,
                }),
                &BTreeMap::new(),
            )
            .unwrap();
    }
    assert_eq!(
        registry.bind_boundary(object(1, 1), object(2, 0)),
        Err(Error::InvalidGraph)
    );
    registry.bind_boundary(object(1, 1), object(2, 1)).unwrap();
    assert_eq!(registry.input(endpoint(1, 0, 10)), Err(Error::InvalidGraph));
}

fn identity(index: usize) -> Identity {
    Identity {
        asset: "fixture".into(),
        index,
        instances: vec![],
    }
}
fn object(instance: u64, node: u16) -> Object {
    Object { instance, node }
}
fn endpoint(instance: u64, node: u16, port: u32) -> Endpoint {
    Endpoint {
        object: object(instance, node),
        port,
    }
}
fn link(from: u16, field: u32, to: u16, target: u32) -> DataConnection {
    DataConnection {
        source: Port { node: from, field },
        target: Port {
            node: to,
            field: target,
        },
        bus: 0,
        index: 0,
        flags: None,
    }
}
fn fixture(extra_reference: bool) -> (Arc<Composition>, BTreeMap<Identity, Definition>) {
    let classes = [
        "LinkedSchematicChannelEntityData",
        "LinkedSchematicChannelReferenceEntityData",
        "IntParticipantPropertyEntityData",
        "ParticipantStateEntityData",
        "LogicPrefabReferenceObjectData",
        "InterfaceDescriptorData",
    ];
    let mut composition = Composition {
        nodes: classes
            .iter()
            .enumerate()
            .map(|(i, c)| Node {
                identity: identity(i),
                class: (*c).into(),
                listeners: None,
                forward: match i {
                    4 => Some(5),
                    5 => Some(4),
                    _ => None,
                },
            })
            .collect(),
        buses: vec![],
        bus_entities: vec![],
        connections: vec![],
        properties: vec![],
        links: vec![
            link(0, 11, 3, 0),
            link(0, 11, 4, 31),
            link(5, 31, 2, 0),
            link(0, 11, 3, 0),
        ],
    };
    let mut channels: BTreeMap<_, _> = [false, true]
        .into_iter()
        .enumerate()
        .map(|(i, reference)| {
            (
                identity(i),
                Definition {
                    asset: "event-channel".into(),
                    reference,
                    relay: false,
                    links: vec![11, 12],
                    properties: vec![],
                    reference_properties: vec![],
                },
            )
        })
        .collect();
    if extra_reference {
        composition.nodes.push(Node {
            identity: identity(6),
            class: classes[1].into(),
            listeners: None,
            forward: None,
        });
        channels.insert(identity(6), channels[&identity(1)].clone());
    }
    (Arc::new(composition), channels)
}

#[test]
fn relay_queries_follow_the_linked_definition_across_bound_channels() {
    let (c, mut d) = fixture(false);
    let mut c = (*c).clone();
    c.nodes.push(Node {
        identity: identity(6),
        class: "LinkedSchematicChannelRelayEntityData".into(),
        listeners: None,
        forward: None,
    });
    let mut relay = d[&identity(1)].clone();
    relay.relay = true;
    d.insert(identity(6), relay);
    c.links
        .extend([link(6, RELAY_SOURCE, 1, 12), link(0, 12, 0, 0)]);
    let c = Arc::new(c);
    let mut r = Registry::default();
    r.install(1, c.clone(), &d).unwrap();
    r.install(2, c, &d).unwrap();
    assert_eq!(r.relay_source(object(1, 6)), Err(Error::MissingInput));
    r.bind(object(1, 1), object(2, 0)).unwrap();
    assert_eq!(r.relay_source(object(1, 6)).unwrap(), object(2, 0));
    assert_eq!(
        r.resolve(endpoint(1, 6, 11)).unwrap(),
        [object(2, 3), object(2, 2), object(2, 3)]
    );
    assert_eq!(r.bind(object(1, 6), object(2, 0)), Err(Error::InvalidGraph));
    assert_eq!(r.resolve(endpoint(1, 6, 99)), Err(Error::InvalidGraph));
    r.unbind(object(1, 1)).unwrap();
    assert_eq!(r.resolve(endpoint(1, 6, 11)), Err(Error::MissingInput));
}

#[test]
fn relay_source_cycles_and_wrong_provider_types_fail_without_partial_results() {
    let (c, mut d) = fixture(false);
    let mut c = (*c).clone();
    c.nodes.push(Node {
        identity: identity(6),
        class: "LinkedSchematicChannelRelayEntityData".into(),
        listeners: None,
        forward: None,
    });
    let mut relay = d[&identity(1)].clone();
    relay.relay = true;
    d.insert(identity(6), relay);
    c.links.push(link(6, RELAY_SOURCE, 6, 11));
    let mut r = Registry::default();
    r.install(1, Arc::new(c.clone()), &d).unwrap();
    assert_eq!(r.resolve(endpoint(1, 6, 11)), Err(Error::InvalidGraph));
    c.links.pop();
    c.links.push(link(6, RELAY_SOURCE, 2, 0));
    r.install(2, Arc::new(c), &d).unwrap();
    assert_eq!(r.resolve(endpoint(2, 6, 11)), Err(Error::InvalidGraph));
}
#[test]
fn repeated_exports_cross_prefabs_and_resolve_in_the_bound_event_instance() {
    let (c, d) = fixture(false);
    let mut r = Registry::default();
    r.install(1, c.clone(), &d).unwrap();
    r.install(2, c.clone(), &d).unwrap();
    r.install(3, c, &d).unwrap();
    assert_eq!(r.resolve(endpoint(1, 1, 11)), Err(Error::MissingInput));
    r.bind(object(1, 1), object(2, 0)).unwrap();
    r.bind(object(3, 1), object(3, 0)).unwrap();
    assert_eq!(
        r.resolve(endpoint(1, 1, 11)).unwrap(),
        [object(2, 3), object(2, 2), object(2, 3)]
    );
    assert_eq!(
        r.property(
            3,
            &Property {
                channel: "event-channel".into(),
                port: 11
            }
        )
        .unwrap(),
        [object(3, 3), object(3, 2), object(3, 3)]
    );
    assert!(r.resolve(endpoint(1, 1, 12)).unwrap().is_empty());
    assert_eq!(r.resolve(endpoint(1, 1, 99)), Err(Error::InvalidGraph));
    assert_eq!(r.node(object(2, 2)).unwrap().identity, identity(2));
    assert_eq!(r.object(2, &identity(2)).unwrap(), object(2, 2));
}
#[test]
fn ambiguous_schema_keys_cannot_cross_two_different_live_definitions() {
    let (c, d) = fixture(true);
    let mut r = Registry::default();
    for id in [1, 2, 3] {
        r.install(id, c.clone(), &d).unwrap();
    }
    r.bind(object(1, 1), object(2, 0)).unwrap();
    r.bind(object(1, 6), object(3, 0)).unwrap();
    let key = Property {
        channel: "event-channel".into(),
        port: 11,
    };
    assert_eq!(r.property(1, &key), Err(Error::InvalidGraph));
    r.unbind(object(1, 6)).unwrap();
    r.bind(object(1, 6), object(2, 0)).unwrap();
    assert_eq!(
        r.property(1, &key).unwrap(),
        [object(2, 3), object(2, 2), object(2, 3)]
    );
    assert_eq!(r.bind(object(1, 1), object(3, 0)), Err(Error::InvalidGraph));
}
#[test]
fn cycles_and_stale_instance_reuse_fail_without_mutating_the_registry() {
    let (mut c, d) = fixture(false);
    Arc::make_mut(&mut c).links = vec![link(0, 11, 1, 11)];
    let mut r = Registry::default();
    r.install(1, c.clone(), &d).unwrap();
    r.install(2, c.clone(), &d).unwrap();
    r.bind(object(1, 1), object(2, 0)).unwrap();
    r.bind(object(2, 1), object(1, 0)).unwrap();
    assert_eq!(r.resolve(endpoint(1, 1, 11)), Err(Error::InvalidGraph));
    assert_eq!(r.remove(2), Err(Error::InvalidGraph));
    assert_eq!(r.binding(object(1, 1)), Some(object(2, 0)));
    r.unbind(object(1, 1)).unwrap();
    r.remove(2).unwrap();
    r.install(2, c, &d).unwrap();
    assert_eq!(r.resolve(endpoint(1, 1, 11)), Err(Error::MissingInput));
    assert_eq!(r.binding(object(2, 1)), None);
}
#[test]
fn incompatible_schema_and_invalid_topology_are_rejected_before_installing() {
    let (c, d) = fixture(false);
    let mut r = Registry::default();
    r.install(1, c.clone(), &d).unwrap();
    let mut other = d.clone();
    other.get_mut(&identity(0)).unwrap().asset = "different".into();
    r.install(2, c.clone(), &other).unwrap();
    assert_eq!(r.bind(object(1, 1), object(2, 0)), Err(Error::InvalidGraph));
    assert_eq!(r.bind(object(1, 0), object(1, 1)), Err(Error::InvalidGraph));
    let mut invalid = (*c).clone();
    invalid.links.push(link(0, 11, 300, 0));
    assert_eq!(r.install(3, Arc::new(invalid), &d), Err(Error::UnknownNode));
    r.install(3, c, &d).unwrap();
}
