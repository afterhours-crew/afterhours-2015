// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    events::{self, Dispatch, Endpoint, Graph, PropertyHandler},
    properties::{Definition, Kind, Port, Store},
};
#[derive(Clone, Debug, PartialEq)]
struct State {
    node: Node,
    props: Store,
    input: Option<Value>,
    output: Vec<(u32, Value)>,
    mode: u8,
}
fn port(field: u32) -> Port {
    Port { node: 0, field }
}
fn setup(length_bound: bool, normal_bound: bool) -> (State, Graph) {
    let cells = [
        (length_bound, LENGTH, Kind::Float),
        (normal_bound, NORMAL, Kind::Vec3),
    ]
    .into_iter()
    .filter(|(bound, _, _)| *bound)
    .map(|(_, field, kind)| Definition {
        kind,
        initial: None,
        ports: vec![port(field)],
        listeners: vec![port(field)],
    })
    .collect();
    (
        State {
            node: Node::new(length_bound, normal_bound),
            props: Store::new(1, cells).unwrap(),
            input: Some(Value::Vec3([3.0, 4.0, 0.0])),
            output: vec![],
            mode: 0,
        },
        Graph::new(1, vec![]).unwrap(),
    )
}
impl events::Handler for State {
    fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        Ok(())
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.props
    }
    fn property_changed(&mut self, p: Port, _: bool, d: &mut Dispatch<'_>) -> Result<(), Error> {
        self.output.push((p.field, self.props.read(p)?.unwrap()));
        if p.field == LENGTH {
            match self.mode {
                1 => self.input = Some(Value::Vec3([10.0, 0.0, 0.0])),
                2 => {
                    self.input = Some(Value::Vec3([0.0, 0.0, 10.0]));
                    self.mode = 0;
                    d.vector_length(self, 0, Action::PropertyChanged(INPUT))?;
                }
                3 => self.input = Some(Value::Vec3([f32::NAN, 0.0, 0.0])),
                4 => {
                    let _ = d.vector_length(self, 2, Action::Activate);
                }
                5 => d.vector_length(self, 0, Action::PropertyChanged(INPUT))?,
                _ => {}
            }
        }
        Ok(())
    }
}
impl Handler for State {
    fn vector_length(&mut self, _: u16) -> Result<&mut Node, Error> {
        Ok(&mut self.node)
    }
    fn vector_length_input(&self, _: u16) -> Result<Option<Value>, Error> {
        Ok(self.input)
    }
}
fn apply(s: &State, g: &Graph, action: Action) -> State {
    g.transaction(s, |s, d| d.vector_length(s, 0, action))
        .unwrap()
        .0
}
#[test]
fn activation_readiness_equal_writes_and_bound_outputs() {
    for (l, n) in [(true, true), (true, false), (false, true), (false, false)] {
        let (mut s, g) = setup(l, n);
        assert!(
            apply(&s, &g, Action::PropertyChanged(INPUT))
                .output
                .is_empty()
        );
        s.input = None;
        let mut active = apply(&s, &g, Action::Activate);
        assert!(active.output.is_empty());
        active.input = Some(Value::Vec3([3.0, 4.0, 0.0]));
        let mut expected = vec![];
        if l {
            expected.push((LENGTH, Value::Float(5.0)));
        }
        if n {
            expected.push((NORMAL, Value::Vec3([0.6, 0.8, 0.0])));
        }
        let first = apply(&active, &g, Action::PropertyChanged(INPUT));
        assert_eq!(first.output, expected);
        let second = apply(&first, &g, Action::PropertyChanged(INPUT));
        assert_eq!(second.output, expected.repeat(2));
        assert_eq!(apply(&second, &g, Action::Event(42)), second);
    }
}
#[test]
fn nested_length_callbacks_reread_vector_with_outer_length() {
    let (mut s, g) = setup(true, true);
    s.mode = 1;
    assert_eq!(
        apply(&s, &g, Action::Activate).output,
        [
            (LENGTH, Value::Float(5.0)),
            (NORMAL, Value::Vec3([2.0, 0.0, 0.0]))
        ]
    );
    s.mode = 2;
    assert_eq!(
        apply(&s, &g, Action::Activate).output,
        [
            (LENGTH, Value::Float(5.0)),
            (LENGTH, Value::Float(10.0)),
            (NORMAL, Value::Vec3([0.0, 0.0, 1.0])),
            (NORMAL, Value::Vec3([0.0, 0.0, 2.0])),
        ]
    );
}
#[test]
fn strict_normal_threshold_preserves_short_vector_and_signed_zero() {
    let v = [-0.0, NORMAL_EPSILON, 0.0];
    assert_eq!(length(v), Ok(NORMAL_EPSILON));
    assert_eq!(
        normal(v, NORMAL_EPSILON).unwrap().map(f32::to_bits),
        v.map(f32::to_bits)
    );
    assert_eq!(
        normal([0.0, NORMAL_EPSILON * 2.0, 0.0], NORMAL_EPSILON * 2.0),
        Ok([0.0, 1.0, 0.0])
    );
    assert_eq!(length([f32::MAX, 0.0, 0.0]), Err(Error::NonFinite));
}
#[test]
fn callback_errors_cycles_and_invalid_inputs_roll_back_all_outputs() {
    let (mut s, g) = setup(true, true);
    for (mode, error) in [
        (3, Error::NonFinite),
        (4, Error::UnknownNode),
        (5, Error::Bound),
    ] {
        s.mode = mode;
        let before = s.clone();
        assert_eq!(
            g.transaction(&s, |s, d| d.vector_length(s, 0, Action::Activate)),
            Err(error)
        );
        assert_eq!(s, before);
    }
    s.mode = 0;
    s.input = Some(Value::Float(1.0));
    assert_eq!(
        g.transaction(&s, |s, d| d.vector_length(s, 0, Action::Activate)),
        Err(Error::InvalidGraph)
    );
}
