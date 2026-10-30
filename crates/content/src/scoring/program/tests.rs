// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    events::{DataConnection, Node as EventNode},
    interfaces::Field,
    property_types::Declaration,
    realms::{Listeners, Realm},
};
use nfs_world::scoring::{
    nodes::{Definition as Scalar, FLOAT_VALUE, IN, OUT},
    synced::Kind as SyncedKind,
};

fn identity(asset: &str, index: usize, instances: &[(&str, usize)]) -> Identity {
    Identity {
        asset: asset.into(),
        index,
        instances: instances
            .iter()
            .map(|(a, i)| ((*a).to_string(), *i))
            .collect(),
    }
}
fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
fn row(bus: u16, index: u16, source: Port, target: Port, flags: u32) -> DataConnection {
    DataConnection {
        bus,
        index,
        source,
        target,
        flags: Some(flags),
    }
}
fn event_node(
    id: Identity,
    class: &str,
    listeners: Option<Listeners>,
    forward: Option<u16>,
) -> EventNode {
    EventNode {
        identity: id,
        class: class.into(),
        listeners,
        forward,
    }
}
fn server() -> Option<Listeners> {
    Some(Listeners {
        events: Realm::Server,
        properties: Realm::Server,
    })
}
fn declaration(
    id: Identity,
    class: &str,
    inputs: &[(u32, Kind)],
    outputs: &[(u32, Kind)],
) -> property_types::Node {
    property_types::Node {
        identity: id,
        class: class.into(),
        declaration: Declaration {
            inputs: inputs.to_vec(),
            outputs: outputs.to_vec(),
        },
    }
}
fn created(id: Identity, class: &str) -> creation::Node {
    creation::Node {
        identity: id,
        class: class.into(),
        realm: Realm::Server,
    }
}

