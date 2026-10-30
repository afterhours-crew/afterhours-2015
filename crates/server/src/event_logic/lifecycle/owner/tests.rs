// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    Host, SceneInputs,
    commands::Command,
    graphs::{Instance, Runtime},
    tests::{definition, identity},
};
use super::*;
use nfs_content::{
    events::lifecycle::{
        bindings::{Observer, StateReporter},
        channels::Definition,
    },
    scoring::events::{Composition, Node},
};
use nfs_world::{
    participant_states::Action as StateAction,
    scoring::{
        events::{Connection, Context, Delivery, Endpoint, native::Operation},
        interpreter::{Inputs, Session, hosted::Action},
        program::{Behavior, Identity as ProgramIdentity, Node as ProgramNode, Step as Startup},
        properties::{Definition as Cell, Kind, Port, Registration},
        value::Value,
    },
};
use std::sync::Arc;

#[derive(Clone, Debug)]
struct Edge {
    fail: bool,
    values: Vec<i32>,
    events: Vec<Option<Context>>,
}
impl Driver for Edge {
    fn invoke(&mut self, request: Request, values: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if request.instance != 2 || request.node != 1 {
            return Err(Error::UnsupportedEvent);
        }
        Ok(match request.call {
            Call::Bind | Call::Activate | Call::Retire => vec![],
            Call::Property { field: 99, .. } => {
                let Some(Value::Integer(value)) = values.read(2, Port { node: 1, field: 99 })?
                else {
                    return Err(Error::MissingInput);
                };
                self.values.push(value);
                if self.fail && value == 1 {
                    return Err(Error::UnsupportedEvent);
                }
                if value == 1 {
                    vec![Step::Lifecycle(vec![Command::State(StateAction::Enter {
                        state: 3,
                        participant: 7,
                    })])]
                } else {
                    vec![]
                }
            }
            Call::Event { context, .. } => {
                self.events.push(context);
                vec![]
            }
            _ => return Err(Error::UnsupportedEvent),
        })
    }
    fn effect(
        &mut self,
        _: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        Ok(vec![])
    }
}
fn namespaces() -> Namespaces {
    Namespaces {
        channels: 1,
        updates: 2,
        shared: 3,
        participants: 4,
        sets: 5,
        root: 6,
    }
}
fn composition(reference: bool) -> Composition {
    Composition {
        nodes: (0..if reference { 2 } else { 6 })
            .map(|i| Node {
                identity: identity(i),
                class: match (reference, i) {
                    (true, 0) => "LinkedSchematicChannelReferenceEntityData",
                    (false, 0) => "LinkedSchematicChannelEntityData",
                    (false, 5) => "ParticipantStateReporterEntityData",
                    _ => "FixtureNative",
                }
                .into(),
                listeners: None,
                forward: None,
            })
            .collect(),
        buses: vec![],
        bus_entities: vec![],
        properties: vec![],
        links: vec![],
        connections: vec![Connection {
            source: Endpoint {
                node: if reference { 0 } else { 5 },
                event: if reference {
                    77
                } else {
                    super::super::reporters::ENTER
                },
            },
            target: Endpoint {
                node: if reference { 1 } else { 0 },
                event: 77,
            },
            delivery: Delivery::Local,
        }],
    }
}
fn channels(reference: bool) -> BTreeMap<nfs_content::scoring::events::Identity, Definition> {
    BTreeMap::from([(
        identity(0),
        Definition {
            asset: "shared-schema".into(),
            reference,
            relay: false,
            links: vec![],
            properties: vec![11],
            reference_properties: vec![],
        },
    )])
}
fn program(c: &Composition, reference: bool) -> Program {
    let port = Port { node: 0, field: 11 };
    let input = if reference {
        Port { node: 1, field: 99 }
    } else {
        Port {
            node: 5,
            field: super::super::reporters::COUNT,
        }
    };
    let mut startup = vec![
        Startup::Register(Registration::Notify(port)),
        Startup::Bind(0),
        Startup::Activate(0),
    ];
    if reference {
        startup.push(Startup::Register(Registration::Notify(input)));
    }
    startup.extend([
        Startup::Bind(if reference { 1 } else { 5 }),
        Startup::Activate(if reference { 1 } else { 5 }),
    ]);
    Program {
        nodes: c
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| ProgramNode {
                identity: ProgramIdentity {
                    asset: n.identity.asset.clone(),
                    index: n.identity.index as u32,
                    instances: vec![],
                },
                class: n.class.clone(),
                created: reference || i == 0 || i == 5,
                events: true,
                properties: true,
                forward: None,
                defaults: vec![],
                behavior: if reference || i == 0 || i == 5 {
                    Behavior::Native
                } else {
                    Behavior::Inert
                },
            })
            .collect(),
        cells: vec![Cell {
            kind: Kind::Integer,
            initial: None,
            ports: vec![port, input],
            listeners: vec![],
        }],
        connections: c.connections.clone(),
        startup,
        unmodeled: vec![],
    }
}
fn fixture(fail: bool) -> Runtime<Owner<Edge>> {
    let mut data = definition();
    data.transitions.clear();
    data.registered_transitions.clear();
    data.bindings.reporters.push(StateReporter {
        identity: identity(5),
        compound: false,
        states: vec![nfs_content::events::lifecycle::Endpoint {
            serializer: 2,
            port: 0,
        }],
    });
    data.bindings.order = vec![Observer::Reporter(0)];
    let plan = Arc::new(super::super::Plan::new(&data, |_| unreachable!()).unwrap());
    let source = composition(false);
    let reference = composition(true);
    let sd = channels(false);
    let rd = channels(true);
    let sp = program(&source, false);
    let rp = program(&reference, true);
    let mut links = Registry::default();
    links.install(1, Arc::new(source.clone()), &sd).unwrap();
    links.install(2, Arc::new(reference), &rd).unwrap();
    links
        .bind(
            Object {
                instance: 2,
                node: 0,
            },
            Object {
                instance: 1,
                node: 0,
            },
        )
        .unwrap();
    let bindings = Bindings::new(
        1,
        plan.clone(),
        &source,
        &(0..6).map(identity).collect::<Vec<_>>(),
        &links,
        &BTreeMap::new(),
    )
    .unwrap();
    let mut owner = Owner::new(
        bindings,
        links,
        Stores::default(),
        namespaces(),
        Edge {
            fail,
            values: vec![],
            events: vec![],
        },
    )
    .unwrap();
    owner
        .install_channels(&[
            Graph {
                instance: 2,
                program: &rp,
                channels: &rd,
            },
            Graph {
                instance: 1,
                program: &sp,
                channels: &sd,
            },
        ])
        .unwrap();
    let mut runtime = Runtime::new(Host::prepare(plan, SceneInputs::default()).unwrap(), owner);
    let mut steps = Vec::new();
    for (id, program) in [(1, sp), (2, rp)] {
        steps.push(Step::Install {
            id,
            instance: Box::new(Instance {
                session: Session::new(Arc::new(program)).unwrap(),
                inputs: Inputs {
                    participant: 7,
                    vehicle: None,
                    others: vec![],
                    participants: vec![],
                    road: None,
                    stats: None,
                    now: 0.,
                    time_scale: 1.,
                },
            }),
        });
        steps.push(Step::Graph {
            id,
            action: Action::Start,
        });
    }
    runtime.execute(steps).unwrap();
    runtime
}

