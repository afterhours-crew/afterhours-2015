// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{properties::Definition, property_cast::Bindings};

fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
#[derive(Clone, Debug, PartialEq)]
struct State {
    store: Store,
    bindings: Bindings,
    trace: Vec<(u32, Value, bool)>,
    feedback: Option<Value>,
    loop_feedback: bool,
    fail: bool,
    swallow: bool,
}
impl Handler for State {
    fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.store
    }
    fn property_changed(
        &mut self,
        p: Port,
        first: bool,
        d: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        if p.node == 0 {
            return d
                .property_cast(self, 0, Action::PropertyChanged(p.field))
                .map(|_| ());
        }
        let value = self.store.read(p)?.ok_or(Error::MissingInput)?;
        self.trace.push((p.field, value, first));
        if p.field == Channel::Float.output() {
            let feedback = if self.loop_feedback {
                Some(Value::Integer(7))
            } else {
                self.feedback.take()
            };
            if let Some(value) = feedback {
                let result = d.publish(self, port(0, Channel::Integer.input()), value);
                if !self.swallow {
                    result?;
                }
            }
        }
        if self.fail && p.field == Channel::Boolean.output() {
            return Err(Error::UnsupportedEvent);
        }
        Ok(())
    }
}
impl CastHandler for State {
    fn property_cast(&self, node: u16) -> Result<Bindings, Error> {
        if node != 0 {
            return Err(Error::UnknownNode);
        }
        Ok(self.bindings)
    }
}
fn state(initial: [Option<Value>; 3], outputs: &[Channel]) -> State {
    let mut cells = Vec::new();
    for (channel, initial) in property_cast::CHANNELS.into_iter().zip(initial) {
        let p = port(0, channel.input());
        cells.push(Definition {
            kind: channel.kind(),
            initial,
            ports: vec![p],
            listeners: vec![p],
        });
    }
    for &channel in outputs {
        let p = port(0, channel.output());
        let listener = port(1, channel.output());
        cells.push(Definition {
            kind: channel.kind(),
            initial: None,
            ports: vec![p, listener],
            listeners: vec![listener],
        });
    }
    State {
        store: Store::new(2, cells).unwrap(),
        bindings: Bindings::new(
            &property_cast::CHANNELS.map(Channel::input),
            &outputs.iter().map(|c| c.output()).collect::<Vec<_>>(),
        )
        .unwrap(),
        trace: vec![],
        feedback: None,
        loop_feedback: false,
        fail: false,
        swallow: false,
    }
}
fn graph() -> Graph {
    Graph::new(2, vec![]).unwrap()
}
fn apply(s: &State, action: Action) -> Result<State, Error> {
    graph()
        .transaction(s, |s, d| d.property_cast(s, 0, action).map(|_| ()))
        .map(|(s, _)| s)
}
fn expected(float: f32, integer: i32, boolean: bool, first: bool) -> Vec<(u32, Value, bool)> {
    vec![
        (Channel::Float.output(), Value::Float(float), first),
        (Channel::Integer.output(), Value::Integer(integer), first),
        (Channel::Boolean.output(), Value::Boolean(boolean), first),
    ]
}

#[test]
fn activation_uses_first_ready_type_and_never_invents_zero_for_unready_inputs() {
    let values = [
        Some(Value::Boolean(false)),
        Some(Value::Integer(7)),
        Some(Value::Float(-2.5)),
    ];
    for (initial, expected) in [
        (values, expected(0.0, 0, false, true)),
        ([None, values[1], values[2]], expected(7.0, 7, true, true)),
        ([None, None, values[2]], expected(-2.5, -2, true, true)),
    ] {
        let s = state(initial, &property_cast::CHANNELS);
        assert_eq!(apply(&s, Action::Activate).unwrap().trace, expected);
    }
    let unready = state([None; 3], &property_cast::CHANNELS);
    assert_eq!(apply(&unready, Action::Activate).unwrap(), unready);
    let mut unbound = state(values, &property_cast::CHANNELS);
    unbound.bindings = Bindings::new(&[], &property_cast::CHANNELS.map(Channel::output)).unwrap();
    assert_eq!(apply(&unbound, Action::Activate).unwrap(), unbound);
}

