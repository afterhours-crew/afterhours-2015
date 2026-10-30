// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    Host, SceneInputs,
    graphs::{Driver, Instance, Reader, Runtime},
    tests::{definition, identity, plan},
};
use super::*;
use nfs_content::{
    events::lifecycle::{
        bindings::{Predicate, StateReporter},
        channels::Definition,
    },
    scoring::events::{DataConnection, Node},
};
use nfs_world::scoring::{
    events::{Connection, Delivery},
    interpreter::{Inputs, Session},
    program::{
        Behavior, Identity as ProgramIdentity, Node as ProgramNode, Program, Step as Startup,
    },
    properties::{Definition as Cell, Kind},
};

fn fixture() -> (Bindings, Registry, Arc<Composition>) {
    let mut d = definition();
    d.bindings.reporters.push(StateReporter {
        identity: identity(5),
        compound: false,
        states: vec![nfs_content::events::lifecycle::Endpoint {
            serializer: 2,
            port: 0,
        }],
    });
    d.bindings.predicates.push(Predicate {
        identity: identity(6),
        kind: PredicateKind::Triggered,
    });
    d.bindings.order = vec![
        Observer::Reporter(0),
        Observer::Predicate(0),
        Observer::Transition(4),
    ];
    let p = plan(d);
    let c = Arc::new(Composition {
        nodes: (0..9)
            .map(|i| Node {
                identity: identity(i),
                class: match i {
                    7 => "LinkedSchematicChannelEntityData",
                    8 => "LinkedSchematicChannelReferenceEntityData",
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
        connections: vec![Connection {
            source: Endpoint {
                node: 5,
                event: super::super::reporters::ENTER,
            },
            target: Endpoint {
                node: 6,
                event: 0xa425_3151,
            },
            delivery: Delivery::Local,
        }],
        links: [(11, 2), (11, 2), (12, 3)]
            .into_iter()
            .enumerate()
            .map(|(index, (field, node))| DataConnection {
                source: Port { node: 7, field },
                target: Port { node, field: 0 },
                bus: 0,
                index: index as u16,
                flags: None,
            })
            .collect(),
    });
    let channels = [(7, false), (8, true)]
        .into_iter()
        .map(|(i, reference)| {
            (
                identity(i),
                Definition {
                    asset: "event-channel".into(),
                    reference,
                    relay: false,
                    links: vec![11, 12, 13],
                    properties: vec![],
                    reference_properties: vec![],
                },
            )
        })
        .collect();
    let mut links = Registry::default();
    links.install(1, c.clone(), &channels).unwrap();
    links.install(2, c.clone(), &channels).unwrap();
    links
        .bind(
            Object {
                instance: 2,
                node: 8,
            },
            Object {
                instance: 1,
                node: 7,
            },
        )
        .unwrap();
    let b = Bindings::new(
        1,
        p,
        &c,
        &(0..9).map(identity).collect::<Vec<_>>(),
        &links,
        &BTreeMap::new(),
    )
    .unwrap();
    (b, links, c)
}

#[test]
fn copied_set_events_dispatch_to_the_matching_lifecycle_owner() {
    use nfs_content::events::lifecycle::bindings::{ParticipantSet, SetKind, SetSource};
    let (_, _, original) = fixture();
    let mut d = definition();
    d.bindings.sets = vec![
        ParticipantSet {
            identity: identity(5),
            kind: SetKind::Owned,
        },
        ParticipantSet {
            identity: identity(6),
            kind: SetKind::Copy {
                source: Some(SetSource::Local(identity(5))),
            },
        },
    ];
    d.bindings.order = vec![Observer::Set(0), Observer::Set(1), Observer::Transition(4)];
    let p = plan(d);
    let mut c = (*original).clone();
    c.nodes.truncate(7);
    c.nodes[5].class = "ParticipantSetEntityData".into();
    c.nodes[6].class = "ParticipantSetCopyEntityData".into();
    c.connections.clear();
    c.links = vec![DataConnection {
        source: Port {
            node: 6,
            field: 0x2e0d_19d8,
        },
        target: Port { node: 5, field: 0 },
        bus: 0,
        index: 0,
        flags: None,
    }];
    let c = Arc::new(c);
    let mut links = Registry::default();
    links.install(1, c.clone(), &BTreeMap::new()).unwrap();
    let b = Bindings::new(
        1,
        p.clone(),
        &c,
        &(0..7).map(identity).collect::<Vec<_>>(),
        &links,
        &BTreeMap::new(),
    )
    .unwrap();
    let mut h = Host::prepare(p, SceneInputs::default()).unwrap();
    h.dispatch(
        vec![Command::Set {
            source: SetSource::Local(identity(5)),
            participant: 9,
            member: true,
        }],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    for call in [
        Call::Activate,
        Call::Event {
            event: 0x7c81_3ec0,
            context: None,
        },
    ] {
        let request = Request {
            instance: 1,
            node: 6,
            call,
        };
        assert!(
            b.invoke(
                &Request {
                    instance: 2,
                    ..request.clone()
                },
                &links
            )
            .unwrap()
            .is_none()
        );
        let mut steps = b.invoke(&request, &links).unwrap().unwrap();
        assert_eq!(steps.len(), 1);
        let Step::Lifecycle(commands) = steps.remove(0) else {
            panic!("not lifecycle")
        };
        h.dispatch(commands, &mut |_, _| Ok(vec![])).unwrap();
    }
    assert_eq!(
        h.source_members(&SetSource::Local(identity(6))).unwrap(),
        std::collections::BTreeSet::from([9])
    );
}

#[test]
fn request_to_final_excludes_final_and_preserves_registration_order() {
    let (mut bindings, _, _) = fixture();
    Arc::make_mut(&mut bindings.plan)
        .root
        .as_mut()
        .unwrap()
        .registered_states = vec![3, 2, 1, 2];
    assert_eq!(
        bindings.request_final(7).unwrap(),
        Command::State(StateAction::MoveFirstMember {
            sources: vec![2, 1, 2],
            participant: 7,
            target: Some(3),
        })
    );
}

#[test]
fn native_timing_dispatch_selects_exact_state_or_transition_and_keeps_event_context() {
    use super::super::timed;
    use nfs_content::events::lifecycle::{StateConfiguration, StateKind};
    let mut data = super::super::tests::timed_definition();
    data.states[0].kind = StateKind::Countdown;
    data.states[0].configuration = StateConfiguration::Countdown {
        sequence: ("sequence".into(), 0),
        seconds: 3.,
    };
    let plan = Arc::new(Plan::new(&data, |_| unreachable!()).unwrap());
    let (_, _, original) = fixture();
    let mut c = (*original).clone();
    c.connections.clear();
    c.links.clear();
    let mut links = Registry::default();
    links
        .install(1, Arc::new(c.clone()), &BTreeMap::new())
        .unwrap();
    let bindings = Bindings::new(
        1,
        plan,
        &c,
        &(0..9).map(identity).collect::<Vec<_>>(),
        &links,
        &BTreeMap::new(),
    )
    .unwrap();
    for (node, call, expected) in [
        (1, Call::Update, timed::Command::Countdown(1)),
        (4, Call::Update, timed::Command::Transition(4)),
        (6, Call::Update, timed::Command::Transition(6)),
        (
            5,
            Call::Event {
                event: 0x44b3_58f4,
                context: Some(Context::Participant(7)),
            },
            timed::Command::DnfEvent {
                transition: 5,
                event: 0x44b3_58f4,
                participant: Some(7),
            },
        ),
    ] {
        let steps = bindings
            .invoke(
                &Request {
                    instance: 1,
                    node,
                    call,
                },
                &links,
            )
            .unwrap()
            .unwrap();
        let [Step::Lifecycle(commands)] = steps.as_slice() else {
            panic!("timing command");
        };
        assert_eq!(commands, &[Command::Timing(expected)]);
    }
    assert!(
        bindings
            .invoke(
                &Request {
                    instance: 1,
                    node: 5,
                    call: Call::Update
                },
                &links
            )
            .unwrap()
            .is_none()
    );
    assert!(
        bindings
            .invoke(
                &Request {
                    instance: 2,
                    node: 1,
                    call: Call::Update
                },
                &links
            )
            .unwrap()
            .is_none()
    );
}
#[test]
fn transition_sources_resolve_the_current_channel_instance_preserving_repeats() {
    let (b, mut links, _) = fixture();
    let key = |port| nfs_world::scoring::participant_properties::Property {
        channel: "event-channel".into(),
        port,
    };
    let mut request = transitions::Request {
        node: 0,
        selection: transitions::Selection::FirstCompleted(7),
        sources: vec![key(11)],
        target: Some(key(12)),
    };
    assert_eq!(
        b.transition(2, &request, &links).unwrap(),
        Command::State(StateAction::MoveFirstCompleted {
            sources: vec![2, 2],
            participant: 7,
            target: Some(3)
        })
    );
    request.target = Some(key(13));
    request.selection = transitions::Selection::AllMembers;
    assert_eq!(
        b.transition(2, &request, &links).unwrap(),
        Command::State(StateAction::MoveAll {
            sources: vec![2, 2],
            target: None
        })
    );
    links
        .unbind(Object {
            instance: 2,
            node: 8,
        })
        .unwrap();
    assert_eq!(b.transition(2, &request, &links), Err(Error::MissingInput));
    links
        .bind(
            Object {
                instance: 2,
                node: 8,
            },
            Object {
                instance: 2,
                node: 7,
            },
        )
        .unwrap();
    assert_eq!(b.transition(2, &request, &links), Err(Error::InvalidGraph));
}
#[derive(Clone)]
struct Owner {
    bindings: Bindings,
    links: Registry,
    changes: Vec<(u64, bool)>,
}
impl Driver for Owner {
    fn invoke(&mut self, request: Request, _: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        self.bindings
            .invoke(&request, &self.links)?
            .ok_or(Error::UnsupportedEvent)
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        if let Effect::Boolean { change, .. } = effect {
            self.changes.push((change.participant, change.value));
        }
        Ok(self.bindings.effect(effect)?.unwrap_or_default())
    }
}
#[test]
fn lifecycle_reporter_reenters_the_real_graph_and_preserves_participant_context() {
    let (bindings, links, c) = fixture();
    let port = Port {
        node: 5,
        field: super::super::reporters::COUNT,
    };
    let program = Program {
        nodes: c
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| ProgramNode {
                identity: ProgramIdentity {
                    asset: n.identity.asset.clone(),
                    index: i as u32,
                    instances: vec![],
                },
                class: n.class.clone(),
                created: matches!(i, 4..=6),
                events: true,
                properties: true,
                forward: None,
                defaults: vec![],
                behavior: if matches!(i, 4..=6) {
                    Behavior::Native
                } else {
                    Behavior::Inert
                },
            })
            .collect(),
        cells: vec![Cell {
            kind: Kind::Integer,
            initial: None,
            ports: vec![port],
            listeners: vec![],
        }],
        connections: c.connections.clone(),
        startup: vec![
            Startup::Bind(5),
            Startup::Activate(5),
            Startup::Bind(6),
            Startup::Activate(6),
            Startup::Bind(4),
            Startup::Activate(4),
        ],
        unmodeled: vec![],
    };
    let host = Host::prepare(bindings.plan.clone(), SceneInputs::default()).unwrap();
    let mut runtime = Runtime::new(
        host,
        Owner {
            bindings,
            links,
            changes: vec![],
        },
    );
    runtime
        .execute(vec![
            Step::Install {
                id: 1,
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
            },
            Step::Graph {
                id: 1,
                action: Action::Start,
            },
        ])
        .unwrap();
    assert_eq!(
        runtime.instance(1).unwrap().session.read(port).unwrap(),
        Some(Value::Integer(0))
    );
    runtime
        .execute(vec![Step::Lifecycle(vec![Command::State(
            StateAction::Enter {
                state: 2,
                participant: 7,
            },
        )])])
        .unwrap();
    assert_eq!(runtime.driver().changes, [(7, true)]);
    assert_eq!(
        runtime.instance(1).unwrap().session.read(port).unwrap(),
        Some(Value::Integer(1))
    );
    assert!(
        runtime
            .lifecycle()
            .machine()
            .members(2)
            .unwrap()
            .contains(&7)
    );
}
