// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
fn i(result: u8, operation: Operation) -> Instruction {
    Instruction { result, operation }
}
fn ret(register: Register) -> Instruction {
    i(0, Operation::Return(register))
}

#[test]
fn integer_equality_and_boolean_selection_keep_separate_register_banks() {
    for (left, right) in [(i32::MIN, i32::MIN), (i32::MIN, i32::MAX), (1, 1), (2, 1)] {
        let p = Program::new(vec![
            i(0, Operation::Integer(left)),
            i(1, Operation::Integer(right)),
            i(0, Operation::EqualInteger(0, 1)),
            i(1, Operation::Boolean(false)),
            i(2, Operation::Boolean(true)),
            i(
                0,
                Operation::SelectBoolean {
                    condition: 0,
                    yes: 1,
                    no: 2,
                },
            ),
            ret(Register::Boolean(0)),
        ])
        .unwrap();
        assert_eq!(
            p.evaluate(|_, _| None),
            Ok(Some(Value::Boolean(left != right)))
        );
    }
    for operation in [
        Operation::EqualInteger(0, 1),
        Operation::SelectBoolean {
            condition: 0,
            yes: 1,
            no: 2,
        },
    ] {
        assert_eq!(
            Program::new(vec![
                i(0, Operation::Boolean(true)),
                i(1, operation),
                ret(Register::Boolean(1))
            ]),
            Err(Error::UninitializedRegister)
        );
    }
}

#[test]
fn event_conditions_keep_typed_registers_and_inclusive_boundaries() {
    for (left, right, expected) in [
        (1., 2., [false, false, true]),
        (2., 2., [false, true, true]),
        (3., 2., [true, true, false]),
    ] {
        for (op, expected) in [
            Comparison::Greater,
            Comparison::GreaterEqual,
            Comparison::LessEqual,
        ]
        .into_iter()
        .zip(expected)
        {
            let p = Program::new(vec![
                i(0, Operation::Float(left)),
                i(1, Operation::Float(right)),
                i(0, Operation::CompareFloat(op, 0, 1)),
                i(1, Operation::LessFloat(1, 0)),
                i(2, Operation::AndBoolean(0, 1)),
                ret(Register::Boolean(0)),
            ])
            .unwrap();
            assert_eq!(p.evaluate(|_, _| None), Ok(Some(Value::Boolean(expected))));
        }
    }
    assert_eq!(
        Program::new(vec![
            i(0, Operation::Integer(1)),
            i(1, Operation::AndBoolean(0, 0)),
            ret(Register::Boolean(1))
        ]),
        Err(Error::UninitializedRegister)
    );
}

#[test]
fn integer_limits_clamp_order_and_float_min_ties_match_native_operations() {
    for (op, a, b, expected) in [
        (IntegerOp::Subtract, i32::MIN, 1, i32::MAX),
        (IntegerOp::Min, -9, 7, -9),
        (IntegerOp::Max, -9, 7, 7),
    ] {
        let p = Program::new(vec![
            i(0, Operation::Integer(a)),
            i(1, Operation::Integer(b)),
            i(0, Operation::BinaryInteger(op, 0, 1)),
            ret(Register::Integer(0)),
        ])
        .unwrap();
        assert_eq!(p.evaluate(|_, _| None), Ok(Some(Value::Integer(expected))));
    }
    for (value, min, max, expected) in
        [(3, 5, 10, 5), (15, 5, 10, 10), (7, 5, 10, 7), (7, 10, 5, 5)]
    {
        let p = Program::new(vec![
            i(0, Operation::Integer(value)),
            i(1, Operation::Integer(min)),
            i(2, Operation::Integer(max)),
            i(
                0,
                Operation::ClampInteger {
                    value: 0,
                    min: 1,
                    max: 2,
                },
            ),
            ret(Register::Integer(0)),
        ])
        .unwrap();
        assert_eq!(p.evaluate(|_, _| None), Ok(Some(Value::Integer(expected))));
    }
    for (a, b) in [(0., -0.), (-0., 0.)] {
        let p = Program::new(vec![
            i(0, Operation::Float(a)),
            i(1, Operation::Float(b)),
            i(0, Operation::BinaryFloat(FloatOp::Min, 0, 1)),
            ret(Register::Float(0)),
        ])
        .unwrap();
        let Some(Value::Float(result)) = p.evaluate(|_, _| None).unwrap() else {
            panic!()
        };
        assert_eq!(result.to_bits(), b.to_bits());
    }
}

