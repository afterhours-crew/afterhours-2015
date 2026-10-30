// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    Host, Observer, SceneInputs,
    graphs::{Driver, Instance, Runtime},
    reporters,
    tests::{definition, identity, plan},
};
use super::*;
use nfs_content::{
    events::lifecycle::bindings::{ParticipantSet, SetCounter, SetKind},
    scoring::events::{Composition, DataConnection, Node as ContentNode},
};
use nfs_world::scoring::{
    events::Endpoint as Event,
    interpreter::{Inputs, Session},
    program::{Behavior, Identity, Node, Program, Step as Startup},
    properties::{Definition, Kind, Port, Registration},
    value::Value,
};
use std::sync::Arc;

fn key() -> Key {
    Key {
        channel: "fixture/set".into(),
        port: 19,
    }
}
fn source() -> SetSource {
    SetSource::Local(identity(80))
}
fn object(node: u16) -> Object {
    Object { instance: 11, node }
}
fn port(node: u16) -> Port {
    Port {
        node,
        field: participant_sets::CONTAINS,
    }
}
fn fixture() -> (Instance, Registry) {
    let classes = [
        "ParticipantSetAccessorEntityData",
        "ParticipantSetAccessorEntityData",
        "FixtureObserver",
        "ParticipantProviderEntityData",
        "ParticipantSetEntityData",
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
            created: i != 4,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior: match i {
                0 | 1 => Behavior::ParticipantSetAccessor {
                    set: Some(key()),
                    provider: Some(3),
                },
                2 => Behavior::Native,
                3 => Behavior::ParticipantProvider,
                _ => Behavior::Inert,
            },
        })
        .collect();
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
        links: [0, 1]
            .into_iter()
            .map(|node| DataConnection {
                source: Port {
                    node,
                    field: 0x2e0d_19d8,
                },
                target: Port { node: 4, field: 0 },
                bus: 0,
                index: 0,
                flags: None,
            })
            .collect(),
    });
    let mut links = Registry::default();
    links.install(11, composition, &BTreeMap::new()).unwrap();
    let program = Program {
        nodes,
        connections: vec![],
        unmodeled: vec![],
        cells: [0, 1]
            .into_iter()
            .map(|node| Definition {
                kind: Kind::Boolean,
                initial: None,
                ports: vec![port(node)],
                listeners: vec![],
            })
            .collect(),
        startup: vec![
            Startup::Register(Registration::Notify(port(0))),
            Startup::Register(Registration::Notify(port(1))),
            Startup::Bind(3),
            Startup::Bind(0),
            Startup::Bind(1),
            Startup::Bind(2),
            Startup::Activate(0),
            Startup::Activate(2),
            Startup::Activate(1),
        ],
    };
    (
        Instance {
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
        },
        links,
    )
}
#[derive(Clone, Debug)]
struct Owner {
    accessors: SetAccessors,
    links: Registry,
    seen: Vec<(i32, Option<Value>, Option<Value>)>,
    fail: bool,
    retired: bool,
}
impl Driver for Owner {
    fn invoke(&mut self, request: Request, reader: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.accessors.invoke(&request, &self.links, reader)? {
            return Ok(steps);
        }
        match request.call {
            Call::Activate => Ok(vec![Step::Lifecycle(vec![Command::Activate {
                observer: Observer::SetCounter(0),
                binding: false,
            }])]),
            Call::Bind => Ok(vec![]),
            Call::Retire => {
                self.retired = true;
                Ok(vec![])
            }
            _ => Err(Error::UnsupportedEvent),
        }
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        reader: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.accessors.effect(effect)? {
            return Ok(steps);
        }
        if let Effect::Reporter(reporters::Effect::Count { value, .. }) = effect {
            if !self.retired {
                self.seen
                    .push((*value, reader.read(11, port(0))?, reader.read(11, port(1))?));
            }
            if self.fail && *value == 0 {
                return Err(Error::UnsupportedEvent);
            }
        }
        Ok(vec![])
    }
}
fn runtime(initial_member: bool) -> Runtime<Owner> {
    let (instance, links) = fixture();
    let mut d = definition();
    d.transitions.clear();
    d.bindings.sets = vec![ParticipantSet {
        identity: identity(80),
        kind: SetKind::Owned,
    }];
    d.bindings.set_counters = vec![SetCounter {
        identity: identity(81),
        source: Some(source()),
    }];
    d.bindings.order = vec![Observer::Set(0), Observer::SetCounter(0)];
    let mut r = Runtime::new(
        Host::prepare(plan(d), SceneInputs::default()).unwrap(),
        Owner {
            accessors: SetAccessors::new(4, BTreeMap::from([(object(4), source())])).unwrap(),
            links,
            seen: vec![],
            fail: false,
            retired: false,
        },
    );
    let mut steps = vec![];
    if initial_member {
        steps.push(Step::Lifecycle(vec![Command::Set {
            source: source(),
            participant: 7,
            member: true,
        }]));
    }
    steps.extend([
        Step::Install {
            id: 11,
            instance: Box::new(instance),
        },
        Step::Graph {
            id: 11,
            action: Action::Start,
        },
    ]);
    r.execute(steps).unwrap();
    r
}
fn event(member: bool) -> Step {
    Step::Graph {
        id: 11,
        action: Action::Graph(Operation::Deliver {
            target: Event {
                node: 0,
                event: if member {
                    participant_sets::ADD
                } else {
                    participant_sets::REMOVE
                },
            },
            context: None,
        }),
    }
}
fn output(r: &Runtime<Owner>, node: u16) -> Option<Value> {
    r.instance(11).unwrap().session.read(port(node)).unwrap()
}