/// Root bus: Float(1) -> SyncedFloat(2); a prefab reference (3) whose child
/// bus has Float(5), Float(6) merged into Float(7)'s value through an
/// interface (8) with a Source default, plus an unsupported telemetry sink (9)
/// fed by an inert provider (10) through an untyped cell.
pub(crate) fn fixture() -> Sources {
    let root = |i| identity("root", i, &[]);
    let child = |i| identity("child", i, &[("root", 3)]);
    let nodes = vec![
        event_node(root(0), "ParticipantLogicBlueprint", None, None),
        event_node(root(1), "FloatEntityData", server(), None),
        event_node(root(2), "SyncedFloatEntityData", server(), None),
        event_node(root(3), "LogicPrefabReferenceObjectData", server(), Some(8)),
        event_node(child(0), "LogicPrefabBlueprint", None, None),
        event_node(child(1), "FloatEntityData", server(), None),
        event_node(child(2), "FloatEntityData", server(), None),
        event_node(child(3), "SyncedFloatEntityData", server(), None),
        event_node(child(4), "InterfaceDescriptorData", None, Some(3)),
        event_node(root(4), "TelemetryHookEntityData", server(), None),
        event_node(root(5), "PlayerInfoAccessorEntityData", server(), None),
    ];
    let composition = Composition {
        buses: vec![root(0), child(0)],
        bus_entities: vec![],
        connections: vec![],
        links: vec![],
        properties: vec![
            row(0, 0, port(1, FLOAT_VALUE), port(2, IN), 3),
            row(0, 1, port(10, 77), port(9, 78), 3),
            row(0, 2, port(1, FLOAT_VALUE), port(3, 500), 3),
            row(1, 0, port(5, FLOAT_VALUE), port(7, IN), 3),
            row(1, 1, port(6, FLOAT_VALUE), port(7, IN), 3),
            row(1, 2, port(8, 500), port(6, 999), 3),
        ],
        nodes,
    };
    let scalar = |id, default| nodes::Node {
        identity: id,
        configuration: nodes::Configuration {
            definition: Scalar::Float { default, step: 1.0 },
            defaults: vec![],
        },
    };
    Sources {
        ai: vec![],
        burnout_overrides: vec![],
        burnouts: vec![],
        item_info: vec![],
        vehicle_info: vec![],
        teleport_locations: vec![],
        vehicle_tuning: vec![],
        context_reporters: vec![],
        transitions: vec![],
        linked_curves: vec![],
        event_details: vec![],
        sets: vec![],
        rank_queries: vec![],
        banks: vec![],
        participant_sets: vec![],
        participant_event_providers: vec![],
        interfaces: vec![Interface {
            identity: child(4),
            bus: child(0),
            fields: vec![
                Field {
                    id: 500,
                    opaque_default: None,
                    access: Access::Target,
                    default: None,
                },
                Field {
                    id: 600,
                    opaque_default: None,
                    access: Access::Target,
                    default: Some(Value::Float(4.0)),
                },
            ],
        }],
        declarations: vec![
            declaration(
                root(1),
                "FloatEntityData",
                &[],
                &[(FLOAT_VALUE, Kind::Float)],
            ),
            declaration(
                root(2),
                "SyncedFloatEntityData",
                &[(IN, Kind::Float)],
                &[(OUT, Kind::Float)],
            ),
            declaration(
                child(1),
                "FloatEntityData",
                &[],
                &[(FLOAT_VALUE, Kind::Float)],
            ),
            declaration(
                child(2),
                "FloatEntityData",
                &[(999, Kind::Float)],
                &[(FLOAT_VALUE, Kind::Float)],
            ),
            declaration(
                child(3),
                "SyncedFloatEntityData",
                &[(IN, Kind::Float)],
                &[(OUT, Kind::Float)],
            ),
        ],
        creation: vec![
            created(root(1), "FloatEntityData"),
            created(root(2), "SyncedFloatEntityData"),
            created(root(3), "LogicPrefabReferenceObjectData"),
            created(child(1), "FloatEntityData"),
            created(child(2), "FloatEntityData"),
            created(child(3), "SyncedFloatEntityData"),
            created(root(4), "TelemetryHookEntityData"),
            created(root(5), "PlayerInfoAccessorEntityData"),
        ],
        scalars: vec![
            scalar(root(1), 1.0),
            scalar(child(1), 2.0),
            scalar(child(2), 3.0),
        ],
        synced: vec![
            synced::Node {
                identity: root(2),
                kind: SyncedKind::Float,
            },
            synced::Node {
                identity: child(3),
                kind: SyncedKind::Float,
            },
        ],
        calculations: vec![],
        arithmetic: vec![],
        folds: vec![],
        gates: vec![],
        delays: vec![],
        stopwatches: vec![],
        awards: vec![],
        roads: vec![],
        drafts: vec![],
        donuts: vec![],
        proximities: vec![],
        buildups: vec![],
        composition,
    }
}