#[test]
fn vector_functions_use_length_and_distance_and_refuse_nonfinite_inputs() {
    for (op, expected) in [
        (Operation::NormVec3(0), 5.),
        (Operation::DistanceVec3(0, 1), 13.),
    ] {
        let p = Program::new(vec![
            i(0, Operation::InputVec3(10)),
            i(1, Operation::InputVec3(11)),
            i(0, op),
            ret(Register::Float(0)),
        ])
        .unwrap();
        assert_eq!(
            p.inputs().collect::<Vec<_>>(),
            [(10, Type::Vec3), (11, Type::Vec3)]
        );
        assert_eq!(
            p.evaluate(|port, kind| {
                assert_eq!(kind, Type::Vec3);
                Some(Value::Vec3(if port == 10 {
                    [3., 4., 0.]
                } else {
                    [3., 4., 13.]
                }))
            }),
            Ok(Some(Value::Float(expected)))
        );
        assert!(
            p.evaluate(|_, _| Some(Value::Vec3([f32::INFINITY, 0., 0.])))
                .is_err()
        );
        assert_eq!(
            p.evaluate(|_, _| Some(Value::Float(1.))),
            Err(Error::MissingInput)
        );
    }
    for (x, expected) in [
        (1., 0.),
        (0., std::f32::consts::FRAC_PI_2),
        (-1., std::f32::consts::PI),
    ] {
        let p = Program::new(vec![
            i(0, Operation::Float(x)),
            i(0, Operation::ArcCos(0)),
            ret(Register::Float(0)),
        ])
        .unwrap();
        assert_eq!(p.evaluate(|_, _| None), Ok(Some(Value::Float(expected))));
    }
    let p = Program::new(vec![
        i(0, Operation::Float(1.01)),
        i(0, Operation::ArcCos(0)),
        ret(Register::Float(0)),
    ])
    .unwrap();
    assert!(p.evaluate(|_, _| None).is_err());
}

