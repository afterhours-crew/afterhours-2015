// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::events::{DataConnection, Node as EventNode};
use nfs_frostbite::ebx::{Fields, Value as Ebx};

fn object(class: &str, fields: Vec<(&str, Ebx)>) -> Object {
    Object {
        class: class.into(),
        guid: None,
        fields: Fields(fields.into_iter().map(|(k, v)| (k.into(), v)).collect()),
    }
}

#[test]
fn owned_vehicle_queries_declare_only_the_verified_output_types() {
    use nfs_world::scoring::vehicle_query as q;
    for (class, query) in [
        ("RaceVehiclePhysicsQueryEntityData", q::Query::Physics),
        ("RaceVehicleDriftQueryEntityData", q::Query::Drift),
    ] {
        let declaration = compile(&object(class, vec![])).unwrap().unwrap();
        assert!(declaration.inputs.is_empty());
        let mut expected: Vec<_> = query.outputs().iter().map(|o| (o.field, o.kind)).collect();
        expected.sort_by_key(|p| p.0);
        assert_eq!(declaration.outputs, expected);
    }
    let physics = compile(&object("RaceVehiclePhysicsQueryEntityData", vec![]))
        .unwrap()
        .unwrap();
    assert!(physics.outputs.contains(&(q::AIRBORNE, Kind::Boolean)));
    assert!(physics.outputs.contains(&(q::TRANSFORM, Kind::Transform)));
    assert!(physics.outputs.contains(&(q::VELOCITY, Kind::Vec3)));
}
fn instruction(name: &str, code: i32, result: u32, p1: u32, p2: u32) -> Ebx {
    Ebx::Struct(
        object(
            "",
            vec![
                (
                    "Code",
                    Ebx::Enum {
                        value: code,
                        name: Some(format!("MathOpCode_{name}")),
                    },
                ),
                ("Result", Ebx::UInt32(result)),
                ("Param1", Ebx::UInt32(p1)),
                ("Param2", Ebx::UInt32(p2)),
            ],
        )
        .fields,
    )
}
fn math(rows: Vec<Ebx>) -> Object {
    object(
        "MathEntityData",
        vec![
            ("EvaluateOnEvent", Ebx::Bool(false)),
            (
                "Assembly",
                Ebx::Struct(
                    object(
                        "",
                        vec![
                            ("Instructions", Ebx::Array(rows)),
                            ("FunctionCalls", Ebx::Array(vec![])),
                        ],
                    )
                    .fields,
                ),
            ),
        ],
    )
}

#[test]
fn math_input_and_return_types_are_read_from_validated_instructions() {
    let rows = vec![
        instruction("InputI", 4, 0, 100, 0),
        instruction("InputF", 5, 0, 200, 0),
        instruction("InputF", 5, 1, 200, 0),
    ];
    for (kind, code) in [(Kind::Integer, 2), (Kind::Float, 4)] {
        let mut instructions = rows.clone();
        instructions.push(instruction("Return", 67, 0, code, 0));
        let d = compile(&math(instructions)).unwrap().unwrap();
        assert_eq!(d.inputs, [(100, Kind::Integer), (200, Kind::Float)]);
        assert_eq!(d.outputs, [(nodes::OUT, kind)]);
    }
    let mut bool_rows = rows.clone();
    bool_rows.extend([
        instruction("LessF", 17, 0, 0, 1),
        instruction("Return", 67, 0, 1, 0),
    ]);
    assert_eq!(
        compile(&math(bool_rows)).unwrap().unwrap().outputs,
        [(nodes::OUT, Kind::Boolean)]
    );
    let mut conflict = rows;
    conflict.extend([
        instruction("InputF", 5, 1, 100, 0),
        instruction("Return", 67, 0, 4, 1),
    ]);
    assert!(compile(&math(conflict)).is_err());
}

