// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    IntegerSource, Observer, SceneInputs,
    tests::{identity, ranked},
};
use super::*;
use nfs_world::scoring::{
    events::{Context, Endpoint},
    program::{Behavior, Identity, Node, Program, Step as Startup},
    properties::{Definition, Kind, Registration},
};
use std::sync::Arc;

fn port() -> Port {
    Port { node: 0, field: 7 }
}
fn write(value: i32) -> Step {
    Step::Lifecycle(vec![Command::Integer {
        source: IntegerSource::Local(identity(80)),
        participant: 7,
        value,
    }])
}
fn graph(id: u64, operation: native::Operation) -> Step {
    Step::Graph {
        id,
        action: Action::Graph(operation),
    }
}
fn fixture() -> Instance {
    let session = Session::new(Arc::new(Program {
        nodes: vec![Node {
            identity: Identity {
                asset: "fixture".into(),
                index: 0,
                instances: vec![],
            },
            class: "FixtureNative".into(),
            created: true,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior: Behavior::Native,
        }],
        cells: vec![Definition {
            kind: Kind::Integer,
            initial: None,
            ports: vec![port()],
            listeners: vec![],
        }],
        connections: vec![],
        startup: vec![
            Startup::Register(Registration::Notify(port())),
            Startup::Bind(0),
            Startup::Activate(0),
        ],
        unmodeled: vec![],
    }))
    .unwrap();
    Instance {
        session,
        inputs: Inputs {
            participant: 7,
            vehicle: None,
            others: vec![],
            road: None,
            stats: None,
            now: 0.,
            time_scale: 1.,
        },
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Binding {
    trace: Vec<(usize, i32)>,
    observed: Vec<Value>,
    fail: bool,
    retire: Vec<u64>,
    cycle: bool,
}
impl Driver for Binding {
    fn invoke(
        &mut self,
        request: Request,
        reader: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        Ok(match (request.instance, request.call) {
            (1, native::Call::Activate) => vec![
                Step::Lifecycle(vec![
                    Command::Activate {
                        observer: Observer::Sorter(0),
                        binding: false,
                    },
                    Command::Activate {
                        observer: Observer::Sorter(1),
                        binding: false,
                    },
                ]),
                write(100),
            ],
            (
                2,
                native::Call::Event {
                    event: 9,
                    context: Some(Context::Participant(7)),
                },
            ) => vec![write(200)],
            (1, native::Call::Property { field: 7, .. }) => {
                assert_eq!(
                    reader.integer(&IntegerSource::Local(identity(80)), 7)?,
                    Some(200)
                );
                assert_eq!(reader.ranking(0)?[0].value, 200);
                assert!(reader.ranking(1)?.is_empty());
                self.observed.push(
                    reader
                        .read(1, port())?
                        .ok_or(scoring::Error::MissingInput)?,
                );
                // An unrelated idle graph is readable through the registry.
                assert_eq!(reader.read(3, port())?, None);
                // The child is suspended in the call that reentered this
                // ancestor. Its current candidate remains readable too.
                assert_eq!(reader.read(2, port())?, None);
                if self.fail {
                    return Err(scoring::Error::UnsupportedEvent);
                }
                vec![graph(
                    2,
                    native::Operation::Publish {
                        port: port(),
                        value: Value::Integer(37),
                    },
                )]
            }
            (2, native::Call::Property { field: 7, .. }) => {
                assert_eq!(reader.read(1, port())?, Some(Value::Integer(200)));
                assert_eq!(reader.read(2, port())?, Some(Value::Integer(37)));
                vec![]
            }
            (id, native::Call::Retire) => {
                self.retire.push(id);
                vec![]
            }
            (id, native::Call::Event { event: 10, .. }) if self.cycle => vec![graph(
                id,
                native::Operation::Deliver {
                    target: Endpoint { node: 0, event: 10 },
                    context: None,
                },
            )],
            (_, native::Call::Bind | native::Call::Activate) => vec![],
            _ => return Err(scoring::Error::UnsupportedEvent),
        })
    }
    fn effect(
        &mut self,
        effect: &super::super::Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        let super::super::Effect::Ranking(e) = effect else {
            return Ok(vec![]);
        };
        let value = e.rows[0].value;
        self.trace.push((e.sorter.index, value));
        Ok(if e.sorter == identity(81) && value == 100 {
            vec![graph(
                2,
                native::Operation::Deliver {
                    target: Endpoint { node: 0, event: 9 },
                    context: Some(Context::Participant(7)),
                },
            )]
        } else if e.sorter == identity(81) && value == 200 {
            vec![graph(
                1,
                native::Operation::Publish {
                    port: port(),
                    value: Value::Integer(200),
                },
            )]
        } else {
            vec![]
        })
    }
}
fn runtime(binding: Binding) -> Runtime<Binding> {
    Runtime::new(
        Host::prepare(ranked(), SceneInputs::default()).unwrap(),
        binding,
    )
}
fn setup() -> Vec<Step> {
    vec![
        Step::Install {
            id: 3,
            instance: Box::new(fixture()),
        },
        Step::Install {
            id: 1,
            instance: Box::new(fixture()),
        },
        Step::Install {
            id: 2,
            instance: Box::new(fixture()),
        },
        Step::Graph {
            id: 2,
            action: Action::Start,
        },
        Step::Graph {
            id: 1,
            action: Action::Start,
        },
    ]
}

#[test]
fn two_active_graphs_reenter_the_same_lifecycle_transaction_before_the_next_sorter() {
    let mut runtime = runtime(Binding::default());
    let output = runtime.execute(setup()).unwrap();
    assert_eq!(
        runtime.driver().trace,
        [(81, 100), (81, 200), (82, 200), (82, 100)]
    );
    assert_eq!(runtime.driver().observed, [Value::Integer(200)]);
    assert_eq!(
        runtime.instance(2).unwrap().session.read(port()).unwrap(),
        Some(Value::Integer(37))
    );
    assert_eq!(
        runtime.instance(1).unwrap().session.read(port()).unwrap(),
        Some(Value::Integer(200))
    );
    assert_eq!(
        runtime
            .lifecycle
            .integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(200)
    );
    assert_eq!(runtime.lifecycle.ranking(1).unwrap()[0].value, 100);
    assert_eq!(
        output
            .iter()
            .filter(|p| matches!(p, Publication::Lifecycle(_)))
            .count(),
        4
    );
    runtime
        .execute(vec![Step::Remove { id: 1 }, Step::Remove { id: 2 }])
        .unwrap();
    assert_eq!(runtime.driver().retire, [1, 2]);
    assert!(runtime.instance(1).is_none());
    assert!(runtime.instance(2).is_none());
}

#[test]
fn nested_failure_rolls_back_graphs_host_driver_and_installations_then_can_retry() {
    let mut runtime = runtime(Binding {
        fail: true,
        ..Default::default()
    });
    assert_eq!(
        runtime.execute(setup()).unwrap_err(),
        scoring::Error::UnsupportedEvent
    );
    assert!(runtime.instance(1).is_none());
    assert!(runtime.instance(2).is_none());
    assert_eq!(
        runtime
            .lifecycle
            .integer_value(&IntegerSource::Local(identity(80)), 7),
        None
    );
    assert!(runtime.lifecycle.ranking(0).unwrap().is_empty());
    assert!(runtime.driver().trace.is_empty());
    assert!(runtime.driver().observed.is_empty());
    runtime.driver.fail = false;
    runtime.execute(setup()).unwrap();
    assert_eq!(runtime.driver().observed, [Value::Integer(200)]);
}

#[test]
fn graph_identity_bounds_missing_instances_and_callback_cycles_fail_atomically() {
    let mut runtime = runtime(Binding::default());
    let mut steps = setup();
    steps.push(Step::Install {
        id: 1,
        instance: Box::new(fixture()),
    });
    assert_eq!(
        runtime.execute(steps).unwrap_err(),
        scoring::Error::InvalidGraph
    );
    assert!(runtime.instance(1).is_none());
    runtime.execute(setup()).unwrap();
    assert_eq!(
        runtime.execute(vec![Step::Remove { id: 99 }]).unwrap_err(),
        scoring::Error::UnknownNode
    );
    runtime.driver.cycle = true;
    assert_eq!(
        runtime
            .execute(vec![graph(
                2,
                native::Operation::Deliver {
                    target: Endpoint { node: 0, event: 10 },
                    context: None
                }
            )])
            .unwrap_err(),
        scoring::Error::Bound
    );
    assert!(runtime.instance(2).is_some());
    runtime
        .execute(vec![Step::Graph {
            id: 2,
            action: Action::Tick(0.1),
        }])
        .unwrap();
}
