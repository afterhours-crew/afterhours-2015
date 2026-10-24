// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    events::{Identity, Node},
    interfaces::Field,
};

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
fn fixture() -> (Composition, Vec<Interface>) {
    let identity = |asset: &str, index, nested| Identity {
        asset: asset.into(),
        index,
        instances: if nested {
            vec![("root".into(), 2)]
        } else {
            Vec::new()
        },
    };
    let nodes = [
        ("root", 0, "ParticipantLogicBlueprint", false, None),
        ("root", 1, "FloatEntityData", false, None),
        ("root", 2, "LogicPrefabReferenceObjectData", false, Some(7)),
        ("root", 3, "FloatEntityData", false, None),
        ("child", 0, "LogicPrefabBlueprint", true, None),
        ("child", 1, "FloatEntityData", true, None),
        ("child", 2, "FloatEntityData", true, None),
        ("child", 3, "InterfaceDescriptorData", true, Some(2)),
    ]
    .into_iter()
    .map(|(asset, index, class, nested, forward)| Node {
        identity: identity(asset, index, nested),
        class: class.into(),
        listeners: None,
        forward,
    })
    .collect();
    let buses = vec![identity("root", 0, false), identity("child", 0, true)];
    let interfaces = vec![Interface {
        identity: identity("child", 3, true),
        bus: buses[1].clone(),
        fields: vec![
            Field {
                id: 100,
                opaque_default: None,
                access: Access::Target,
                default: None,
            },
            Field {
                id: 200,
                opaque_default: None,
                access: Access::Source,
                default: None,
            },
        ],
    }];
    (
        Composition {
            nodes,
            buses,
            bus_entities: vec![],
            connections: Vec::new(),
            links: Vec::new(),
            properties: vec![
                row(0, 0, port(1, 10), port(2, 100), 3),
                row(0, 1, port(2, 200), port(3, 20), 3),
                row(1, 0, port(7, 100), port(6, 30), 3),
                row(1, 1, port(5, 40), port(7, 200), 19),
            ],
        },
        interfaces,
    )
}

#[test]
fn opaque_interface_defaults_are_retained_but_cannot_feed_a_server_cell() {
    let (mut graph, mut interfaces) = fixture();
    interfaces[0].fields[0].opaque_default = Some("Vec2 (1,2)".into());
    assert!(
        graph
            .typed_property_layout(&interfaces, Realm::Server, &[])
            .unwrap_err()
            .to_string()
            .contains("unsupported retained interface default")
    );
    for row in &mut graph.properties {
        row.flags = Some(2);
    }
    assert!(
        graph
            .typed_property_layout(&interfaces, Realm::Server, &[])
            .is_ok()
    );
    assert_eq!(
        interfaces[0].fields[0].opaque_default.as_deref(),
        Some("Vec2 (1,2)")
    );
}

#[test]
fn prefab_outputs_inherit_parent_use_and_inputs_keep_direction() {
    let (mut c, interfaces) = fixture();
    // Output-boundary realm comes from the parent consumer, not this row.
    c.properties[3].flags = Some(18);
    let plan = c.property_plan(&interfaces, Realm::Server).unwrap();
    assert_eq!(plan.excluded_rows, 0);
    let root = &plan.buses[0];
    assert_eq!(root.inputs[0].source(), Source::Output(port(1, 10)));
    let child = &plan.buses[1];
    assert_eq!(child.parent_reference, Some(2));
    assert_eq!(child.inputs[0].source(), Source::Output(port(7, 100)));
    assert_eq!(
        child.outputs,
        [OutputAlias {
            parent: port(2, 200),
            child: port(5, 40)
        }]
    );
    assert_eq!(child.sources[&Source::Output(port(5, 40))], 3);
    assert_eq!(child.sources[&Source::Output(port(7, 100))], 3);
    assert!(
        c.property_plan(&interfaces, Realm::Client).unwrap().buses[1]
            .outputs
            .is_empty()
    );
    // An unused parent output does not create an internal boundary binding.
    c.properties.remove(1);
    let plan = c.property_plan(&interfaces, Realm::Server).unwrap();
    assert_eq!(plan.excluded_rows, 1);
    assert!(plan.buses[1].outputs.is_empty());
    assert!(
        !plan.buses[1]
            .sources
            .contains_key(&Source::Output(port(5, 40)))
    );
}