#[test]
fn compiles_behaviors_cells_and_native_startup_order() {
    let program = compile(&fixture()).unwrap();
    assert_eq!(program.nodes.len(), 11);
    assert!(matches!(
        program.nodes[1].behavior,
        Behavior::Scalar(Scalar::Float { .. })
    ));
    assert!(matches!(
        program.nodes[2].behavior,
        Behavior::Synced(SyncedKind::Float)
    ));
    assert_eq!(program.nodes[3].behavior, Behavior::Inert);
    assert_eq!(program.nodes[3].forward, Some(8));
    assert_eq!(program.nodes[9].behavior, Behavior::Unsupported);
    assert!(!program.nodes[0].created && !program.nodes[8].created);
    assert!(program.nodes[1].events && program.nodes[1].properties);
    // Typed cells: root Float out (aliases root Synced In and the prefab input,
    // which the child's interface input reuses), root Synced out, child Float
    // outs, the merge cell, child Synced out. The untyped sink cell is reported.
    let typed: Vec<_> = program.cells.iter().map(|c| c.ports.clone()).collect();
    assert!(typed.iter().any(|ports| {
        ports.contains(&port(1, FLOAT_VALUE))
            && ports.contains(&port(2, IN))
            && ports.contains(&port(3, 500))
            && ports.contains(&port(8, 500))
            && ports.contains(&port(6, 999))
    }));
    assert!(typed.iter().any(|ports| ports == &[port(7, IN)]));
    // Unconsumed synchronized outputs still own a private typed cell.
    assert!(typed.iter().any(|ports| ports == &[port(2, OUT)]));
    assert!(typed.iter().any(|ports| ports == &[port(7, OUT)]));
    assert_eq!(program.cells.len(), 6);
    assert_eq!(
        program.unmodeled,
        vec![
            Unmodeled {
                port: port(10, 77),
                class: "PlayerInfoAccessorEntityData".into()
            },
            Unmodeled {
                port: port(9, 78),
                class: "TelemetryHookEntityData".into()
            }
        ]
    );
    let expected = vec![
        Step::Bind(1),
        Step::Bind(2),
        Step::Register(Registration::Notify(port(2, IN))),
        Step::Bind(3),
        Step::Register(Registration::Forward {
            source: port(5, FLOAT_VALUE),
            target: port(7, IN),
        }),
        Step::CopyReady {
            source: port(5, FLOAT_VALUE),
            target: port(7, IN),
        },
        Step::Register(Registration::Forward {
            source: port(6, FLOAT_VALUE),
            target: port(7, IN),
        }),
        Step::CopyReady {
            source: port(6, FLOAT_VALUE),
            target: port(7, IN),
        },
        Step::Bind(5),
        Step::Bind(6),
        Step::Register(Registration::Notify(port(6, 999))),
        Step::Bind(7),
        Step::Register(Registration::Notify(port(7, IN))),
        Step::Bind(9),
        Step::Bind(10),
        Step::Activate(1),
        Step::Activate(2),
        Step::Activate(3),
        Step::Activate(5),
        Step::Activate(6),
        Step::Activate(7),
        Step::Activate(9),
        Step::Activate(10),
    ];
    assert_eq!(program.startup, expected);
    assert_eq!(program.connections, vec![]);
}

#[test]
fn interface_source_defaults_become_startup_defaults_only_when_retained() {
    let mut sources = fixture();
    // Retain the Source field by consuming it inside the child bus.
    sources
        .composition
        .properties
        .push(row(1, 3, port(8, 600), port(5, 998), 3));
    sources.declarations[2]
        .declaration
        .inputs
        .push((998, Kind::Float));
    let program = compile(&sources).unwrap();
    assert!(program.startup.contains(&Step::Default {
        port: port(8, 600),
        value: Value::Float(4.0),
        mode: DefaultMode::IfUnready,
    }));
    let position = |step: &Step| program.startup.iter().position(|s| s == step).unwrap();
    assert!(
        position(&Step::Default {
            port: port(8, 600),
            value: Value::Float(4.0),
            mode: DefaultMode::IfUnready,
        }) < position(&Step::Bind(5))
    );
    assert!(
        !compile(&fixture())
            .unwrap()
            .startup
            .iter()
            .any(|s| matches!(s, Step::Default { .. }))
    );
}

#[test]
fn listener_mode_bit_and_realm_suppress_registration() {
    let mut sources = fixture();
    sources.composition.properties[0].flags = Some(19);
    let program = compile(&sources).unwrap();
    assert!(
        !program
            .startup
            .contains(&Step::Register(Registration::Notify(port(2, IN))))
    );
    let mut sources = fixture();
    sources.composition.nodes[2].listeners = Some(Listeners {
        events: Realm::Server,
        properties: Realm::Client,
    });
    let program = compile(&sources).unwrap();
    assert!(!program.nodes[2].properties && program.nodes[2].events);
    assert!(
        !program
            .startup
            .contains(&Step::Register(Registration::Notify(port(2, IN))))
    );
}

