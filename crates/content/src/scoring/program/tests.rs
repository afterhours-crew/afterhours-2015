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
        interfaces: vec![Interface {
            identity: child(4),
            bus: child(0),
            fields: vec![
                Field {
                    id: 500,
                    access: Access::Target,
                    default: None,
                },
                Field {
                    id: 600,
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
