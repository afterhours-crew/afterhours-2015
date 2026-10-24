// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    Host, Observer, SceneInputs,
    graphs::{Driver, Instance, Runtime},
    tests::{identity, ranked},
};
use super::*;
use nfs_content::{
    events::lifecycle::channels::Definition as Channel,
    scoring::events::{Composition, DataConnection, Node as ContentNode},
};
use nfs_world::scoring::{
    interpreter::{Inputs, Session},
    nodes,
    program::{Behavior, Identity, Node, Program, Step as Startup},
    properties::{Definition, Port, Registration},
};
use std::sync::Arc;

fn property() -> Property {
    Property {
        channel: "fixture/channel".into(),
        port: 19,
    }
}
fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
fn object(instance: u64, node: u16) -> Object {
    Object { instance, node }
}
fn input() -> Inputs {
    Inputs {
        participant: 7,
        vehicle: None,
        others: vec![],
        road: None,
        stats: None,
        now: 0.,
        time_scale: 1.,
    }
}
fn fixture(kind: Kind) -> (Instance, Registry) {
    let (accessor, store, identifier, default) = match kind {
        Kind::Boolean => (
            "BoolParticipantPropertyAccessorEntityData",
            "BoolParticipantPropertyEntityData",
            0x969a_1637,
            Value::Boolean(false),
        ),
        Kind::Float => (
            "FloatParticipantPropertyAccessorEntityData",
            "FloatParticipantPropertyEntityData",
            0xf279_5629,
            Value::Float(0.),
        ),
        _ => (
            "IntParticipantPropertyAccessorEntityData",
            "IntParticipantPropertyEntityData",
            nfs_content::scoring::participant_properties::IDENTIFIER,
            Value::Integer(5),
        ),
    };
    let classes = [
        accessor,
        accessor,
        "LinkedSchematicChannelReferenceEntityData",
        "FixtureObserver",
        store,
        "LinkedSchematicChannelEntityData",
    ];
    let nodes: Vec<_> = classes
        .iter()
        .enumerate()
        .map(|(i, class)| Node {
            identity: Identity {
                asset: "fixture".into(),
                index: i as u32,
                instances: vec![],
            },
            class: (*class).into(),
            created: matches!(i, 0 | 1 | 3),
            events: true,
            properties: true,
            forward: None,
            defaults: if i == 0 {
                vec![(nodes::FLOAT_VALUE, default.clone())]
            } else {
                vec![]
            },
            behavior: match i {
                0 | 1 => Behavior::ParticipantProperty {
                    property: property(),
                    kind,
                    write: i == 0,
                },
                3 => Behavior::Native,
                _ => Behavior::Inert,
            },
        })
        .collect();
    let edge = |a, p, b, q| DataConnection {
        source: port(a, p),
        target: port(b, q),
        bus: 0,
        index: 0,
        flags: None,
    };
    let composition = Arc::new(Composition {
        nodes: nodes
            .iter()
            .enumerate()
            .map(|(i, n)| ContentNode {
                identity: identity(i),
                class: n.class.clone(),
                listeners: None,
                forward: None,
            })
            .collect(),
        buses: vec![],
        bus_entities: vec![],
        connections: vec![],
        properties: vec![],
        links: vec![
            edge(0, identifier, 2, 19),
            edge(1, identifier, 2, 19),
            edge(5, 19, 4, 0),
        ],
    });
    let channel = |reference| Channel {
        asset: property().channel,
        reference,
        relay: false,
        links: vec![19],
        properties: vec![],
        reference_properties: vec![],
    };
    let mut links = Registry::default();
    links
        .install(
            11,
            composition,
            &BTreeMap::from([(identity(2), channel(true)), (identity(5), channel(false))]),
        )
        .unwrap();
    links.bind(object(11, 2), object(11, 5)).unwrap();
    let cell = |ports: Vec<Port>| Definition {
        kind,
        initial: if ports == [port(0, nodes::FLOAT_VALUE)] {
            Some(default.clone())
        } else {
            None
        },
        ports,
        listeners: vec![],
    };
    let program = Program {
        nodes,
        connections: vec![],
        unmodeled: vec![],
        cells: vec![
            cell(vec![port(0, nodes::FLOAT_VALUE)]),
            cell(vec![port(0, nodes::OUTPUT_VALUE), port(3, 7)]),
            cell(vec![port(1, nodes::OUTPUT_VALUE)]),
        ],
        startup: vec![
            Startup::Register(Registration::Notify(port(0, nodes::FLOAT_VALUE))),
            Startup::Register(Registration::Notify(port(3, 7))),
            Startup::Bind(0),
            Startup::Bind(1),
            Startup::Bind(3),
            Startup::Activate(0),
            Startup::Activate(3),
            Startup::Activate(1),
        ],
    };
    (
        Instance {
            session: Session::new(Arc::new(program)).unwrap(),
            inputs: input(),
        },
        links,
    )
}
#[derive(Clone, Debug)]
struct Owner {
    accessors: ParticipantAccessors,
    links: Registry,
    seen: Vec<(i32, Option<i32>)>,
    fail: bool,
}
impl Driver for Owner {
    fn invoke(&mut self, request: Request, reader: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.accessors.invoke(&request, &self.links, reader)? {
            return Ok(steps);
        }
        Ok(match request.call {
            Call::Activate => vec![Step::Lifecycle(vec![Command::Activate {
                observer: Observer::Sorter(1),
                binding: false,
            }])],
            Call::Property { .. } => {
                let value = reader
                    .integer(&IntegerSource::Local(identity(80)), 7)?
                    .ok_or(Error::MissingInput)?;
                self.seen
                    .push((value, reader.ranking(1)?.first().map(|row| row.value)));
                if self.fail && value == 9 {
                    return Err(Error::UnsupportedEvent);
                }
                vec![]
            }
            Call::Bind | Call::Retire => vec![],
            _ => return Err(Error::UnsupportedEvent),
        })
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        Ok(self.accessors.effect(effect)?.unwrap_or_default())
    }
}
fn runtime(fail: bool) -> Runtime<Owner> {
    let (instance, links) = fixture(Kind::Integer);
    let mut runtime = Runtime::new(
        Host::prepare(ranked(), SceneInputs::default()).unwrap(),
        Owner {
            accessors: ParticipantAccessors::new(
                3,
                BTreeMap::from([(
                    object(11, 4),
                    Source::Integer(IntegerSource::Local(identity(80))),
                )]),
            )
            .unwrap(),
            links,
            seen: vec![],
            fail,
        },
    );
    runtime
        .execute(vec![
            Step::Lifecycle(vec![Command::Activate {
                observer: Observer::Sorter(0),
                binding: false,
            }]),
            Step::Install {
                id: 11,
                instance: Box::new(instance),
            },
            Step::Graph {
                id: 11,
                action: Action::Start,
            },
        ])
        .unwrap();
    runtime
}
fn change(value: i32) -> Step {
    Step::Graph {
        id: 11,
        action: Action::Graph(Operation::Publish {
            port: port(0, nodes::FLOAT_VALUE),
            value: Value::Integer(value),
        }),
    }
}

