// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{cell, inputs, node, port};
use super::*;
use crate::scoring::{properties::Registration, scale_clamp as scale};
use events::native::{Call, Callbacks, Host, Invocation, Request};

#[derive(Clone, Debug, Default, PartialEq)]
struct OwnedProperties {
    value: Option<Value>,
    seen: Vec<Value>,
    reenter: bool,
    fail: bool,
    listeners: Vec<u16>,
}
impl Host for OwnedProperties {
    fn invoke(&mut self, r: Request, graphs: &mut dyn Callbacks) -> Result<(), Error> {
        match r.call {
            Call::ParticipantObserve {
                property,
                participant,
                seed,
                ..
            } => {
                self.listeners.push(r.node);
                if seed && let Some(value) = self.value.clone() {
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: r.instance,
                            operation: Operation::Import(events::native::Import::Accessor {
                                node: r.node,
                                write: participant_properties::Write {
                                    participant,
                                    property,
                                    value
                                },
                            }),
                        }
                    )?);
                }
            }
            Call::ParticipantUnobserve => self.listeners.retain(|node| *node != r.node),
            Call::ParticipantWrite(write) => {
                if self.value.as_ref() == Some(&write.value) {
                    return Ok(());
                }
                self.value = Some(write.value.clone());
                for node in self.listeners.clone() {
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: r.instance,
                            operation: Operation::Import(events::native::Import::Accessor {
                                node,
                                write: write.clone()
                            })
                        }
                    )?);
                }
            }
            Call::Property { field: 7, .. } => {
                let value = graphs
                    .read(r.instance, port(r.node, 7))?
                    .flatten()
                    .ok_or(Error::MissingInput)?;
                assert_eq!(Some(&value), self.value.as_ref());
                self.seen.push(value);
                if self.reenter {
                    self.reenter = false;
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: r.instance,
                            operation: Operation::Publish {
                                port: port(0, nodes::FLOAT_VALUE),
                                value: Value::Integer(3)
                            }
                        }
                    )?);
                }
                if self.fail {
                    return Err(Error::InvalidGraph);
                }
            }
            Call::Bind | Call::Activate | Call::Retire => {}
            _ => return Err(Error::InvalidGraph),
        }
        Ok(())
    }
}
fn property_fixture(ready: bool) -> (Session, participant_properties::Property) {
    let property = participant_properties::Property {
        channel: "synthetic/event".into(),
        port: 19,
    };
    let mut writer = node(
        0,
        Behavior::ParticipantProperty {
            property: property.clone(),
            kind: Kind::Integer,
            write: true,
        },
    );
    writer.defaults = vec![(nodes::FLOAT_VALUE, Value::Integer(2))];
    let reader = node(
        1,
        Behavior::ParticipantProperty {
            property: property.clone(),
            kind: Kind::Integer,
            write: false,
        },
    );
    let session = Session::new(Arc::new(Program {
        nodes: vec![writer, reader, node(2, Behavior::Native)],
        cells: vec![
            super::super::super::properties::Definition {
                initial: ready.then_some(Value::Integer(2)),
                ..cell(Kind::Integer, &[port(0, nodes::FLOAT_VALUE)])
            },
            cell(Kind::Integer, &[port(0, nodes::OUTPUT_VALUE), port(2, 7)]),
            cell(Kind::Integer, &[port(1, nodes::OUTPUT_VALUE)]),
        ],
        connections: vec![],
        startup: vec![
            Step::Register(Registration::Notify(port(0, nodes::FLOAT_VALUE))),
            Step::Register(Registration::Notify(port(2, 7))),
            Step::Bind(0),
            Step::Bind(1),
            Step::Bind(2),
            Step::Activate(0),
            Step::Activate(1),
            Step::Activate(2),
        ],
        unmodeled: vec![],
    }))
    .unwrap();
    (session, property)
}

