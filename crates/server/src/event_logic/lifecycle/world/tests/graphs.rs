// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_logic::lifecycle::graphs::{self, Driver, Reader, Runtime};
use nfs_world::scoring::{
    self,
    events::{
        Endpoint,
        native::{self, Call, Request},
    },
    interpreter::{Inputs, Session, hosted::Action as GraphAction},
    program::{Behavior, Node, Program, Step as Startup},
    properties::{Definition, Kind, Port, Registration},
    value::Value,
};
type Step = graphs::Step<Scoped<Command>>;
fn port() -> Port {
    Port { node: 0, field: 7 }
}
fn graph(id: u64, operation: native::Operation) -> Step {
    Step::Graph {
        id,
        action: GraphAction::Graph(operation),
    }
}
fn event(id: u64) -> Step {
    graph(
        id,
        native::Operation::Deliver {
            target: Endpoint { node: 0, event: 9 },
            context: None,
        },
    )
}
fn fixture() -> graphs::Instance {
    graphs::Instance {
        session: Session::new(Arc::new(Program {
            nodes: vec![Node {
                identity: scoring::program::Identity {
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
#[derive(Clone, Debug)]
struct Binding {
    first: Instance,
    second: Instance,
    fail: bool,
    trace: Vec<u8>,
}
impl Driver<Scoped<Command>, Publication> for Binding {
    fn invoke(
        &mut self,
        request: Request,
        values: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        match (request.instance, request.call) {
            (_, Call::Bind | Call::Activate | Call::Retire) => Ok(vec![]),
            (1, Call::Event { event: 9, .. }) => {
                self.trace.push(1);
                Ok(vec![Step::Lifecycle(vec![state(
                    self.first,
                    Action::Enter {
                        state: 4,
                        participant: 7,
                    },
                )])])
            }
            (2, Call::Event { event: 9, .. }) => {
                self.trace.push(2);
                values.select(self.first.value())?;
                assert_eq!(values.boolean(&BooleanSource::Local(0), 7)?, Some(true));
                values.select(self.second.value())?;
                assert_eq!(values.boolean(&BooleanSource::Local(0), 7)?, None);
                Ok(vec![
                    Step::Lifecycle(vec![state(
                        self.second,
                        Action::Move {
                            source: 4,
                            participant: 7,
                            target: Some(2),
                        },
                    )]),
                    graph(
                        2,
                        native::Operation::Publish {
                            port: port(),
                            value: Value::Integer(37),
                        },
                    ),
                ])
            }
            (2, Call::Property { field: 7, .. }) => {
                self.trace.push(3);
                assert_eq!(values.read(2, port())?, Some(Value::Integer(37)));
                Ok(vec![])
            }
            (1, Call::Property { field: 7, .. }) => {
                self.trace.push(4);
                assert_eq!(values.read(1, port())?, Some(Value::Integer(200)));
                values.select(self.first.value())?;
                assert_eq!(values.boolean(&BooleanSource::Local(0), 7)?, Some(false));
                values.select(self.second.value())?;
                assert_eq!(values.boolean(&BooleanSource::Local(0), 7)?, Some(false));
                if self.fail {
                    return Err(scoring::Error::Bound);
                }
                Ok(vec![])
            }
            _ => Err(scoring::Error::UnsupportedEvent),
        }
    }
    fn effect(
        &mut self,
        effect: &Publication,
        machine: &dyn Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, scoring::Error> {
        if effect.instance == self.first
            && matches!(
                effect.value,
                Effect::Boolean {
                    change: properties::Change { value: true, .. },
                    ..
                }
            )
        {
            assert!(
                machine
                    .members(4)
                    .map_err(|_| scoring::Error::InvalidGraph)?
                    .contains(&7)
            );
            return Ok(vec![event(2)]);
        }
        if effect.instance == self.second
            && matches!(
                effect.value,
                Effect::State(Notification::Entered { state: 2, .. })
            )
        {
            assert!(
                machine
                    .members(4)
                    .map_err(|_| scoring::Error::InvalidGraph)?
                    .is_empty()
            );
            return Ok(vec![graph(
                1,
                native::Operation::Publish {
                    port: port(),
                    value: Value::Integer(200),
                },
            )]);
        }
        Ok(vec![])
    }
}
fn runtime(fail: bool) -> (Runtime<Binding, WorldHost>, Instance, Instance, Instance) {
    let (world, scene, first, second) = setup();
    let mut runtime = Runtime::new(
        world,
        Binding {
            first,
            second,
            fail,
            trace: vec![],
        },
    );
    runtime
        .execute(vec![
            Step::Install {
                id: 1,
                instance: Box::new(fixture()),
            },
            Step::Install {
                id: 2,
                instance: Box::new(fixture()),
            },
            Step::Graph {
                id: 1,
                action: GraphAction::Start,
            },
            Step::Graph {
                id: 2,
                action: GraphAction::Start,
            },
            Step::Lifecycle(vec![activate(first), activate(second)]),
        ])
        .unwrap();
    (runtime, scene, first, second)
}
#[test]
fn graphs_reenter_across_two_lifecycle_instances_under_one_scene_guard() {
    let (mut runtime, scene, first, second) = runtime(false);
    let effects = runtime.execute(vec![event(1)]).unwrap();
    assert_eq!(runtime.driver().trace, [1, 2, 3, 4]);
    assert!(
        runtime
            .lifecycle()
            .view(scene)
            .unwrap()
            .members(1)
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .lifecycle()
            .view(first)
            .unwrap()
            .members(2)
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .lifecycle()
            .view(second)
            .unwrap()
            .members(2)
            .unwrap()
            .contains(&7)
    );
    assert!(effects.iter().any(|e| matches!(e, graphs::Publication::Lifecycle(Publication {
        value: Effect::State(Notification::Transitioned { .. }), transition_target: Some(Address { instance, state: 2 }), ..
    }) if *instance == second)));
    assert_eq!(
        runtime.instance(1).unwrap().session.read(port()).unwrap(),
        Some(Value::Integer(200))
    );
    assert_eq!(
        runtime.instance(2).unwrap().session.read(port()).unwrap(),
        Some(Value::Integer(37))
    );
}
#[test]
fn late_graph_failure_rolls_back_every_graph_scope_and_shared_scene() {
    let (mut runtime, scene, first, second) = runtime(true);
    assert_eq!(runtime.execute(vec![event(1)]), Err(scoring::Error::Bound));
    assert!(runtime.driver().trace.is_empty());
    assert!(
        runtime
            .lifecycle()
            .view(scene)
            .unwrap()
            .members(1)
            .unwrap()
            .is_empty()
    );
    for instance in [first, second] {
        assert!(
            runtime
                .lifecycle()
                .view(instance)
                .unwrap()
                .members(2)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            runtime
                .lifecycle()
                .boolean(instance, &BooleanSource::Local(0), 7),
            Ok(None)
        );
    }
    for graph in [1, 2] {
        assert_eq!(
            runtime
                .instance(graph)
                .unwrap()
                .session
                .read(port())
                .unwrap(),
            None
        );
    }
}