#[test]
fn separate_typed_registers_and_overwrites_preserve_condition_operands() {
    let program = Program::new(vec![
        i(0, Operation::InputFloat(100)),
        i(0, Operation::InputInteger(200)),
        i(1, Operation::FloatFromInteger(0)),
        i(0, Operation::LessFloat(0, 1)),
        i(1, Operation::Float(7.0)),
        i(
            2,
            Operation::SelectFloat {
                condition: 0,
                yes: 1,
                no: 0,
            },
        ),
        ret(Register::Float(2)),
    ])
    .unwrap();
    assert_eq!(
        program.inputs().collect::<Vec<_>>(),
        [(100, Type::Float), (200, Type::Integer)]
    );
    assert_eq!(program.output_type(), Type::Float);
    for (x, expected) in [(1.0, 7.0), (8.0, 8.0)] {
        assert_eq!(
            program.evaluate(|port, kind| match (port, kind) {
                (100, Type::Float) => Some(Value::Float(x)),
                (200, Type::Integer) => Some(Value::Integer(5)),
                _ => None,
            }),
            Ok(Some(Value::Float(expected)))
        );
    }
    assert_eq!(program.evaluate(|_, _| None), Err(Error::MissingInput));
}
#[test]
fn native_round_adds_half_and_truncates_including_negative_values() {
    let program = Program::new(vec![
        i(0, Operation::InputFloat(1)),
        i(0, Operation::RoundFloat(0)),
        ret(Register::Float(0)),
    ])
    .unwrap();
    for (x, expected) in [
        (0.49, 0.0),
        (0.5, 1.0),
        (1.5, 2.0),
        (-0.5, 0.0),
        (-1.5, -1.0),
        (-1.8, -1.0),
        (-2.5, -2.0),
    ] {
        assert_eq!(
            program.evaluate(|_, _| Some(Value::Float(x))),
            Ok(Some(Value::Float(expected)))
        );
    }
    assert_eq!(
        program.evaluate(|_, _| Some(Value::Float(2_147_483_648.0))),
        Err(Error::ConversionRange)
    );
}
#[test]
fn division_aborts_without_replacing_a_previous_output_and_calls_are_isolated() {
    let program = Program::new(vec![
        i(0, Operation::InputFloat(1)),
        i(1, Operation::InputFloat(2)),
        i(2, Operation::BinaryFloat(FloatOp::Divide, 0, 1)),
        ret(Register::Float(2)),
    ])
    .unwrap();
    for divisor in [2.0, 0.0, -0.0, 4.0] {
        assert_eq!(
            program.evaluate(|p, _| Some(Value::Float(if p == 1 { 12.0 } else { divisor }))),
            Ok(if divisor == 0.0 {
                None
            } else {
                Some(Value::Float(12.0 / divisor))
            })
        );
    }
    assert_eq!(
        program.evaluate(|_, _| Some(Value::Float(f32::INFINITY))),
        Err(Error::NonFinite)
    );
}
#[test]
fn arithmetic_and_integer_conversion_follow_scalar_operations() {
    let program = Program::new(vec![
        i(0, Operation::Float(9.0)),
        i(1, Operation::Float(0.5)),
        i(2, Operation::BinaryFloat(FloatOp::Power, 0, 1)),
        i(2, Operation::BinaryFloat(FloatOp::Subtract, 2, 1)),
        i(2, Operation::BinaryFloat(FloatOp::Multiply, 2, 0)),
        i(2, Operation::BinaryFloat(FloatOp::Add, 2, 1)),
        ret(Register::Float(2)),
    ])
    .unwrap();
    assert_eq!(program.evaluate(|_, _| None), Ok(Some(Value::Float(23.0))));
    let ints = Program::new(vec![
        i(0, Operation::Integer(i32::MAX)),
        i(1, Operation::Integer(1)),
        i(2, Operation::AddInteger(0, 1)),
        ret(Register::Integer(2)),
    ])
    .unwrap();
    assert_eq!(
        ints.evaluate(|_, _| None),
        Ok(Some(Value::Integer(i32::MIN)))
    );
}
#[test]
fn reject_invalid_register_flow_returns_and_nonfinite_results() {
    assert_eq!(Program::new(vec![]), Err(Error::Bound));
    assert_eq!(
        Program::new(vec![ret(Register::Float(0))]),
        Err(Error::UninitializedRegister)
    );
    assert_eq!(
        Program::new(vec![i(0, Operation::Integer(1)), ret(Register::Float(0))]),
        Err(Error::UninitializedRegister)
    );
    assert_eq!(
        Program::new(vec![
            i(32, Operation::Integer(1)),
            ret(Register::Integer(0))
        ]),
        Err(Error::Bound)
    );
    assert_eq!(
        Program::new(vec![
            i(0, Operation::Integer(1)),
            ret(Register::Integer(32))
        ]),
        Err(Error::Bound)
    );
    assert_eq!(
        Program::new(vec![i(0, Operation::Integer(1))]),
        Err(Error::InvalidProgram)
    );
    assert_eq!(
        Program::new(vec![
            i(0, Operation::Integer(1)),
            ret(Register::Integer(0)),
            ret(Register::Integer(0))
        ]),
        Err(Error::InvalidProgram)
    );
    assert_eq!(
        Program::new(vec![
            i(0, Operation::Float(f32::NAN)),
            ret(Register::Float(0))
        ]),
        Err(Error::NonFinite)
    );
    assert_eq!(
        Program::new(vec![i(0, Operation::Integer(1)); MAX_INSTRUCTIONS + 1]),
        Err(Error::Bound)
    );
    let overflow = Program::new(vec![
        i(0, Operation::Float(f32::MAX)),
        i(1, Operation::Float(2.0)),
        i(0, Operation::BinaryFloat(FloatOp::Multiply, 0, 1)),
        ret(Register::Float(0)),
    ])
    .unwrap();
    assert_eq!(overflow.evaluate(|_, _| None), Err(Error::NonFinite));
}
#[test]
fn native_intf_truncates_toward_zero_and_refuses_out_of_range_values() {
    let program = Program::new(vec![
        i(0, Operation::InputFloat(1)),
        i(0, Operation::IntegerFromFloat(0)),
        ret(Register::Integer(0)),
    ])
    .unwrap();
    for (x, expected) in [(0.99, 0), (1.5, 1), (-1.8, -1), (13500.0, 13500), (-0.5, 0)] {
        assert_eq!(
            program.evaluate(|_, _| Some(Value::Float(x))),
            Ok(Some(Value::Integer(expected)))
        );
    }
    for x in [2_147_483_648.0, -2_147_483_904.0] {
        assert_eq!(
            program.evaluate(|_, _| Some(Value::Float(x))),
            Err(Error::ConversionRange)
        );
    }
    // The installed reward shape: integer to float, multiply, intf (E995).
    let reward = Program::new(vec![
        i(0, Operation::InputInteger(177629)),
        i(0, Operation::FloatFromInteger(0)),
        i(1, Operation::InputFloat(177628)),
        i(0, Operation::BinaryFloat(FloatOp::Multiply, 0, 1)),
        i(0, Operation::IntegerFromFloat(0)),
        ret(Register::Integer(0)),
    ])
    .unwrap();
    assert_eq!(
        reward.evaluate(|port, _| Some(match port {
            177629 => Value::Integer(9000),
            _ => Value::Float(1.5),
        })),
        Ok(Some(Value::Integer(13500)))
    );
    assert_eq!(
        Program::new(vec![
            i(0, Operation::IntegerFromFloat(0)),
            ret(Register::Integer(0))
        ]),
        Err(Error::UninitializedRegister)
    );
}
