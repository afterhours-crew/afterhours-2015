// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    events::{self, Dispatch, Endpoint, Graph},
    nodes,
    properties::{Definition as Cell, Port, Store},
};

#[derive(Clone, Debug, PartialEq)]
struct State {
    gate: Gate,
    props: Store,
    inputs: [Option<Value>; 6],
    output: Vec<(u32, Value)>,
    mode: u8,
}
fn port(channel: Channel) -> Port {
    Port {
        node: 0,
        field: channel.output(),
    }
}
fn setup(open: bool, write_on_open: bool, channels: &[Channel]) -> (State, Graph) {
    (
        State {
            gate: Gate::new(
                Definition {
                    open,
                    write_on_open,
                },
                channels,
            )
            .unwrap(),
            props: Store::new(
                1,
                channels
                    .iter()
                    .map(|c| Cell {
                        kind: c.kind(),
                        initial: None,
                        ports: vec![port(*c)],
                        listeners: vec![port(*c)],
                    })
                    .collect(),
            )
            .unwrap(),
            inputs: [
                Some(Value::Boolean(true)),
                Some(Value::Integer(9)),
                Some(Value::Float(3.5)),
                Some(Value::Vec3([1.0, 2.0, 3.0])),
                Some(Value::Vec4([4.0, 3.0, 2.0, 1.0])),
                Some(Value::Transform([[1.0, 2.0, 3.0]; 4])),
            ],
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
        self.output
            .push((p.field, self.props.read(p)?.ok_or(Error::MissingInput)?));
        if p.field == Channel::Boolean.output() {
            if self.mode == 1 {
                d.property_gate(self, 0, Action::Event(nodes::CLOSE))?;
                self.inputs[1] = Some(Value::Integer(71));
            } else if self.mode == 2 {
                self.inputs[1] = Some(Value::Float(1.0));
            } else if self.mode == 3 {
                let _ = d.property_gate(self, 99, Action::Event(nodes::OPEN));
            } else if self.mode == 4 {
                d.property_gate(self, 0, Action::PropertyChanged(Channel::Boolean.input()))?;
            }
        }
        Ok(())
    }
}
impl Handler for State {
    fn property_gate(&mut self, node: u16) -> Result<&mut Gate, Error> {
        if node != 0 {
            return Err(Error::UnknownNode);
        }
        Ok(&mut self.gate)
    }
    fn property_gate_input(&self, _: u16, c: Channel) -> Result<Option<Value>, Error> {
        Ok(self.inputs[c as usize].clone())
    }
}
fn apply(s: &State, g: &Graph, a: Action) -> State {
    g.transaction(s, |s, d| d.property_gate(s, 0, a)).unwrap().0
}
#[test]
fn fresh_open_flushes_all_channels_in_order_even_without_write_on_open() {
    let (s, g) = setup(true, false, &CHANNELS);
    let s = apply(&s, &g, Action::Initialize);
    assert_eq!(
        s.output,
        CHANNELS
            .into_iter()
            .map(|c| (c.output(), s.inputs[c as usize].clone().unwrap()))
            .collect::<Vec<_>>()
    );
    let closed = apply(&s, &g, Action::Event(nodes::CLOSE));
    let reopened = apply(&closed, &g, Action::Event(nodes::OPEN));
    assert_eq!(reopened.output, s.output);
    assert!(reopened.gate.is_open());
    assert_eq!(
        g.transaction(&s, |s, d| d.property_gate(s, 0, Action::Initialize)),
        Err(Error::InvalidGraph)
    );
}
#[test]
fn closed_unready_repeated_open_and_equal_changes_preserve_contract() {
    let (mut s, g) = setup(false, true, &CHANNELS);
    s.inputs[1] = None;
    let s = apply(&s, &g, Action::Initialize);
    assert!(
        apply(&s, &g, Action::PropertyChanged(Channel::Float.input()))
            .output
            .is_empty()
    );
    let opened = apply(&s, &g, Action::Event(nodes::OPEN));
    assert_eq!(opened.output.len(), 5);
    assert_eq!(opened.props.read(port(Channel::Integer)), Ok(None));
    let repeated = apply(&opened, &g, Action::Event(nodes::OPEN));
    assert_eq!(opened.output, repeated.output);
    let changed = apply(
        &repeated,
        &g,
        Action::PropertyChanged(Channel::Float.input()),
    );
    assert_eq!(
        changed.output.last(),
        Some(&(Channel::Float.output(), Value::Float(3.5)))
    );
    assert_eq!(changed.output.len(), 6);
    let closed = apply(&changed, &g, Action::Event(42));
    assert!(!closed.gate.is_open());
    assert_eq!(closed.output, changed.output);
}
#[test]
fn later_inputs_are_current_and_nested_close_does_not_cancel_flush() {
    let (mut s, g) = setup(false, true, &[Channel::Boolean, Channel::Integer]);
    s.mode = 1;
    let s = apply(&s, &g, Action::Initialize);
    let opened = apply(&s, &g, Action::Event(nodes::OPEN));
    assert!(!opened.gate.is_open());
    assert_eq!(
        opened.output,
        [
            (Channel::Boolean.output(), Value::Boolean(true)),
            (Channel::Integer.output(), Value::Integer(71))
        ]
    );
    assert!(s.output.is_empty());
}
#[test]
fn failures_after_publication_poison_and_discard_entire_candidate() {
    for (mode, error) in [
        (2, Error::InvalidGraph),
        (3, Error::UnknownNode),
        (4, Error::Bound),
    ] {
        let (mut s, g) = setup(false, true, &[Channel::Boolean, Channel::Integer]);
        s.mode = mode;
        let s = apply(&s, &g, Action::Initialize);
        assert_eq!(
            g.transaction(&s, |s, d| d.property_gate(s, 0, Action::Event(nodes::OPEN))),
            Err(error)
        );
        assert!(!s.gate.is_open());
        assert!(s.output.is_empty());
        assert_eq!(s.props.read(port(Channel::Boolean)), Ok(None));
    }
}
#[test]
fn only_bound_channels_are_read_and_vector_types_remain_distinct() {
    let (mut s, g) = setup(true, true, &[Channel::Vec3]);
    s.inputs[0] = Some(Value::Float(f32::NAN)); // unused channel is not read
    let s = apply(&s, &g, Action::Initialize);
    assert_eq!(
        s.output,
        [(Channel::Vec3.output(), Value::Vec3([1.0, 2.0, 3.0]))]
    );
    for bad in [
        Value::Vec4([0.0; 4]),
        Value::Vec3([0.0, f32::INFINITY, 0.0]),
    ] {
        let mut invalid = s.clone();
        invalid.inputs[3] = Some(bad);
        assert!(
            g.transaction(&invalid, |s, d| d.property_gate(
                s,
                0,
                Action::PropertyChanged(Channel::Vec3.input())
            ))
            .is_err()
        );
        assert_eq!(
            invalid.props.read(port(Channel::Vec3)),
            Ok(Some(Value::Vec3([1.0, 2.0, 3.0])))
        );
    }
    assert!(
        Gate::new(
            Definition {
                open: false,
                write_on_open: true
            },
            &[Channel::Float, Channel::Float]
        )
        .is_err()
    );
    assert!(nodes::cast_numeric(Value::Vec3([1.0; 3])).is_err());
}
