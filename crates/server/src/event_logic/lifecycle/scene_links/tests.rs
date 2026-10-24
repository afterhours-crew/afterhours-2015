// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_logic::lifecycle::links::{Endpoint, Object};
use nfs_content::{
    events::lifecycle::channels::Definition,
    scoring::events::{Composition, DataConnection, Node},
};
use nfs_world::scoring::properties::Port;

fn identity(index: usize, occurrence: usize) -> Identity {
    Identity {
        asset: "fixture-prefab".into(),
        index,
        instances: vec![("fixture-scene".into(), occurrence)],
    }
}
fn composition(reference: bool, occurrence: usize) -> Composition {
    Composition {
        nodes: (0..2)
            .map(|index| Node {
                identity: identity(index, occurrence),
                class: if index == 1 {
                    "IntParticipantPropertyEntityData"
                } else if reference {
                    "LinkedSchematicChannelReferenceEntityData"
                } else {
                    "LinkedSchematicChannelEntityData"
                }
                .into(),
                forward: None,
                listeners: None,
            })
            .collect(),
        buses: vec![],
        bus_entities: vec![],
        connections: vec![],
        properties: vec![],
        links: if reference {
            vec![]
        } else {
            vec![DataConnection {
                source: Port { node: 0, field: 10 },
                target: Port { node: 1, field: 0 },
                bus: 0,
                index: 0,
                flags: None,
            }]
        },
    }
}
fn definitions(reference: bool, occurrence: usize) -> BTreeMap<Identity, Definition> {
    BTreeMap::from([(
        identity(0, occurrence),
        Definition {
            asset: "shared-schema".into(),
            reference,
            relay: false,
            links: vec![10],
            properties: vec![],
            reference_properties: vec![],
        },
    )])
}
fn fixture() -> (Registry, BTreeMap<u32, Scene>, Vec<Channel>) {
    let mut registry = Registry::default();
    registry
        .install(1, Arc::new(composition(true, 5)), &definitions(true, 5))
        .unwrap();
    registry
        .install(2, Arc::new(composition(true, 5)), &definitions(true, 5))
        .unwrap();
    let scenes = [(71, 10, 8), (72, 11, 9)]
        .into_iter()
        .map(|(key, instance, occurrence)| {
            let scene = Scene {
                instance,
                sources: Arc::new(SceneSources {
                    name: format!("scene-{key}"),
                    content_key: key,
                    boundary: None,
                    composition: composition(false, occurrence),
                    linked_data: Default::default(),
                    serializers: BTreeMap::from([
                        (3, identity(0, occurrence)),
                        (4, identity(1, occurrence)),
                    ]),
                    channels: definitions(false, occurrence),
                }),
            };
            registry.install_scene(&scene).unwrap();
            (key, scene)
        })
        .collect();
    let channels = vec![Channel {
        reference: identity(0, 5),
        definition: identity(0, 8),
        asset: "shared-schema".into(),
        owner: Binding::Scene {
            content_key: 71,
            serializer: 3,
        },
        links: BTreeMap::new(),
    }];
    (registry, scenes, channels)
}

#[test]
fn equal_schemas_in_separate_events_bind_their_actual_scene_occurrences() {
    let (mut registry, scenes, mut channels) = fixture();
    registry
        .bind_event_channels(1, &channels, &[], &scenes)
        .unwrap();
    channels[0].definition = identity(0, 9);
    channels[0].owner = Binding::Scene {
        content_key: 72,
        serializer: 3,
    };
    registry
        .bind_event_channels(2, &channels, &[], &scenes)
        .unwrap();
    for (instance, expected) in [(1, 10), (2, 11)] {
        assert_eq!(
            registry
                .resolve(Endpoint {
                    object: Object { instance, node: 0 },
                    port: 10
                })
                .unwrap(),
            vec![Object {
                instance: expected,
                node: 1
            }]
        );
    }
}

#[test]
fn wrong_serializer_or_occurrence_rolls_back_all_bindings() {
    let (mut registry, scenes, channels) = fixture();
    let mut bad = channels.clone();
    bad.push(Channel {
        owner: Binding::Scene {
            content_key: 71,
            serializer: 4,
        },
        ..channels[0].clone()
    });
    assert_eq!(
        registry.bind_event_channels(1, &bad, &[], &scenes),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        registry.binding(Object {
            instance: 1,
            node: 0
        }),
        None
    );
    bad[1].owner = Binding::Scene {
        content_key: 72,
        serializer: 3,
    };
    assert_eq!(
        registry.bind_event_channels(1, &bad, &[], &scenes),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        registry.binding(Object {
            instance: 1,
            node: 0
        }),
        None
    );
    registry
        .bind_event_channels(1, &channels, &[], &scenes)
        .unwrap();
    assert_eq!(
        registry.binding(Object {
            instance: 1,
            node: 0
        }),
        Some(Object {
            instance: 10,
            node: 0
        })
    );
}

#[test]
fn absent_or_wrong_scene_key_is_not_substituted() {
    let (mut registry, mut scenes, channels) = fixture();
    assert_eq!(
        registry.bind_event_channels(1, &channels, &[], &BTreeMap::new()),
        Err(Error::MissingInput)
    );
    let wrong = scenes.remove(&72).unwrap();
    scenes.insert(71, wrong);
    assert_eq!(
        registry.bind_event_channels(1, &channels, &[], &scenes),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        registry.binding(Object {
            instance: 1,
            node: 0
        }),
        None
    );
}