#[test]
fn accessor_callbacks_interleave_with_native_sorters_and_late_reader_seeds_only_itself() {
    let mut r = runtime(false);
    assert_eq!(r.driver().seen, [(5, None)]);
    assert_eq!(
        r.instance(11)
            .unwrap()
            .session
            .read(port(1, nodes::OUTPUT_VALUE))
            .unwrap(),
        Some(Value::Integer(5))
    );
    r.execute(vec![change(9)]).unwrap();
    assert_eq!(r.driver().seen, [(5, None), (9, None)]);
    assert_eq!(r.lifecycle().ranking(1).unwrap()[0].value, 9);
    assert_eq!(
        r.instance(11)
            .unwrap()
            .session
            .read(port(1, nodes::OUTPUT_VALUE))
            .unwrap(),
        Some(Value::Integer(9))
    );
    r.execute(vec![change(9)]).unwrap();
    assert_eq!(r.driver().seen.len(), 2);
    r.execute(vec![Step::Remove { id: 11 }]).unwrap();
    assert!(r.driver().accessors.bindings.is_empty());
    // No callback can target the retired graph after its subscriptions detach.
    r.execute(vec![Step::Lifecycle(vec![Command::Integer {
        source: IntegerSource::Local(identity(80)),
        participant: 7,
        value: 12,
    }])])
    .unwrap();
    assert_eq!(
        r.lifecycle()
            .integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(12)
    );
}

#[test]
fn callback_failure_restores_graphs_store_rankings_and_subscription_owner() {
    let mut r = runtime(true);
    assert_eq!(r.execute(vec![change(9)]), Err(Error::UnsupportedEvent));
    assert_eq!(r.driver().seen, [(5, None)]);
    assert_eq!(
        r.lifecycle()
            .integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(5)
    );
    assert!(r.lifecycle().ranking(1).unwrap().is_empty());
    assert_eq!(
        r.instance(11)
            .unwrap()
            .session
            .read(port(0, nodes::OUTPUT_VALUE))
            .unwrap(),
        Some(Value::Integer(5))
    );
}