#[test]
fn untyped_cells_reaching_modeled_nodes_and_unknown_classes_fail() {
    let mut sources = fixture();
    // Feed the untyped provider output into an undeclared port of a modeled node.
    sources
        .composition
        .properties
        .push(row(0, 3, port(10, 77), port(1, 4242), 3));
    assert!(compile(&sources).is_err());
    let mut sources = fixture();
    sources.composition.nodes[9].class = "MysteryEntityData".into();
    sources.creation[6].class = "MysteryEntityData".into();
    assert!(compile(&sources).is_err());
    // A class outside the server realm needs no behavior.
    let mut sources = fixture();
    sources.composition.nodes[9].class = "PropertyDebugEntityData".into();
    sources.creation.remove(6);
    let program = compile(&sources).unwrap();
    assert_eq!(program.nodes[9].behavior, Behavior::Inert);
    assert!(!program.nodes[9].created);
    // Authored server eligibility cannot supply a missing native factory.
    sources
        .creation
        .push(created(identity("root", 5, &[]), "PropertyDebugEntityData"));
    // Use the occurrence's actual composition identity, independent of order.
    sources.creation.last_mut().unwrap().identity = sources.composition.nodes[9].identity.clone();
    let program = compile(&sources).unwrap();
    assert!(!program.nodes[9].created);
}

#[test]
fn duplicate_or_foreign_reader_identities_fail() {
    let mut sources = fixture();
    sources.scalars.push(sources.scalars[0].clone());
    assert!(compile(&sources).is_err());
    let mut sources = fixture();
    sources.synced[0].identity = identity("elsewhere", 1, &[]);
    assert!(compile(&sources).is_err());
}

#[test]
fn integer_participant_property_accessors_compile_only_when_written() {
    let root = |i| identity("root", i, &[]);
    let mut sources = fixture();
    // An integer (11) writes the accessor's (12) Value.
    sources
        .composition
        .nodes
        .push(event_node(root(6), "IntEntityData", server(), None));
    sources.composition.nodes.push(event_node(
        root(7),
        "IntParticipantPropertyAccessorEntityData",
        server(),
        None,
    ));
    sources
        .composition
        .properties
        .push(row(0, 3, port(11, FLOAT_VALUE), port(12, FLOAT_VALUE), 3));
    sources.declarations.extend([
        declaration(
            root(6),
            "IntEntityData",
            &[],
            &[(FLOAT_VALUE, Kind::Integer)],
        ),
        declaration(
            root(7),
            "IntParticipantPropertyAccessorEntityData",
            &[(FLOAT_VALUE, Kind::Integer)],
            &[(nfs_world::scoring::nodes::OUTPUT_VALUE, Kind::Integer)],
        ),
    ]);
    sources.creation.extend([
        created(root(6), "IntEntityData"),
        created(root(7), "IntParticipantPropertyAccessorEntityData"),
    ]);
    sources.scalars.extend([
        nodes::Node {
            identity: root(6),
            configuration: nodes::Configuration {
                definition: Scalar::Integer {
                    default: 5,
                    step: 1,
                },
                defaults: vec![],
            },
        },
        nodes::Node {
            identity: root(7),
            configuration: nodes::Configuration {
                definition: Scalar::IntegerParticipantProperty,
                defaults: vec![(FLOAT_VALUE, Value::Integer(0))],
            },
        },
    ]);
    let program = compile(&sources).unwrap();
    assert_eq!(
        program.nodes[12].behavior,
        Behavior::Scalar(Scalar::IntegerParticipantProperty)
    );
    // An accessor whose Value nothing writes would read the store: refused.
    let mut reading = sources.clone();
    reading.composition.properties.pop();
    assert!(compile(&reading).is_err());
    let property = super::super::participant_properties::Property {
        channel: "test/race".into(),
        port: 77,
    };
    let bindings = BTreeMap::from([(root(7), property.clone())]);
    let program = compile_with_properties(&reading, &bindings).unwrap();
    assert_eq!(
        program.nodes[12].behavior,
        Behavior::ParticipantProperty {
            kind: Kind::Integer,
            property: property.clone(),
            write: false
        }
    );
    let program = compile_with_properties(&sources, &bindings).unwrap();
    assert_eq!(
        program.nodes[12].behavior,
        Behavior::ParticipantProperty {
            kind: Kind::Integer,
            property: property.clone(),
            write: true
        }
    );
    assert!(
        compile_with_properties(&sources, &BTreeMap::from([(root(6), property.clone())])).is_err()
    );
    assert!(
        compile_with_properties(
            &sources,
            &BTreeMap::from([(identity("foreign", 7, &[]), property)])
        )
        .is_err()
    );
}

