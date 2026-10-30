// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::properties::{Definition, Kind};

const PORT: Port = Port { node: 0, field: 11 };
#[derive(Clone, Debug, PartialEq)]
struct State {
    store: Store,
    trace: Vec<u32>,
}
impl State {
    fn new() -> Self {
        Self {
            store: Store::new(
                1,
                vec![Definition {
                    kind: Kind::Integer,
                    initial: None,
                    ports: vec![PORT],
                    listeners: vec![PORT],
                }],
            )
            .unwrap(),
            trace: vec![],
        }
    }
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, d: &mut Dispatch<'_>) -> Result<(), Error> {
        self.trace.push(target.event);
        if target.event != 3 {
            d.native(
                self,
                target.node,
                Call::Event {
                    event: target.event,
                    context: None,
                },
            )?;
        }
        self.trace.push(target.event + 100);
        Ok(())
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.store
    }
    fn property_changed(
        &mut self,
        port: Port,
        first: bool,
        d: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        d.native(
            self,
            port.node,
            Call::Property {
                field: port.field,
                first,
            },
        )
    }
}

#[derive(Clone, Debug)]
struct Native {
    trace: Vec<u32>,
    child: Option<State>,
    cycle: bool,
}
impl Host for Native {
    fn invoke(&mut self, request: Request, graphs: &mut dyn Callbacks) -> Result<(), Error> {
        match request.call {
            Call::Event { event, .. } => {
                self.trace.push(event);
                if self.cycle {
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: request.instance,
                            operation: Operation::Deliver {
                                target: Endpoint { node: 0, event },
                                context: None
                            }
                        }
                    )?);
                } else if event == 1 {
                    if let Some(mut child) = self.child.take() {
                        Graph::new(1, vec![])?.execute_hosted(
                            &mut child,
                            Scope {
                                instance: 2,
                                host: self,
                                upstream: Some(graphs),
                            },
                            |s, d| s.handle(Endpoint { node: 0, event: 2 }, d),
                        )?;
                        self.child = Some(child);
                    } else {
                        assert!(graphs.apply(
                            self,
                            Invocation {
                                instance: request.instance,
                                operation: Operation::Deliver {
                                    target: Endpoint { node: 0, event: 2 },
                                    context: None
                                }
                            }
                        )?);
                    }
                } else if event == 2 {
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: 1,
                            operation: Operation::Publish {
                                port: PORT,
                                value: Value::Integer(42)
                            }
                        }
                    )?);
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: 1,
                            operation: Operation::Deliver {
                                target: Endpoint { node: 0, event: 3 },
                                context: None
                            }
                        }
                    )?);
                }
                self.trace.push(event + 100);
            }
            Call::Property {
                field: 11,
                first: true,
            } => {
                assert_eq!(graphs.read(1, PORT)?, Some(Some(Value::Integer(42))));
                self.trace.push(11);
            }
            _ => return Err(Error::InvalidGraph),
        }
        Ok(())
    }
}

#[test]
fn native_callbacks_reenter_the_current_graph_before_the_native_and_graph_callers_continue() {
    let original = State::new();
    let mut state = original.clone();
    let mut host = Native {
        trace: vec![],
        child: None,
        cycle: false,
    };
    let remote = Graph::new(1, vec![])
        .unwrap()
        .execute_hosted(
            &mut state,
            Scope {
                instance: 1,
                host: &mut host,
                upstream: None,
            },
            |s, d| s.handle(Endpoint { node: 0, event: 1 }, d),
        )
        .unwrap();
    assert!(remote.is_empty());
    assert_eq!(state.trace, [1, 2, 3, 103, 102, 101]);
    assert_eq!(host.trace, [1, 2, 11, 102, 101]);
    assert_eq!(state.store.read(PORT), Ok(Some(Value::Integer(42))));
    assert_eq!(original.store.read(PORT), Ok(None));
}

#[test]
fn another_instance_can_call_back_into_its_suspended_parent_without_losing_identity() {
    let mut state = State::new();
    let mut host = Native {
        trace: vec![],
        child: Some(State::new()),
        cycle: false,
    };
    Graph::new(1, vec![])
        .unwrap()
        .execute_hosted(
            &mut state,
            Scope {
                instance: 1,
                host: &mut host,
                upstream: None,
            },
            |s, d| s.handle(Endpoint { node: 0, event: 1 }, d),
        )
        .unwrap();
    assert_eq!(state.trace, [1, 3, 103, 101]);
    assert_eq!(host.child.as_ref().unwrap().trace, [2, 102]);
    assert_eq!(host.child.as_ref().unwrap().store.read(PORT), Ok(None));
    assert_eq!(host.trace, [1, 2, 11, 102, 101]);
}

#[test]
fn unbound_hosts_and_recursive_native_callbacks_fail_without_publishing_a_success() {
    let mut state = State::new();
    let graph = Graph::new(1, vec![]).unwrap();
    assert!(matches!(
        graph.transaction(&state, |s, d| s.handle(Endpoint { node: 0, event: 1 }, d)),
        Err(Error::MissingInput)
    ));
    let mut host = Native {
        trace: vec![],
        child: None,
        cycle: true,
    };
    assert_eq!(
        graph.execute_hosted(
            &mut state,
            Scope {
                instance: 1,
                host: &mut host,
                upstream: None
            },
            |s, d| s.handle(Endpoint { node: 0, event: 1 }, d)
        ),
        Err(Error::Bound)
    );
}