#[test]
fn merge_values_are_distinct_from_producers_and_keep_repeated_rows() {
    let (mut c, interfaces) = fixture();
    c.properties
        .insert(1, row(0, 1, port(3, 11), port(2, 100), 19));
    c.properties[2].index = 2;
    let plan = c.property_plan(&interfaces, Realm::Server).unwrap();
    let root = &plan.buses[0];
    assert_eq!(root.inputs[0].producers, [port(1, 10), port(3, 11)]);
    assert_eq!(root.inputs[0].source(), Source::Merge(port(2, 100)));
    assert_eq!(root.inputs[0].mode, 0);
    assert_eq!(root.sources[&Source::Merge(port(2, 100))], 3);
    assert_eq!(root.sources[&Source::Output(port(1, 10))], 0);
    assert_eq!(root.sources[&Source::Output(port(3, 11))], 0);
    c.properties[1].source = port(1, 10);
    let repeated = c.property_plan(&interfaces, Realm::Server).unwrap();
    assert_eq!(repeated.buses[0].inputs[0].producers, [port(1, 10); 2]);
    assert_eq!(repeated.buses[0].sources.len(), 3);
    c.properties
        .insert(2, row(0, 2, port(3, 12), port(2, 100), 3));
    c.properties[3].index = 3;
    assert!(c.property_plan(&interfaces, Realm::Server).is_err());
}

#[test]
fn source_metadata_retains_first_record_replacement_and_combined_realms() {
    let (mut c, interfaces) = fixture();
    c.properties = vec![
        row(0, 0, port(1, 10), port(3, 1), 1),
        row(0, 1, port(1, 10), port(3, 2), 19),
        row(0, 2, port(1, 11), port(3, 2), 3),
        row(0, 3, port(1, 10), port(3, 3), 2),
    ];
    let plan = c.property_plan(&interfaces, Realm::Server).unwrap();
    assert_eq!(plan.excluded_rows, 1);
    let root = &plan.buses[0];
    // Native construction replaces the first record for this source, even
    // though the second target is the one that gained a second producer.
    assert_eq!(root.sources[&Source::Merge(port(3, 2))], 1);
    assert_eq!(root.sources[&Source::Output(port(1, 10))], 1);
    assert_eq!(root.inputs[1].mode, 1);
    assert_eq!(combine(0, 3), 3);
    assert_eq!(combine(3, 0), 1);
    assert_eq!(combine(2, 3), 1);
    assert_eq!(combine(9, 3), 9);
}

#[test]
fn implicit_outputs_use_blueprint_identity_and_signed_field_order() {
    let (mut c, mut interfaces) = fixture();
    c.properties = vec![
        row(0, 0, port(2, 3), port(3, 1), 3),
        row(0, 1, port(2, u32::MAX), port(3, 2), 3),
        row(0, 2, port(2, 200), port(3, 3), 3),
    ];
    interfaces[0].fields.push(Field {
        id: 3,
        opaque_default: None,
        access: Access::Target,
        default: None,
    });
    let plan = c.property_plan(&interfaces, Realm::Server).unwrap();
    assert_eq!(
        plan.buses[1].outputs,
        [
            OutputAlias {
                parent: port(2, u32::MAX),
                child: port(4, u32::MAX)
            },
            OutputAlias {
                parent: port(2, 3),
                child: port(4, 3)
            },
        ]
    );
    assert_eq!(plan.buses[1].sources[&Source::Output(port(4, 3))], 0);
}

