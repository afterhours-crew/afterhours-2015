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