#[test]
fn membership_callbacks_interleave_with_counter_and_filter_other_participants() {
    let mut r = runtime(false);
    r.execute(vec![event(true)]).unwrap();
    assert_eq!(
        r.driver().seen,
        [
            (0, Some(Value::Boolean(false)), Some(Value::Boolean(false))),
            (1, Some(Value::Boolean(true)), Some(Value::Boolean(false)))
        ]
    );
    assert_eq!(output(&r, 1), Some(Value::Boolean(true)));
    let count = r.driver().seen.len();
    r.execute(vec![event(true)]).unwrap();
    assert_eq!(r.driver().seen.len(), count);
    r.execute(vec![Step::Lifecycle(vec![Command::Set {
        source: source(),
        participant: 8,
        member: true,
    }])])
    .unwrap();
    assert_eq!(
        r.driver().seen.last(),
        Some(&(2, Some(Value::Boolean(true)), Some(Value::Boolean(true))))
    );
    r.execute(vec![event(false)]).unwrap();
    assert_eq!(
        r.driver().seen.last(),
        Some(&(1, Some(Value::Boolean(false)), Some(Value::Boolean(true))))
    );
    assert_eq!(output(&r, 1), Some(Value::Boolean(false)));
}

#[test]
fn startup_seeds_each_accessor_at_activation_and_retirement_detaches_it() {
    let mut r = runtime(true);
    assert_eq!(
        r.driver().seen,
        [(1, Some(Value::Boolean(true)), Some(Value::Boolean(false)))]
    );
    assert_eq!(output(&r, 1), Some(Value::Boolean(true)));
    r.execute(vec![Step::Remove { id: 11 }]).unwrap();
    assert!(r.driver().accessors.bindings.is_empty());
    assert!(r.driver().accessors.observers.is_empty());
    r.execute(vec![Step::Lifecycle(vec![Command::Set {
        source: source(),
        participant: 7,
        member: false,
    }])])
    .unwrap();
    assert!(r.lifecycle().source_members(&source()).unwrap().is_empty());
}

#[test]
fn a_failed_interleaved_counter_rolls_back_both_accessors_and_shared_membership() {
    let mut r = runtime(true);
    // Driver state belongs to the same transaction as its active graph.
    let mut owner = r.driver().clone();
    owner.fail = true;
    let instance = r.instance(11).unwrap().clone();
    r = Runtime::new(r.lifecycle().clone(), owner);
    r.execute(vec![Step::Install {
        id: 11,
        instance: Box::new(instance),
    }])
    .unwrap();
    assert_eq!(r.execute(vec![event(false)]), Err(Error::UnsupportedEvent));
    assert!(
        r.lifecycle()
            .source_members(&source())
            .unwrap()
            .contains(&7)
    );
    assert_eq!(output(&r, 0), Some(Value::Boolean(true)));
    assert_eq!(output(&r, 1), Some(Value::Boolean(true)));
}