#[test]
fn channel_callback_reenters_lifecycle_with_shared_owner_and_commits_or_rolls_back() {
    for fail in [false, true] {
        let mut runtime = fixture(fail);
        let before = runtime.driver().edge().values.clone();
        let result = runtime.execute(vec![Step::Lifecycle(vec![Command::State(
            StateAction::Enter {
                state: 2,
                participant: 7,
            },
        )])]);
        if fail {
            assert_eq!(result.unwrap_err(), Error::UnsupportedEvent);
            assert_eq!(runtime.driver().edge().values, before);
            assert!(runtime.lifecycle().machine().members(2).unwrap().is_empty());
            assert!(runtime.lifecycle().machine().members(3).unwrap().is_empty());
            assert_eq!(
                runtime
                    .instance(1)
                    .unwrap()
                    .session
                    .read(Port { node: 0, field: 11 })
                    .unwrap(),
                Some(Value::Integer(0))
            );
        } else {
            result.unwrap();
            assert!(
                runtime
                    .lifecycle()
                    .machine()
                    .members(2)
                    .unwrap()
                    .contains(&7)
            );
            assert!(
                runtime
                    .lifecycle()
                    .machine()
                    .members(3)
                    .unwrap()
                    .contains(&7)
            );
            assert_eq!(runtime.driver().edge().values.last(), Some(&1));
            assert_eq!(
                runtime.driver().edge().events,
                [Some(Context::Participant(7))]
            );
        }
    }
}

#[test]
fn native_calls_are_not_silently_accepted_and_namespace_collisions_are_rejected() {
    let mut runtime = fixture(false);
    let result = runtime.execute(vec![Step::Graph {
        id: 1,
        action: Action::Graph(Operation::Deliver {
            target: Endpoint { node: 0, event: 8 },
            context: None,
        }),
    }]);
    // Channel events are handled, but a callback without an installed owner is not.
    result.unwrap();
    assert_eq!(
        Namespaces {
            updates: 1,
            ..namespaces()
        }
        .validate(),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        Namespaces {
            shared: 0,
            ..namespaces()
        }
        .validate(),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        runtime
            .execute(vec![Step::Graph {
                id: 1,
                action: Action::Graph(Operation::Update { node: 5 })
            }])
            .unwrap_err(),
        Error::UnsupportedEvent
    );
}