#[derive(Clone, Debug)]
struct ScalarOwner {
    accessors: ParticipantAccessors,
    links: Registry,
    source: Source,
    observed: Vec<Value>,
}
impl Driver for ScalarOwner {
    fn invoke(&mut self, request: Request, reader: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.accessors.invoke(&request, &self.links, reader)? {
            return Ok(steps);
        }
        match request.call {
            Call::Property { .. } => {
                self.observed
                    .push(self.source.read(reader, 7)?.ok_or(Error::MissingInput)?);
            }
            Call::Bind | Call::Activate | Call::Retire => {}
            _ => return Err(Error::UnsupportedEvent),
        }
        Ok(vec![])
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        Ok(self.accessors.effect(effect)?.unwrap_or_default())
    }
}
#[test]
fn boolean_accessors_seed_only_present_values_and_retire_exact_subscriptions() {
    let (instance, links) = fixture(Kind::Boolean);
    let mut runtime = Runtime::new(
        Host::prepare(
            super::super::tests::plan(super::super::tests::definition()),
            SceneInputs::default(),
        )
        .unwrap(),
        ScalarOwner {
            accessors: ParticipantAccessors::new(
                4,
                BTreeMap::from([(
                    object(11, 4),
                    Source::Boolean(BooleanSource::Channel(property())),
                )]),
            )
            .unwrap(),
            links,
            source: Source::Boolean(BooleanSource::Channel(property())),
            observed: vec![],
        },
    );
    runtime
        .execute(vec![
            Step::Install {
                id: 11,
                instance: Box::new(instance),
            },
            Step::Graph {
                id: 11,
                action: Action::Start,
            },
        ])
        .unwrap();
    assert_eq!(runtime.driver().observed, [Value::Boolean(false)]);
    assert_eq!(
        runtime
            .instance(11)
            .unwrap()
            .session
            .read(port(1, nodes::OUTPUT_VALUE))
            .unwrap(),
        Some(Value::Boolean(false))
    );
    runtime
        .execute(vec![Step::Graph {
            id: 11,
            action: Action::Graph(Operation::Publish {
                port: port(0, nodes::FLOAT_VALUE),
                value: Value::Boolean(true),
            }),
        }])
        .unwrap();
    assert_eq!(
        runtime.driver().observed,
        [Value::Boolean(false), Value::Boolean(true)]
    );
    assert_eq!(
        runtime
            .instance(11)
            .unwrap()
            .session
            .read(port(1, nodes::OUTPUT_VALUE))
            .unwrap(),
        Some(Value::Boolean(true))
    );
    runtime
        .execute(vec![Step::Lifecycle(vec![Command::Boolean {
            key: property(),
            participant: 9,
            value: false,
        }])])
        .unwrap();
    assert_eq!(
        runtime.driver().observed,
        [Value::Boolean(false), Value::Boolean(true)]
    );
    runtime.execute(vec![Step::Remove { id: 11 }]).unwrap();
    assert!(runtime.driver().accessors.bindings.is_empty());
    runtime
        .execute(vec![Step::Lifecycle(vec![Command::Boolean {
            key: property(),
            participant: 7,
            value: false,
        }])])
        .unwrap();
    assert_eq!(
        runtime.driver().observed,
        [Value::Boolean(false), Value::Boolean(true)]
    );
}

#[test]
fn float_accessors_share_one_source_and_reject_invalid_transactions() {
    let (instance, links) = fixture(Kind::Float);
    let source = Source::Float(property());
    let mut runtime = Runtime::new(
        Host::prepare(
            super::super::tests::plan(super::super::tests::definition()),
            SceneInputs::default(),
        )
        .unwrap(),
        ScalarOwner {
            accessors: ParticipantAccessors::new(
                5,
                BTreeMap::from([(object(11, 4), source.clone())]),
            )
            .unwrap(),
            links,
            source,
            observed: vec![],
        },
    );
    runtime
        .execute(vec![
            Step::Install {
                id: 11,
                instance: Box::new(instance),
            },
            Step::Graph {
                id: 11,
                action: Action::Start,
            },
        ])
        .unwrap();
    assert_eq!(runtime.driver().observed, [Value::Float(0.)]);
    let change = |value| Step::Graph {
        id: 11,
        action: Action::Graph(Operation::Publish {
            port: port(0, nodes::FLOAT_VALUE),
            value: Value::Float(value),
        }),
    };
    runtime.execute(vec![change(12.5)]).unwrap();
    assert_eq!(
        runtime.driver().observed,
        [Value::Float(0.), Value::Float(12.5)]
    );
    assert_eq!(
        runtime
            .instance(11)
            .unwrap()
            .session
            .read(port(1, nodes::OUTPUT_VALUE))
            .unwrap(),
        Some(Value::Float(12.5))
    );
    assert!(
        runtime
            .execute(vec![change(20.), change(f32::NAN)])
            .is_err()
    );
    assert_eq!(runtime.lifecycle().float_value(&property(), 7), Some(12.5));
    assert_eq!(
        runtime.driver().observed,
        [Value::Float(0.), Value::Float(12.5)]
    );
    runtime.execute(vec![Step::Remove { id: 11 }]).unwrap();
    assert!(runtime.driver().accessors.bindings.is_empty());
    runtime
        .execute(vec![Step::Lifecycle(vec![Command::Float {
            source: property(),
            participant: 7,
            value: 14.,
        }])])
        .unwrap();
    assert_eq!(
        runtime.driver().observed,
        [Value::Float(0.), Value::Float(12.5)]
    );
}
