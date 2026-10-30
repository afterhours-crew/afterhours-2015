// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_logic::lifecycle::{SceneInputs, tests::ranked};
use nfs_world::scoring::{
    events::{
        Connection, Delivery, Endpoint,
        native::{Call, Operation},
    },
    program::{Behavior, Identity, Node, Program, Step as Startup},
    properties::{Definition, Kind, Registration},
};
use std::sync::Arc;

fn port(node: u16) -> Port {
    Port { node, field: 7 }
}
fn graph(id: u64, operation: Operation) -> Step {
    Step::Graph {
        id,
        action: Action::Graph(operation),
    }
}
fn deliver(id: u64, event: u32) -> Step {
    graph(
        id,
        Operation::Deliver {
            target: Endpoint { node: 0, event },
            context: None,
        },
    )
}
fn fixture() -> Instance {
    let nodes = (0..3)
        .map(|i| Node {
            identity: Identity {
                asset: "fixture".into(),
                index: i,
                instances: vec![],
            },
            class: "FixtureNative".into(),
            created: true,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior: Behavior::Native,
        })
        .collect();
    let mut startup: Vec<_> = (0..3).map(Startup::Bind).collect();
    startup.extend([0, 2].map(|i| Startup::Register(Registration::Notify(port(i)))));
    startup.extend((0..3).map(Startup::Activate));
    Instance {
        session: Session::new(Arc::new(Program {
            nodes,
            cells: vec![Definition {
                kind: Kind::Integer,
                initial: Some(Value::Integer(5)),
                ports: vec![port(0), port(2)],
                listeners: vec![],
            }],
            startup,
            connections: [0, 2]
                .map(|target| Connection {
                    source: Endpoint { node: 0, event: 1 },
                    target: Endpoint {
                        node: target,
                        event: 2,
                    },
                    delivery: Delivery::Local,
                })
                .to_vec(),
            unmodeled: vec![],
        }))
        .unwrap(),
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
#[derive(Clone, Debug, PartialEq)]
enum Trace {
    Event(u64, u16, u32),
    Property(u64, u16),
    Retire(u64, u16),
    Read(Option<Value>),
}
#[derive(Clone, Debug, Default)]
struct Owner {
    trace: Vec<Trace>,
    fail: bool,
    remove_on_start: Option<Step>,
}
impl Owner {
    fn remove(&self, id: u64) -> Vec<Step> {
        vec![
            Step::Remove { id },
            Step::Continue {
                owner: 77,
                token: id,
            },
        ]
    }
}
impl Driver for Owner {
    fn resume(
        &mut self,
        owner: u64,
        id: u64,
        reader: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        assert_eq!(owner, 77);
        self.trace.push(Trace::Read(reader.read(id, port(0))?));
        // Bindings are gone but the value storage still exists through unwind.
        Ok(vec![
            graph(
                id,
                Operation::Publish {
                    port: port(0),
                    value: Value::Integer(99),
                },
            ),
            graph(
                id,
                Operation::Emit {
                    source: Endpoint { node: 0, event: 1 },
                    context: None,
                },
            ),
            graph(
                id,
                Operation::Deliver {
                    target: Endpoint { node: 2, event: 99 },
                    context: None,
                },
            ),
            Step::Remove { id },
        ])
    }
    fn invoke(
        &mut self,
        request: Request,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        let Request {
            instance,
            node,
            call,
        } = request;
        Ok(match call {
            Call::Retire => {
                self.trace.push(Trace::Retire(instance, node));
                if self.fail && node == 1 {
                    return Err(scoring::Error::UnsupportedEvent);
                }
                vec![]
            }
            Call::Event { event, .. } => {
                self.trace.push(Trace::Event(instance, node, event));
                match event {
                    2 => self.remove(instance),
                    4 => vec![deliver(2, 5)],
                    5 => self.remove(1),
                    _ => vec![],
                }
            }
            Call::Property { .. } => {
                self.trace.push(Trace::Property(instance, node));
                self.remove(instance)
            }
            Call::Activate if self.remove_on_start.is_some() => {
                vec![self.remove_on_start.clone().unwrap()]
            }
            Call::Bind | Call::Activate => vec![],
            _ => return Err(scoring::Error::UnsupportedEvent),
        })
    }
    fn effect(
        &mut self,
        _: &super::super::Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        Ok(vec![])
    }
}
fn runtime() -> Runtime<Owner> {
    let mut r = Runtime::new(
        Host::prepare(ranked(), SceneInputs::default()).unwrap(),
        Owner::default(),
    );
    r.execute(vec![
        Step::Install {
            id: 1,
            instance: Box::new(fixture()),
        },
        Step::Graph {
            id: 1,
            action: Action::Start,
        },
        Step::Install {
            id: 2,
            instance: Box::new(fixture()),
        },
        Step::Graph {
            id: 2,
            action: Action::Start,
        },
    ])
    .unwrap();
    r
}

#[test]
fn active_graph_deactivates_in_reverse_order_before_current_event_returns() {
    let mut r = runtime();
    r.execute(vec![graph(
        1,
        Operation::Emit {
            source: Endpoint { node: 0, event: 1 },
            context: None,
        },
    )])
    .unwrap();
    assert_eq!(
        r.driver().trace,
        [
            Trace::Event(1, 0, 2),
            Trace::Retire(1, 2),
            Trace::Retire(1, 1),
            Trace::Retire(1, 0),
            Trace::Read(Some(Value::Integer(5)))
        ]
    );
    assert!(r.instance(1).is_none());
    assert!(r.instance(2).is_some());
}

#[test]
fn retirement_during_property_notification_skips_later_detached_listeners() {
    let mut r = runtime();
    r.execute(vec![graph(
        1,
        Operation::Publish {
            port: port(0),
            value: Value::Integer(9),
        },
    )])
    .unwrap();
    assert_eq!(
        r.driver().trace,
        [
            Trace::Property(1, 0),
            Trace::Retire(1, 2),
            Trace::Retire(1, 1),
            Trace::Retire(1, 0),
            Trace::Read(Some(Value::Integer(9)))
        ]
    );
    assert!(r.instance(1).is_none());
}

#[test]
fn descendant_can_retire_suspended_ancestor_and_read_it_until_unwind() {
    let mut r = runtime();
    r.execute(vec![deliver(1, 4)]).unwrap();
    assert_eq!(
        r.driver().trace,
        [
            Trace::Event(1, 0, 4),
            Trace::Event(2, 0, 5),
            Trace::Retire(1, 2),
            Trace::Retire(1, 1),
            Trace::Retire(1, 0),
            Trace::Read(Some(Value::Integer(5)))
        ]
    );
    assert!(r.instance(1).is_none());
    assert!(r.instance(2).is_some());
}

#[test]
fn failed_retirement_restores_graph_bindings_driver_and_values_then_can_retry() {
    let mut r = runtime();
    r.driver.fail = true;
    assert_eq!(
        r.execute(vec![graph(
            1,
            Operation::Publish {
                port: port(0),
                value: Value::Integer(9)
            }
        )])
        .unwrap_err(),
        scoring::Error::UnsupportedEvent
    );
    assert!(r.driver().trace.is_empty());
    assert_eq!(
        r.instance(1).unwrap().session.read(port(0)).unwrap(),
        Some(Value::Integer(5))
    );
    r.driver.fail = false;
    r.execute(vec![deliver(1, 2)]).unwrap();
    assert!(r.instance(1).is_none());
}

#[test]
fn removal_during_startup_rejects_without_partial_installation() {
    for removal in [
        Step::Remove { id: 1 },
        Step::Graph {
            id: 1,
            action: Action::Retire,
        },
        graph(1, Operation::Retire),
    ] {
        let mut r = Runtime::new(
            Host::prepare(ranked(), SceneInputs::default()).unwrap(),
            Owner {
                remove_on_start: Some(removal),
                ..Default::default()
            },
        );
        assert_eq!(
            r.execute(vec![
                Step::Install {
                    id: 1,
                    instance: Box::new(fixture())
                },
                Step::Graph {
                    id: 1,
                    action: Action::Start
                }
            ])
            .unwrap_err(),
            scoring::Error::InvalidGraph
        );
        assert!(r.instance(1).is_none());
        assert!(r.driver().trace.is_empty());
    }
}