#[test]
fn input_callbacks_work_before_activation_repeat_and_snapshot_across_nested_casts() {
    let mut s = state(
        [None, Some(Value::Integer(7)), None],
        &property_cast::CHANNELS,
    );
    s.feedback = Some(Value::Integer(9));
    let updated = apply(&s, Action::PropertyChanged(Channel::Integer.input())).unwrap();
    assert_eq!(
        updated.trace,
        vec![
            (Channel::Float.output(), Value::Float(7.0), true),
            (Channel::Float.output(), Value::Float(9.0), false),
            (Channel::Integer.output(), Value::Integer(9), true),
            (Channel::Boolean.output(), Value::Boolean(true), true),
            (Channel::Integer.output(), Value::Integer(7), false),
            (Channel::Boolean.output(), Value::Boolean(true), false),
        ]
    );
    assert_eq!(
        updated
            .store
            .read(port(0, Channel::Float.output()))
            .unwrap(),
        Some(Value::Float(9.0))
    );
    assert_eq!(
        updated
            .store
            .read(port(0, Channel::Integer.output()))
            .unwrap(),
        Some(Value::Integer(7))
    );
    assert_eq!(
        s.store.read(port(0, Channel::Integer.input())).unwrap(),
        Some(Value::Integer(7))
    );
    let mut repeated = apply(&updated, Action::PropertyChanged(Channel::Integer.input())).unwrap();
    assert_eq!(repeated.trace.split_off(6), expected(9.0, 9, true, false));
}

#[test]
fn optional_outputs_keep_order_and_invalid_or_missing_inputs_fail_atomically() {
    let only_int = state([None, None, Some(Value::Float(-0.0))], &[Channel::Integer]);
    assert_eq!(
        apply(&only_int, Action::Activate).unwrap().trace,
        [(Channel::Integer.output(), Value::Integer(0), true)]
    );
    let empty = state([Some(Value::Boolean(true)), None, None], &[]);
    assert!(apply(&empty, Action::Activate).unwrap().trace.is_empty());
    let missing = state([None; 3], &property_cast::CHANNELS);
    assert_eq!(
        apply(&missing, Action::PropertyChanged(Channel::Float.input())),
        Err(Error::MissingInput)
    );
    assert_eq!(
        apply(&missing, Action::PropertyChanged(123)).unwrap(),
        missing
    );
    let range = state(
        [None, None, Some(Value::Float(2_147_483_648.0))],
        &property_cast::CHANNELS,
    );
    assert_eq!(apply(&range, Action::Activate), Err(Error::ConversionRange));
    assert!(range.trace.is_empty());
    let mut fail = state(
        [None, Some(Value::Integer(3)), None],
        &property_cast::CHANNELS,
    );
    fail.fail = true;
    assert_eq!(apply(&fail, Action::Activate), Err(Error::UnsupportedEvent));
    assert!(fail.trace.is_empty());
    for swallow in [false, true] {
        let mut cycle = fail.clone();
        cycle.fail = false;
        cycle.loop_feedback = true;
        cycle.swallow = swallow;
        assert_eq!(apply(&cycle, Action::Activate), Err(Error::Bound));
        assert!(cycle.trace.is_empty());
    }
}

#[test]
fn string_unknown_duplicate_and_excessive_bindings_are_rejected() {
    for field in [0x4bb0_cd9b, 0, Channel::Float.output()] {
        assert!(Bindings::new(&[field], &[]).is_err());
    }
    for field in [0x0885_f34e, 0, Channel::Float.input()] {
        assert!(Bindings::new(&[], &[field]).is_err());
    }
    assert!(Bindings::new(&[Channel::Float.input(); 2], &[]).is_err());
    assert!(Bindings::new(&[], &[Channel::Float.output(); 2]).is_err());
    assert!(Bindings::new(&[Channel::Float.input(); 4], &[]).is_err());
    assert!(Bindings::new(&[], &[Channel::Float.output(); 4]).is_err());
}