#[test]
fn operator_counts_vector_types_and_unknown_classes_remain_explicit() {
    let d = compile(&object(
        "MathOpEntityData",
        vec![(
            "Operators",
            Ebx::Array(vec![Ebx::Enum {
                value: 0,
                name: Some("MathOp_Add".into()),
            }]),
        )],
    ))
    .unwrap()
    .unwrap();
    assert_eq!(d.inputs.len(), 2);
    assert!(d.inputs.contains(&(world::arithmetic::IN0, Kind::Float)));
    assert!(
        d.inputs
            .contains(&(world::boolean_fold::input_port(1).unwrap(), Kind::Float))
    );
    let v = object(
        "VectorMathOpEntityData",
        vec![(
            "MathOperator",
            Ebx::Enum {
                value: 6,
                name: Some("VectorMathOp_Length".into()),
            },
        )],
    );
    let d = compile(&v).unwrap().unwrap();
    assert_eq!(d.inputs, [(world::vector_length::INPUT, Kind::Vec3)]);
    assert!(
        d.outputs
            .contains(&(world::vector_length::NORMAL, Kind::Vec3))
    );
    assert!(
        compile(&object("UnknownFloatProducerEntityData", vec![]))
            .unwrap()
            .is_none()
    );
    assert!(compile(&object("MathOpEntityData", vec![])).is_err());
    assert!(
        compile(&object(
            "FloatCacheEntityData",
            vec![("InValue", Ebx::Int32(0))]
        ))
        .is_err()
    );
}

fn fixture() -> (Composition, Vec<Node>) {
    let identity = |index| Identity {
        asset: "root".into(),
        index,
        instances: vec![],
    };
    let classes = [
        "ParticipantLogicBlueprint",
        "SyncedFloatEntityData",
        "UnknownProducer",
        "UnknownConsumer",
    ];
    let c = Composition {
        nodes: classes
            .iter()
            .enumerate()
            .map(|(index, &class)| EventNode {
                identity: identity(index),
                class: class.into(),
                listeners: None,
                forward: None,
            })
            .collect(),
        buses: vec![identity(0)],
        connections: vec![],
        links: vec![],
        properties: vec![
            DataConnection {
                source: Port { node: 2, field: 30 },
                target: Port {
                    node: 1,
                    field: nodes::IN,
                },
                bus: 0,
                index: 0,
                flags: Some(3),
            },
            DataConnection {
                source: Port {
                    node: 1,
                    field: nodes::OUT,
                },
                target: Port { node: 3, field: 40 },
                bus: 0,
                index: 1,
                flags: Some(3),
            },
            DataConnection {
                source: Port { node: 2, field: 50 },
                target: Port { node: 3, field: 60 },
                bus: 0,
                index: 2,
                flags: Some(3),
            },
        ],
    };
    let d = vec![Node {
        identity: identity(1),
        class: classes[1].into(),
        declaration: compile(&object(classes[1], vec![])).unwrap().unwrap(),
    }];
    (c, d)
}

#[test]
fn connected_types_do_not_establish_unknown_node_behavior_or_defaults() {
    let (c, d) = fixture();
    let typed = c.typed_property_layout(&[], Realm::Server, &d).unwrap();
    assert_eq!(typed.constraints.len(), 2);
    assert_eq!(
        typed
            .kinds
            .iter()
            .filter(|k| **k == Some(Kind::Float))
            .count(),
        2
    );
    assert_eq!(typed.kinds.iter().filter(|k| k.is_none()).count(), 1);
    assert!(
        typed
            .layout
            .unready_definitions(&typed.constraints)
            .is_err()
    );
    let client = c.typed_property_layout(&[], Realm::Client, &d).unwrap();
    assert!(client.constraints.is_empty() && client.kinds.is_empty());
    for change in 0..4 {
        let mut bad_declarations = d.clone();
        match change {
            0 => bad_declarations[0]
                .identity
                .instances
                .push(("foreign".into(), 1)),
            1 => bad_declarations[0].class = "SyncedIntEntityData".into(),
            2 => bad_declarations.push(d[0].clone()),
            3 => bad_declarations[0]
                .declaration
                .inputs
                .push((nodes::IN, Kind::Integer)),
            _ => unreachable!(),
        }
        assert!(
            c.typed_property_layout(&[], Realm::Server, &bad_declarations)
                .is_err()
        );
    }
}
