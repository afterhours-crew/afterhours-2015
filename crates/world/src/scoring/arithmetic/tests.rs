// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::boolean_fold::input_port;
use super::*;
fn output(value: f32) -> Option<Effect> {
    Some(Effect::Publish(OUT, Value::Float(value)))
}
#[test]
fn ordered_fold_uses_zero_for_unbound_and_waits_for_bound_inputs() {
    let mut node = Node::new(
        vec![Operation::Add, Operation::Multiply, Operation::Subtract],
        &[0, 2, 3],
    )
    .unwrap();
    assert_eq!(
        node.apply(Action::PropertyChanged(IN0), |_| panic!("inactive")),
        Ok(None)
    );
    let mut reads = vec![];
    assert_eq!(
        node.apply(Action::Activate, |p| {
            reads.push(p);
            Ok(Some(Value::Float(if p == IN0 {
                2.0
            } else if p == input_port(2)? {
                3.0
            } else {
                1.0
            })))
        }),
        Ok(output(5.0))
    );
    assert_eq!(reads, [IN0, input_port(2).unwrap(), input_port(3).unwrap()]);
    assert_eq!(
        node.apply(Action::PropertyChanged(IN0), |p| {
            if p == IN0 {
                Ok(None)
            } else {
                panic!("unready stops the fold")
            }
        }),
        Ok(None)
    );
    for _ in 0..2 {
        assert_eq!(
            node.apply(Action::PropertyChanged(IN0), |_| Ok(Some(Value::Float(
                2.0
            )))),
            Ok(output(2.0))
        );
    }
    assert_eq!(node.apply(Action::Event(1), |_| panic!("event")), Ok(None));
}
#[test]
fn small_divisors_multiply_by_positive_million_at_strict_threshold() {
    for denominator in [0.0, -0.0, DIVISOR_EPSILON / 2.0, -DIVISOR_EPSILON / 2.0] {
        assert_eq!(
            Operation::Divide.evaluate(2.0, denominator),
            Ok(2_000_000.0)
        );
        assert_eq!(
            Operation::Divide.evaluate(-2.0, denominator),
            Ok(-2_000_000.0)
        );
    }
    for denominator in [DIVISOR_EPSILON, -DIVISOR_EPSILON] {
        assert_eq!(
            Operation::Divide.evaluate(2.0, denominator),
            Ok(2.0 / denominator)
        );
    }
}
#[test]
fn extrema_choose_rhs_bits_on_equality_and_fold_rounds_each_operation() {
    for op in [Operation::Min, Operation::Max] {
        assert_eq!(
            op.evaluate(0.0, -0.0).unwrap().to_bits(),
            (-0.0f32).to_bits()
        );
        assert_eq!(op.evaluate(-0.0, 0.0).unwrap().to_bits(), 0);
    }
    assert_eq!(Operation::Min.evaluate(2.0, -1.0), Ok(-1.0));
    assert_eq!(Operation::Max.evaluate(-1.0, 2.0), Ok(2.0));
    let mut node = Node::new(vec![Operation::Add, Operation::Subtract], &[0, 1, 2]).unwrap();
    assert_eq!(
        node.apply(Action::Activate, |p| Ok(Some(Value::Float(
            if p == input_port(1)? {
                1.0
            } else {
                16_777_216.0
            }
        )))),
        Ok(output(0.0))
    );
}
#[test]
fn bounds_nonfinite_and_type_failures_are_atomic() {
    assert!(Node::new(vec![Operation::Add; MAX_OPERATIONS + 1], &[]).is_err());
    assert!(Node::new(vec![], &[1]).is_err());
    assert!(Node::new(vec![], &[0, 0]).is_err());
    let mut empty = Node::new(vec![], &[]).unwrap();
    assert_eq!(
        empty.apply(Action::Activate, |_| panic!("unbound")),
        Ok(output(0.0))
    );
    let mut node = Node::new(vec![Operation::Multiply], &[0, 1]).unwrap();
    for value in [
        Value::Float(f32::MAX),
        Value::Float(f32::NAN),
        Value::Integer(1),
    ] {
        let before = node.clone();
        assert!(
            node.apply(Action::Activate, |_| Ok(Some(value.clone())))
                .is_err()
        );
        assert_eq!(node, before);
    }
    let mut maximum = Node::new(vec![Operation::Add; MAX_OPERATIONS], &[256]).unwrap();
    assert_eq!(
        maximum.apply(Action::Activate, |p| {
            assert_eq!(p, input_port(256).unwrap());
            Ok(Some(Value::Float(7.0)))
        }),
        Ok(output(7.0))
    );
}
