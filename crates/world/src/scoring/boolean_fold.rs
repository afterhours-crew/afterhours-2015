// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! And/Or activation and lazy evaluation over bound inputs in numeric slot order.
use super::{
    Error,
    nodes::{self, Action, Effect},
    value::Value,
};
use std::collections::BTreeSet;
pub const MAX_INPUTS: usize = 256;
/// In1..InN use the engine's case-sensitive name hash, not consecutive IDs.
pub fn input_port(slot: u16) -> Result<u32, Error> {
    if slot == 0 || slot as usize > MAX_INPUTS {
        return Err(Error::Bound);
    }
    Ok(format!("In{slot}")
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b)))
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    all: bool,
    inputs: Vec<u32>,
    active: bool,
}
impl Node {
    /// Supply connected one-based slots only. Unconnected inputs are omitted;
    /// bound-but-unready values are represented by None during evaluation.
    pub fn new(all: bool, input_count: u16, connected: &[u16]) -> Result<Self, Error> {
        if input_count as usize > MAX_INPUTS || connected.len() > MAX_INPUTS {
            return Err(Error::Bound);
        }
        let mut slots = BTreeSet::new();
        for &slot in connected {
            if slot == 0 || slot > input_count || !slots.insert(slot) {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(Self {
            all,
            inputs: slots
                .into_iter()
                .map(input_port)
                .collect::<Result<_, _>>()?,
            active: false,
        })
    }
    /// Retain returned node state before publishing the effect inside the graph
    /// transaction. No initial false output is synthesized before activation.
    pub fn apply(
        &mut self,
        action: Action,
        mut read: impl FnMut(u32) -> Result<Option<Value>, Error>,
    ) -> Result<Option<Effect>, Error> {
        let mut candidate = self.clone();
        match action {
            Action::Activate => candidate.active = true,
            Action::PropertyChanged(_) if candidate.active => {}
            _ => return Ok(None),
        }
        let output =
            nodes::fold_boolean_by(candidate.all, candidate.inputs.len(), |i| {
                match read(candidate.inputs[i])? {
                    Some(Value::Boolean(v)) => Ok(Some(v)),
                    None => Ok(None),
                    _ => Err(Error::InvalidGraph),
                }
            })?
            .map(|v| Effect::Publish(nodes::OUT, Value::Boolean(v)));
        *self = candidate;
        Ok(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_compacts_connected_slots_and_uses_numeric_order() {
        let mut node = Node::new(true, 12, &[12, 2, 1]).unwrap();
        let mut order = vec![];
        assert_eq!(
            node.apply(Action::PropertyChanged(0), |_| panic!("inactive read"))
                .unwrap(),
            None
        );
        assert_eq!(
            node.apply(Action::Activate, |p| {
                order.push(p);
                Ok(Some(Value::Boolean(true)))
            })
            .unwrap(),
            Some(Effect::Publish(nodes::OUT, Value::Boolean(true)))
        );
        assert_eq!(
            order,
            [
                input_port(1).unwrap(),
                input_port(2).unwrap(),
                input_port(12).unwrap()
            ]
        );
        assert_eq!(input_port(1), Ok(193450867));
        assert_eq!(input_port(2), Ok(193450864));
    }
    #[test]
    fn unready_and_short_circuit_inputs_are_read_lazily() {
        for (all, decisive) in [(true, false), (false, true)] {
            let mut n = Node::new(all, 2, &[1, 2]).unwrap();
            assert_eq!(
                n.apply(Action::Activate, |p| if p == input_port(1).unwrap() {
                    Ok(Some(Value::Boolean(decisive)))
                } else {
                    panic!("short-circuited input read")
                })
                .unwrap(),
                Some(Effect::Publish(nodes::OUT, Value::Boolean(decisive)))
            );
        }
        let mut and = Node::new(true, 2, &[1, 2]).unwrap();
        assert_eq!(
            and.apply(Action::Activate, |p| if p == input_port(1).unwrap() {
                Ok(None)
            } else {
                panic!("unready And stops")
            })
            .unwrap(),
            None
        );
        let mut or = Node::new(false, 2, &[1, 2]).unwrap();
        assert_eq!(
            or.apply(Action::Activate, |p| Ok(if p == input_port(1).unwrap() {
                None
            } else {
                Some(Value::Boolean(true))
            }))
            .unwrap(),
            Some(Effect::Publish(nodes::OUT, Value::Boolean(true)))
        );
    }
    #[test]
    fn empty_identity_equal_publications_and_failed_activation_preserve_state() {
        for all in [true, false] {
            let mut n = Node::new(all, 3, &[]).unwrap();
            for a in [
                Action::Activate,
                Action::PropertyChanged(0),
                Action::PropertyChanged(0),
            ] {
                assert_eq!(
                    n.apply(a, |_| panic!("unconnected input read")).unwrap(),
                    Some(Effect::Publish(nodes::OUT, Value::Boolean(all)))
                );
            }
        }
        let mut n = Node::new(true, 1, &[1]).unwrap();
        let before = n.clone();
        assert_eq!(
            n.apply(Action::Activate, |_| Ok(Some(Value::Integer(1)))),
            Err(Error::InvalidGraph)
        );
        assert_eq!(n, before);
        assert!(Node::new(true, 1, &[2]).is_err());
        assert!(Node::new(true, 2, &[1, 1]).is_err());
        assert!(Node::new(true, 257, &[]).is_err());
    }
}
