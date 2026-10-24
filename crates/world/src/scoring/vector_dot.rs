// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! VectorMathOp Dot. Input changes evaluate only after activation.
use super::{Error, finite, nodes::Action, value::Value};
pub const LEFT: u32 = 2_412_549_647;
pub const RIGHT: u32 = 2_301_908_460;
pub const OUT: u32 = 2_188_532_932;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    active: bool,
}
impl Node {
    pub fn apply(
        &mut self,
        action: Action,
        mut read: impl FnMut(u32) -> Result<Option<Value>, Error>,
    ) -> Result<Option<f32>, Error> {
        let active = match action {
            Action::Activate => true,
            Action::PropertyChanged(_) if self.active => true,
            _ => return Ok(None),
        };
        let mut vector = |field| match read(field)? {
            Some(Value::Vec3(v)) => {
                for x in v {
                    finite(x)?;
                }
                Ok(Some(v))
            }
            None => Ok(None),
            _ => Err(Error::InvalidGraph),
        };
        let value = match vector(LEFT)? {
            Some(a) => match vector(RIGHT)? {
                Some(b) => Some(finite((a[1] * b[1] + a[0] * b[0]) + a[2] * b[2])?),
                None => None,
            },
            None => None,
        };
        self.active = active;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dot_waits_for_activation_and_both_inputs_and_does_not_normalize() {
        let mut n = Node::default();
        assert_eq!(
            n.apply(Action::PropertyChanged(LEFT), |_| panic!()),
            Ok(None)
        );
        assert_eq!(n.apply(Action::Activate, |_| Ok(None)), Ok(None));
        let read = |p| {
            Ok(Some(Value::Vec3(if p == LEFT {
                [2., 3., 4.]
            } else {
                [5., 6., 7.]
            })))
        };
        assert_eq!(n.apply(Action::PropertyChanged(RIGHT), read), Ok(Some(56.)));
        assert_eq!(n.apply(Action::Event(0), |_| panic!()), Ok(None));
        let before = n.clone();
        assert!(
            n.apply(Action::Activate, |_| Ok(Some(Value::Vec3([f32::MAX; 3]))))
                .is_err()
        );
        assert_eq!(n, before);
        assert!(
            n.apply(Action::Activate, |_| Ok(Some(Value::Float(1.))))
                .is_err()
        );
    }
}