#[test]
fn native_bindings_keep_typed_cells_and_cannot_replace_an_ordinary_reader() {
    let mut sources = fixture();
    let key = sources.scalars[0].identity.clone();
    let native = BTreeSet::from([key.clone()]);
    assert!(compile_with_native(&sources, &BTreeMap::new(), &native).is_err());
    sources.scalars.remove(0);
    let program = compile_with_native(&sources, &BTreeMap::new(), &native).unwrap();
    let node = program
        .nodes
        .iter()
        .position(|n| n.identity == super::identity(&key))
        .unwrap();
    assert_eq!(program.nodes[node].behavior, Behavior::Native);
    assert!(
        program
            .cells
            .iter()
            .any(|c| c.ports.iter().any(|p| usize::from(p.node) == node))
    );
    assert!(
        compile_with_native(
            &sources,
            &BTreeMap::new(),
            &BTreeSet::from([identity("foreign", 1, &[])])
        )
        .is_err()
    );
}

#[test]
fn selected_reader_keeps_traversal_and_links_through_excluded_nodes() {
    use crate::graph::{
        Graph, Site,
        tests::{Memory, edge, local, ptr},
    };
    use nfs_frostbite::ebx::Value as Ebx;
    let mut source = Memory::default();
    source.put(
        "r",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Ebx::Array(vec![local(1), local(2)])),
            (
                "LinkConnections",
                Ebx::Array(vec![edge(local(0), 17, local(2), 0)]),
            ),
        ],
    );
    source.put(
        "r",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("c", 0))],
    );
    source.put("r", 2, "ParticipantSetEntityData", &[]);
    source.put(
        "c",
        0,
        "LogicPrefabBlueprint",
        &[("Objects", Ebx::Array(vec![local(1)]))],
    );
    source.put("c", 1, "ParticipantSetEntityData", &[]);
    let native = BTreeSet::from([identity("r", 1, &[]), identity("r", 2, &[])]);
    let mut selected = Selected {
        source: &mut source,
        native: &native,
    };
    let mut graph = Graph::new(&mut selected);
    let rows = graph.read(Site::root("r")).unwrap();
    assert!(
        !rows
            .iter()
            .any(|n| Identity::from(&n.site) == identity("r", 1, &[]))
    );
    assert!(
        rows.iter()
            .any(|n| Identity::from(&n.site) == identity("c", 1, &[("r", 1)]))
    );
    let links = graph.links(&Site::root("r"), 17).unwrap();
    assert_eq!(
        graph.object(&links[0].site).unwrap().class,
        "ParticipantSetEntityData"
    );
}

#[test]
fn explicit_native_selection_excludes_scalar_configuration_only_for_that_occurrence() {
    use crate::graph::tests::{Memory, local};
    use nfs_frostbite::ebx::Value as Ebx;
    let mut source = Memory::default();
    source.put(
        "r",
        0,
        "LogicPrefabBlueprint",
        &[("Objects", Ebx::Array(vec![local(1), local(2)]))],
    );
    for i in 1..=2 {
        source.put(
            "r",
            i,
            "BoolParticipantPropertyAccessorEntityData",
            &[("Value", Ebx::Bool(false))],
        );
    }
    let native = BTreeSet::from([identity("r", 1, &[])]);
    let mut selected = Selected {
        source: &mut source,
        native: &native,
    };
    let nodes = super::super::nodes::read(&mut selected, "r").unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].identity, identity("r", 2, &[]));
}
