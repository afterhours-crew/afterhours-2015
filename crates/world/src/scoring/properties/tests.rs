// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::events::{
    Connection, Delivery, Dispatch, Endpoint, Graph, Handler, PropertyHandler, Remote,
};

fn port(node: u16) -> Port {
    Port { node, field: 7 }
}
fn endpoint(node: u16, event: u32) -> Endpoint {
    Endpoint { node, event }
}
fn cell(initial: Option<Value>, listeners: Vec<Port>) -> Definition {
    Definition {
        kind: Kind::Float,
        initial,
        ports: vec![port(0), port(1), port(2)],
        listeners,
    }
}
#[derive(Clone, Debug, PartialEq)]
struct State {
    properties: Store,
    trace: Vec<(Port, bool, f32)>,
    banked: Vec<f32>,
    mode: u8,
}
impl State {
    fn new(initial: Option<Value>, mode: u8) -> Self {
        Self {
            properties: Store::new(3, vec![cell(initial, vec![port(1), port(2)])]).unwrap(),
            trace: vec![],
            banked: vec![],
            mode,
        }
    }
    fn value(&self, target: Port) -> f32 {
        match self.properties.read(target).unwrap().unwrap() {
            Value::Float(value) => value,
            _ => panic!("test property type"),
        }
    }
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        if target == endpoint(2, 9) {
            self.banked.push(self.value(port(2)));
            dispatch.publish(self, port(0), Value::Float(0.0))?;
        }
        Ok(())
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.properties
    }
    fn property_changed(
        &mut self,
        target: Port,
        first: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let value = self.value(target);
        self.trace.push((target, first, value));
        if target == port(1) {
            match self.mode {
                1 if value > 0.0 => dispatch.publish(self, port(0), Value::Float(0.0))?,
                2 if value > 0.0 => dispatch.emit(self, endpoint(1, 8))?,
                3 => {
                    let _ = dispatch.publish(self, port(0), Value::Float(value));
                }
                4 => {
                    let _ = dispatch.publish(self, port(0), Value::Boolean(true));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[test]
fn aliases_read_current_shared_value_after_nested_publication() {
    let original = State::new(None, 1);
    let graph = Graph::new(3, vec![]).unwrap();
    let (next, output) = graph
        .publish(&original, port(0), Value::Float(5.0))
        .unwrap();
    assert!(output.is_empty());
    assert_eq!(
        next.trace,
        [
            (port(1), true, 5.0),
            (port(1), false, 0.0),
            (port(2), false, 0.0),
            (port(2), true, 0.0),
        ]
    );
    assert_eq!(original.properties.read(port(2)), Ok(None));
    assert_eq!(next.properties.read(port(2)), Ok(Some(Value::Float(0.0))));
    assert_eq!(
        next.properties.read(Port {
            node: 2,
            field: 999
        }),
        Err(Error::MissingInput)
    );
    assert!(original.trace.is_empty());
}

#[test]
fn equal_values_and_repeated_registrations_are_not_coalesced() {
    let mut original = State::new(Some(Value::Float(0.0)), 0);
    original.properties = Store::new(
        3,
        vec![cell(
            Some(Value::Float(0.0)),
            vec![port(2), port(1), port(2)],
        )],
    )
    .unwrap();
    let graph = Graph::new(3, vec![]).unwrap();
    for _ in 0..2 {
        let (next, _) = graph
            .publish(&original, port(0), Value::Float(0.0))
            .unwrap();
        original = next;
    }
    assert_eq!(
        original.trace,
        [
            (port(2), false, 0.0),
            (port(1), false, 0.0),
            (port(2), false, 0.0),
            (port(2), false, 0.0),
            (port(1), false, 0.0),
            (port(2), false, 0.0),
        ]
    );
}

#[test]
fn property_event_property_chain_banks_before_reset_and_defers_remote_output() {
    let original = State::new(None, 2);
    let remote = Remote { bus: 0, index: 17 };
    let graph = Graph::new(
        3,
        vec![Connection {
            source: endpoint(1, 8),
            target: endpoint(2, 9),
            delivery: Delivery::NetworkedClientAndServer(remote),
        }],
    )
    .unwrap();
    let (next, output) = graph
        .publish(&original, port(0), Value::Float(125.0))
        .unwrap();
    assert_eq!(next.banked, [125.0]);
    assert_eq!(next.value(port(2)), 0.0);
    assert_eq!(next.trace.last(), Some(&(port(2), true, 0.0)));
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].source, endpoint(1, 8));
    assert_eq!(output[0].connections, [remote]);
    assert!(original.banked.is_empty());
    let other_session = State::new(Some(Value::Float(30.0)), 0);
    assert_eq!(other_session.value(port(1)), 30.0);
}

#[test]
fn swallowed_nested_errors_and_cycles_poison_whole_candidate() {
    let graph = Graph::new(3, vec![]).unwrap();
    for (mode, error) in [(3, Error::Bound), (4, Error::InvalidGraph)] {
        let original = State::new(None, mode);
        let before = original.clone();
        assert_eq!(
            graph.publish(&original, port(0), Value::Float(1.0)),
            Err(error)
        );
        assert_eq!(original, before);
    }
}

#[test]
fn malformed_layout_and_publication_fail_without_mutation() {
    for definition in [
        cell(Some(Value::Integer(0)), vec![]),
        cell(Some(Value::Float(f32::NAN)), vec![]),
        cell(None, vec![Port { node: 1, field: 99 }]),
        Definition {
            ports: vec![],
            ..cell(None, vec![])
        },
        Definition {
            ports: vec![port(0), port(0)],
            ..cell(None, vec![])
        },
    ] {
        assert!(Store::new(3, vec![definition]).is_err());
    }
    assert_eq!(
        Store::new(2, vec![cell(None, vec![])]),
        Err(Error::UnknownNode)
    );
    assert_eq!(
        Store::new(3, vec![cell(None, vec![]), cell(None, vec![])]),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        Store::new(3, vec![cell(None, vec![port(1); MAX_LISTENERS + 1])]),
        Err(Error::Bound)
    );
    let original = State::new(None, 0);
    let graph = Graph::new(3, vec![]).unwrap();
    for (value, error) in [
        (Value::Boolean(false), Error::InvalidGraph),
        (Value::Float(f32::INFINITY), Error::NonFinite),
    ] {
        assert_eq!(graph.publish(&original, port(0), value), Err(error));
        assert_eq!(original.properties.read(port(0)), Ok(None));
    }
    assert_eq!(
        Graph::new(2, vec![])
            .unwrap()
            .publish(&original, port(0), Value::Float(0.0)),
        Err(Error::InvalidGraph)
    );
}

#[test]
fn publication_fanout_consumes_the_shared_dispatch_budget() {
    let mut original = State::new(None, 0);
    original.properties = Store::new(3, vec![cell(None, vec![port(2); MAX_LISTENERS])]).unwrap();
    let graph = Graph::new(3, vec![]).unwrap();
    assert_eq!(
        graph.publish(&original, port(0), Value::Float(1.0)),
        Err(Error::Bound)
    );
    assert!(original.trace.is_empty());
    assert_eq!(original.properties.read(port(2)), Ok(None));
}