#[test]
fn unsupported_modes_bridges_and_malformed_compositions_fail_closed() {
    for flags in [0, 4, 5, 8, 11, 32, 35, 51, u32::MAX] {
        let (mut c, interfaces) = fixture();
        c.properties[0].flags = Some(flags);
        assert!(
            c.property_plan(&interfaces, Realm::Server).is_err(),
            "{flags}"
        );
    }
    for change in 0..12 {
        let (mut c, mut interfaces) = fixture();
        match change {
            0 => c.properties[0].flags = None,
            1 => c.properties[0].index = 3,
            2 => c.properties[0].bus = 9,
            3 => c.properties[0].source.node = 99,
            4 => c.properties[0].source.node = 5,
            5 => c.properties[3].source.node = 7,
            6 => c.nodes[2].forward = None,
            7 => c.nodes[7].forward = None,
            8 => {
                interfaces.clear();
            }
            9 => interfaces.push(interfaces[0].clone()),
            10 => {
                let duplicate = interfaces[0].fields[0].clone();
                interfaces[0].fields.push(duplicate);
            }
            11 => c.buses.swap(0, 1),
            _ => unreachable!(),
        }
        assert!(
            c.property_plan(&interfaces, Realm::Server).is_err(),
            "case {change}"
        );
    }
    let (mut c, interfaces) = fixture();
    assert!(c.property_plan(&interfaces, Realm::Both).is_err());
    c.properties = vec![c.properties[0]; MAX_CONNECTIONS + 1];
    assert!(c.property_plan(&interfaces, Realm::Server).is_err());
}

#[test]
fn resolved_cells_cross_prefab_boundaries_without_aliasing_merge_producers() {
    let (mut c, interfaces) = fixture();
    c.properties
        .insert(1, row(0, 1, port(3, 11), port(2, 100), 3));
    c.properties[2].index = 2;
    let layout = c.property_layout(&interfaces, Realm::Server).unwrap();
    let merged = layout.inputs[&port(2, 100)];
    assert_eq!(merged, layout.outputs[&port(7, 100)]);
    assert_eq!(merged, layout.inputs[&port(6, 30)]);
    let a = layout.outputs[&port(1, 10)];
    let b = layout.outputs[&port(3, 11)];
    assert_eq!(layout.cells[merged].producers, [a, b]);
    assert_ne!(a, b);
    assert_ne!(a, merged);
    assert_ne!(b, merged);
    assert_eq!(layout.outputs[&port(2, 200)], layout.outputs[&port(5, 40)]);
    assert_eq!(layout.outputs[&port(5, 40)], layout.inputs[&port(3, 20)]);
    assert_eq!(layout.cells.len(), 4);
    // Repeated producer rows still create two forwarding registrations, while
    // referencing one producer value and a different merge value.
    c.properties[1].source = port(1, 10);
    let repeated = c.property_layout(&interfaces, Realm::Server).unwrap();
    let merged = repeated.inputs[&port(2, 100)];
    let source = repeated.outputs[&port(1, 10)];
    assert_eq!(repeated.cells[merged].producers, [source; 2]);
    assert_ne!(source, merged);
}

#[test]
fn conflicting_external_output_aliases_fail_instead_of_losing_a_parent_value() {
    let (mut c, mut interfaces) = fixture();
    c.properties
        .insert(2, row(0, 2, port(2, 201), port(3, 21), 3));
    c.properties.push(row(1, 2, port(5, 40), port(7, 201), 19));
    interfaces[0].fields.push(Field {
        id: 201,
        opaque_default: None,
        access: Access::Source,
        default: None,
    });
    assert!(c.property_plan(&interfaces, Realm::Server).is_ok());
    assert!(c.property_layout(&interfaces, Realm::Server).is_err());
}

#[test]
fn interface_default_constrains_shared_type_without_seeding_any_value() {
    use nfs_world::scoring::properties::Kind;
    use nfs_world::scoring::value::Value;
    let (c, mut interfaces) = fixture();
    interfaces[0].fields[0].default = Some(Value::Float(7.0));
    let typed = c
        .typed_property_layout(&interfaces, Realm::Server, &[])
        .unwrap();
    assert_eq!(
        typed.kinds[typed.layout.outputs[&port(1, 10)]],
        Some(Kind::Float)
    );
    assert_eq!(
        typed.kinds[typed.layout.inputs[&port(6, 30)]],
        Some(Kind::Float)
    );
    assert!(typed.kinds.contains(&None));
    interfaces[0].fields[0].default = None;
    let untyped = c
        .typed_property_layout(&interfaces, Realm::Server, &[])
        .unwrap();
    assert!(untyped.kinds.iter().all(Option::is_none));
}