#[test]
fn hosted_property_write_updates_owner_before_callbacks_and_late_binding_reads_nested_value() {
    let (mut session, property) = property_fixture(true);
    let mut host = OwnedProperties {
        reenter: true,
        ..Default::default()
    };
    let output = session
        .hosted(
            Action::Start,
            &inputs(),
            Scope {
                instance: 11,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert_eq!(host.seen, [Value::Integer(2), Value::Integer(3)]);
    assert_eq!(host.value, Some(Value::Integer(3)));
    assert_eq!(
        session.participant_value(inputs().participant, &property, Kind::Integer),
        host.value
    );
    assert_eq!(
        session.read(port(1, nodes::OUTPUT_VALUE)).unwrap(),
        host.value
    );
    assert!(
        output.participant_properties.is_empty(),
        "hosted writes must not be replayed from output"
    );
    session
        .hosted(
            Action::Graph(Operation::Publish {
                port: port(0, nodes::FLOAT_VALUE),
                value: Value::Integer(3),
            }),
            &inputs(),
            Scope {
                instance: 11,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert_eq!(host.seen.len(), 2, "equal owner writes do not notify");
}

#[test]
fn hosted_import_failure_discards_graph_and_caller_can_discard_owner_candidate() {
    let (mut session, property) = property_fixture(true);
    let host = OwnedProperties::default();
    let mut candidate = OwnedProperties {
        fail: true,
        ..host.clone()
    };
    assert!(
        session
            .hosted(
                Action::Start,
                &inputs(),
                Scope {
                    instance: 11,
                    host: &mut candidate,
                    upstream: None
                }
            )
            .is_err()
    );
    assert!(!session.started());
    assert_eq!(
        session.participant_value(inputs().participant, &property, Kind::Integer),
        None
    );
    assert_eq!(session.read(port(0, nodes::OUTPUT_VALUE)).unwrap(), None);
    assert_eq!(host, OwnedProperties::default());
}

#[derive(Clone, Default)]
struct Native {
    values: Vec<f32>,
    reentered: bool,
    invalid: bool,
}

#[test]
fn connected_unready_accessor_seeds_from_store_without_writing_its_fallback() {
    for initial in [None, Some(Value::Integer(9))] {
        let (mut session, property) = property_fixture(false);
        let mut host = OwnedProperties {
            value: initial.clone(),
            ..Default::default()
        };
        let output = session
            .hosted(
                Action::Start,
                &inputs(),
                Scope {
                    instance: 11,
                    host: &mut host,
                    upstream: None,
                },
            )
            .unwrap();
        assert!(output.participant_properties.is_empty());
        assert_eq!(host.value, initial);
        assert_eq!(host.seen, initial.clone().into_iter().collect::<Vec<_>>());
        assert_eq!(session.read(port(0, nodes::FLOAT_VALUE)).unwrap(), None);
        assert_eq!(session.read(port(0, nodes::OUTPUT_VALUE)).unwrap(), initial);
        assert_eq!(session.read(port(1, nodes::OUTPUT_VALUE)).unwrap(), initial);
        session
            .hosted(
                Action::Graph(Operation::Publish {
                    port: port(0, nodes::FLOAT_VALUE),
                    value: Value::Integer(4),
                }),
                &inputs(),
                Scope {
                    instance: 11,
                    host: &mut host,
                    upstream: None,
                },
            )
            .unwrap();
        assert_eq!(host.value, Some(Value::Integer(4)));
        assert_eq!(
            session.participant_value(inputs().participant, &property, Kind::Integer),
            Some(Value::Integer(4))
        );
    }
}
impl Host for Native {
    fn invoke(&mut self, r: Request, graphs: &mut dyn Callbacks) -> Result<(), Error> {
        match (r.node, r.call) {
            (0, Call::Bind) => {
                let value = if self.invalid {
                    Value::Boolean(true)
                } else {
                    Value::Float(5.)
                };
                assert!(graphs.apply(
                    self,
                    Invocation {
                        instance: r.instance,
                        operation: Operation::Publish {
                            port: port(0, 1),
                            value
                        }
                    }
                )?);
            }
            (2, Call::Property { field: 2, .. }) => {
                let Some(Some(Value::Float(value))) = graphs.read(r.instance, port(2, 2))? else {
                    return Err(Error::MissingInput);
                };
                self.values.push(value);
                if !self.reentered {
                    self.reentered = true;
                    assert!(graphs.apply(
                        self,
                        Invocation {
                            instance: r.instance,
                            operation: Operation::Publish {
                                port: port(0, 1),
                                value: Value::Float(6.)
                            }
                        }
                    )?);
                }
            }
            (_, Call::Bind | Call::Activate) => {}
            _ => return Err(Error::InvalidGraph),
        }
        Ok(())
    }
}
fn fixture() -> Session {
    let mut clamp = node(1, Behavior::ScaleClamp);
    clamp.defaults = scale::INPUTS
        .into_iter()
        .zip([0., 0., 10., 10., 20.].into_iter().map(Value::Float))
        .collect();
    Session::new(Arc::new(Program {
        nodes: vec![node(0, Behavior::Native), clamp, node(2, Behavior::Native)],
        cells: vec![
            cell(Kind::Float, &[port(0, 1), port(1, scale::INPUT)]),
            cell(Kind::Float, &[port(1, scale::OUT), port(2, 2)]),
        ],
        connections: vec![],
        startup: vec![
            Step::Register(Registration::Notify(port(1, scale::INPUT))),
            Step::Register(Registration::Notify(port(2, 2))),
            Step::Bind(0),
            Step::Bind(1),
            Step::Bind(2),
            Step::Activate(0),
            Step::Activate(1),
            Step::Activate(2),
        ],
        unmodeled: vec![],
    }))
    .unwrap()
}

#[test]
fn hosted_interpreter_uses_the_same_ordinary_nodes_and_synchronous_property_callbacks() {
    let mut session = fixture();
    let mut host = Native::default();
    session
        .hosted(
            Action::Start,
            &inputs(),
            Scope {
                instance: 7,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert_eq!(host.values, [15., 16.]);
    assert_eq!(
        session.read(port(1, scale::OUT)),
        Ok(Some(Value::Float(16.)))
    );
    session
        .hosted(
            Action::Graph(Operation::Publish {
                port: port(0, 1),
                value: Value::Float(7.),
            }),
            &inputs(),
            Scope {
                instance: 7,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert_eq!(host.values, [15., 16., 17.]);
}

#[test]
fn native_nodes_require_a_host_and_failed_hosted_start_preserves_the_session() {
    let mut session = fixture();
    assert_eq!(session.start(&inputs()), Err(Error::MissingInput));
    assert!(!session.started());
    assert_eq!(session.failed_step(), Some(2));
    let mut host = Native {
        invalid: true,
        ..Default::default()
    };
    assert!(
        session
            .hosted(
                Action::Start,
                &inputs(),
                Scope {
                    instance: 7,
                    host: &mut host,
                    upstream: None
                }
            )
            .is_err()
    );
    assert!(!session.started());
    assert_eq!(session.read(port(0, 1)), Ok(None));
    host.invalid = false;
    session
        .hosted(
            Action::Start,
            &inputs(),
            Scope {
                instance: 7,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert!(session.started());
}
