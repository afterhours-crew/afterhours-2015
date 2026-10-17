// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
fn vector(v: &[f32]) -> Value {
    Value::Struct(Fields(
        ["x", "y", "z", "w"]
            .into_iter()
            .zip(v)
            .map(|(k, v)| (k.into(), Value::Float32(*v)))
            .collect(),
    ))
}
fn object() -> Object {
    Object {
        class: "PropertyGateEntityData".into(),
        guid: None,
        fields: Fields(vec![
            ("Default".into(), Value::Bool(true)),
            ("WritePropertyOnOpenGate".into(), Value::Bool(false)),
            ("BoolIn".into(), Value::Bool(true)),
            ("IntIn".into(), Value::Int32(-17)),
            ("FloatIn".into(), Value::Float32(0.25)),
            ("Vec3In".into(), vector(&[1.0, 2.0, 3.0])),
            ("Vec4In".into(), vector(&[4.0, 3.0, 2.0, 1.0])),
            (
                "TransformIn".into(),
                Value::Struct(Fields(
                    ["right", "up", "forward", "trans"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, k)| (k.into(), vector(&[i as f32, 0.0, 1.0])))
                        .collect(),
                )),
            ),
        ]),
    }
}
#[test]
fn all_channels_preserve_typed_defaults_and_transform_order() {
    let config = compile(&object()).unwrap();
    assert_eq!(
        config.definition,
        Definition {
            open: true,
            write_on_open: false
        }
    );
    assert_eq!(
        config.defaults,
        [
            GraphValue::Boolean(true),
            GraphValue::Integer(-17),
            GraphValue::Float(0.25),
            GraphValue::Vec3([1.0, 2.0, 3.0]),
            GraphValue::Vec4([4.0, 3.0, 2.0, 1.0]),
            GraphValue::Transform([
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [2.0, 0.0, 1.0],
                [3.0, 0.0, 1.0]
            ])
        ]
    );
}
#[test]
fn malformed_scalar_vector_and_transform_defaults_fail() {
    for (name, value) in [
        ("IntIn", Value::UInt32(7)),
        ("FloatIn", Value::Float32(f32::NAN)),
        ("Vec3In", vector(&[1.0, 2.0])),
        ("Vec4In", vector(&[1.0, 2.0, 3.0, f32::INFINITY])),
        ("TransformIn", Value::Struct(Fields(vec![]))),
    ] {
        let mut obj = object();
        obj.fields.0.iter_mut().find(|(k, _)| k == name).unwrap().1 = value;
        assert!(compile(&obj).is_err(), "{name}");
    }
}
