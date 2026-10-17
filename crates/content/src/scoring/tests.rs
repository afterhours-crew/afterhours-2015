// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world::scoring::math::Value as Scalar;

fn fields(rows: &[(&str, Value)]) -> Fields {
    Fields(rows.iter().map(|(k, v)| ((*k).into(), v.clone())).collect())
}
fn structure(rows: &[(&str, Value)]) -> Value {
    Value::Struct(fields(rows))
}
fn instruction(name: &str, code: i32, result: u32, p1: u32, p2: u32) -> Value {
    structure(&[
        (
            "Code",
            Value::Enum {
                value: code,
                name: Some(format!("MathOpCode_{name}")),
            },
        ),
        ("Result", Value::UInt32(result)),
        ("Param1", Value::UInt32(p1)),
        ("Param2", Value::UInt32(p2)),
    ])
}
fn math_object(rows: Vec<Value>, calls: Vec<Value>) -> Object {
    Object {
        class: "MathEntityData".into(),
        guid: None,
        fields: fields(&[
            (
                "Assembly",
                structure(&[
                    ("Instructions", Value::Array(rows)),
                    ("FunctionCalls", Value::Array(calls)),
                ]),
            ),
            ("EvaluateOnEvent", Value::Bool(true)),
        ]),
    }
}
#[test]
fn compile_function_arguments_and_return_register_from_installed_fields() {
    let calls = vec![structure(&[(
        "Parameters",
        Value::Array(vec![Value::UInt32(3)]),
    )])];
    let obj = math_object(
        vec![
            instruction("ConstI", 1, 3, 9, 0),
            instruction("Func", 66, 5, 1_358_875_324, 0),
            instruction("Return", 67, 0, 4, 5),
        ],
        calls,
    );
    let Calculation::Math {
        program,
        evaluate_on_event,
    } = compile(&obj).unwrap()
    else {
        panic!()
    };
    assert!(evaluate_on_event);
    assert_eq!(program.evaluate(|_, _| None), Ok(Some(Scalar::Float(9.0))));
}
#[test]
fn reject_unknown_or_mismatched_instructions_calls_and_registers() {
    for bad_instruction in [
        instruction("ConstF", 1, 0, 0, 0),
        instruction("Mystery", 99, 0, 0, 0),
        instruction("ConstI", 1, 32, 0, 0),
        instruction("Func", 66, 0, 0, 0),
        instruction("Return", 67, 0, 8, 0),
    ] {
        assert!(compile(&math_object(vec![bad_instruction], vec![])).is_err());
    }
    let calls = vec![structure(&[(
        "Parameters",
        Value::Array(vec![Value::UInt32(0), Value::UInt32(1)]),
    )])];
    assert!(
        compile(&math_object(
            vec![instruction("Func", 66, 0, 194_586_087, 0)],
            calls
        ))
        .is_err()
    );
    assert!(
        compile(&math_object(
            vec![
                instruction("ConstF", 2, 0, f32::NAN.to_bits(), 0),
                instruction("Return", 67, 0, 4, 0)
            ],
            vec![]
        ))
        .is_err()
    );
    assert!(
        compile(&math_object(
            vec![instruction("ConstI", 1, 0, 0, 0); math::MAX_INSTRUCTIONS + 1],
            vec![]
        ))
        .is_err()
    );
}
fn vector(x: f32, y: f32, z: f32) -> Value {
    structure(&[
        ("x", Value::Float32(x)),
        ("y", Value::Float32(y)),
        ("z", Value::Float32(z)),
    ])
}
fn curve_object(count: f32, mode: f32) -> Object {
    Object {
        class: "CurveEntityData".into(),
        guid: None,
        fields: fields(&[
            (
                "CurveEntityTrait",
                Value::Enum {
                    value: 0,
                    name: Some("CURVE_ENTITY_TRAIT_DEFAULT".into()),
                },
            ),
            (
                "ReactionCurve",
                Value::Array(vec![
                    vector(0.0, 0.0, mode),
                    vector(1.0, 100.0, count),
                    vector(0.0, 0.0, 0.0),
                    vector(1.0, 1.0, 6.0),
                ]),
            ),
            ("InputScale", Value::Float32(1.0)),
            ("OutputScale", Value::Float32(2.0)),
            ("Input", Value::Float32(0.25)),
            ("ApplyReactionCurve", Value::Bool(true)),
        ]),
    }
}
#[test]
fn installed_packed_curve_selects_cubic_or_linear_and_validates_count() {
    for (mode, expected) in [(0.0, 25.0), (1.0, 100.0)] {
        let Calculation::Curve {
            curve,
            apply,
            default_input,
        } = compile(&curve_object(2.0, mode)).unwrap()
        else {
            panic!()
        };
        assert!(apply);
        assert_eq!(default_input, 0.25);
        assert_eq!(curve.evaluate(0.5), Ok(expected));
    }
    for count in [1.0, 3.0, 2.5, f32::NAN] {
        assert!(compile(&curve_object(count, 0.0)).is_err());
    }
    let mut obj = curve_object(2.0, 0.0);
    obj.class = "UnknownEntityData".into();
    assert!(compile(&obj).is_err());
}
