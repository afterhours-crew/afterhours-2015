// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_logic::lifecycle::{
    Host, SceneInputs,
    graphs::{Driver, Instance, Runtime},
    tests::{identity, ranked},
};
use nfs_content::scoring::events::{Composition, DataConnection, Node as ContentNode};
use nfs_world::scoring::{
    events::Endpoint as EventEndpoint,
    interpreter::{Inputs, Session},
    participant_properties::Property,
    program::{Behavior, Identity as ProgramIdentity, Node, Step as Startup},
    properties::{Definition, Kind, Registration},
};
use std::sync::Arc;

fn object(node: u16) -> Object {
    Object { instance: 11, node }
}
fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
fn key() -> Property {
    Property {
        channel: "fixture/shared".into(),
        port: 19,
    }
}
fn graph(operation: Operation) -> Step {
    Step::Graph {
        id: 11,
        action: Action::Graph(operation),
    }
}
fn select(participant: u64) -> Step {
    graph(Operation::Deliver {
        target: EventEndpoint { node: 0, event: IN },
        context: Some(Context::Participant(participant)),
    })
}
fn input(value: bool) -> Step {
    graph(Operation::Publish {
        port: port(2, FLOAT_VALUE),
        value: Value::Boolean(value),
    })
}
fn write(participant: u64, value: bool) -> Step {
    Step::Lifecycle(vec![Command::Boolean {
        key: key(),
        participant,
        value,
    }])
}
fn fixture(ready: Option<bool>) -> (Instance, Registry, BTreeMap<Identity, Service>) {
    let classes = [
        "ParticipantEventToParticipantProviderEntityData",
        "BoolParticipantPropertyAccessorEntityData",
        "BoolParticipantPropertyAccessorEntityData",
        "BoolParticipantPropertyAccessorEntityData",
        "FixtureObserver",
        "BoolParticipantPropertyEntityData",
    ];
    let nodes: Vec<_> = classes
        .iter()
        .enumerate()
        .map(|(i, class)| Node {
            identity: ProgramIdentity {
                asset: "fixture".into(),
                index: i as u32,
                instances: vec![],
            },
            class: (*class).into(),
            created: i != 5,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior: if i == 5 {
                Behavior::Inert
            } else {
                Behavior::Native
            },
        })
        .collect();
    let mut services = BTreeMap::from([(identity(0), Service::ParticipantEventProvider)]);
    for i in 1..=3 {
        services.insert(
            identity(i),
            Service::BooleanAccessor {
                source: key(),
                provider: identity(0),
                default: true,
            },
        );
    }
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
        links: (1..=3)
            .flat_map(|i| {
                [(0xb68b_0dbb, 0), (0x969a_1637, 5)].map(|(field, target)| DataConnection {
                    source: port(i, field),
                    target: port(target, 0),
                    bus: 0,
                    index: 0,
                    flags: None,
                })
            })
            .collect(),
    });
    let mut links = Registry::default();
    links.install(11, composition, &BTreeMap::new()).unwrap();
    let mut cells: Vec<_> = (1..=3)
        .map(|i| Definition {
            kind: Kind::Boolean,
            initial: None,
            ports: vec![port(i, OUTPUT_VALUE), port(4, u32::from(i))],
            listeners: vec![],
        })
        .collect();
    cells.push(Definition {
        kind: Kind::Boolean,
        initial: ready.map(Value::Boolean),
        ports: vec![port(2, FLOAT_VALUE)],
        listeners: vec![],
    });
    let mut startup: Vec<_> = (0..=4).map(Startup::Bind).collect();
    startup.extend((1..=3).map(|i| Startup::Register(Registration::Notify(port(4, i)))));
    startup.push(Startup::Register(Registration::Notify(port(
        2,
        FLOAT_VALUE,
    ))));
    startup.extend((0..=4).map(Startup::Activate));
    (
        Instance {
            session: Session::new(Arc::new(Program {
                nodes,
                cells,
                startup,
                connections: vec![],
                unmodeled: vec![],
            }))
            .unwrap(),
            inputs: Inputs {
                participant: 99,
                vehicle: None,
                others: vec![],
                road: None,
                stats: None,
                now: 0.,
                time_scale: 1.,
            },
        },
        links,
        services,
    )
}

#[derive(Clone, Debug)]
struct Owner {
    properties: SharedProperties,
    trace: Vec<(u32, Option<u64>, Option<Value>)>,
    nested: bool,
    fail: bool,
    retire: bool,
}
impl Driver for Owner {
    fn resume(
        &mut self,
        owner: u64,
        token: u64,
        reader: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        self.properties
            .resume(owner, token, reader)?
            .ok_or(Error::InvalidGraph)
    }
    fn invoke(&mut self, request: Request, reader: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.properties.invoke(&request, reader)? {
            return Ok(steps);
        }
        if request.node != 4 {
            return Err(Error::UnknownNode);
        }
        Ok(match request.call {
            Call::Property { field, .. } => {
                let selected = self.properties.participant(object(0))?;
                self.trace
                    .push((field, selected, reader.read(11, port(4, field))?));
                if self.fail && field == 2 {
                    return Err(Error::UnsupportedEvent);
                }
                if self.retire {
                    self.retire = false;
                    return Ok(vec![Step::Remove { id: 11 }]);
                }
                if self.nested && selected == Some(7) && field == 1 {
                    self.nested = false;
                    vec![select(8), input(false)]
                } else {
                    vec![]
                }
            }
            Call::Event {
                event: 77,
                context: Some(Context::Participant(p)),
            } => {
                self.properties.participant_destroyed(p);
                vec![]
            }
            Call::Bind | Call::Activate | Call::Retire => vec![],
            _ => return Err(Error::UnsupportedEvent),
        })
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        Ok(self.properties.effect(effect)?.unwrap_or_default())
    }
}
fn runtime(ready: Option<bool>, nested: bool, fail: bool) -> Runtime<Owner> {
    runtime_with_retirement(ready, nested, fail, false)
}
fn runtime_with_retirement(
    ready: Option<bool>,
    nested: bool,
    fail: bool,
    retire: bool,
) -> Runtime<Owner> {
    let (instance, links, services) = fixture(ready);
    let mut properties = SharedProperties::new(17).unwrap();
    properties
        .install(
            11,
            &services,
            instance.session.program(),
            &links,
            &BTreeMap::from([(object(5), BooleanSource::Channel(key()))]),
        )
        .unwrap();
    let mut r = Runtime::new(
        Host::prepare(ranked(), SceneInputs::default()).unwrap(),
        Owner {
            properties,
            trace: vec![],
            nested,
            fail,
            retire,
        },
    );
    r.execute(vec![
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
    r
}
fn output(r: &Runtime<Owner>, node: u16) -> Option<Value> {
    r.instance(11)
        .unwrap()
        .session
        .read(port(node, OUTPUT_VALUE))
        .unwrap()
}

#[test]
fn graph_can_retire_from_provider_callback_without_removing_active_listener_storage() {
    let mut r = runtime_with_retirement(None, false, false, true);
    r.execute(vec![write(7, true), select(7)]).unwrap();
    assert!(r.instance(11).is_none());
    assert!(r.driver().properties.providers.is_empty());
    assert!(r.driver().properties.accessors.is_empty());
    assert!(r.driver().properties.observers.is_empty());
    assert!(r.driver().properties.pending.is_empty());
    assert_eq!(r.driver().trace.len(), 1);
    r.execute(vec![write(7, false)]).unwrap();
    assert_eq!(r.driver().trace.len(), 1);
}

#[test]
fn unready_input_reads_selected_participant_and_ignores_authored_default() {
    let mut r = runtime(None, false, false);
    r.execute(vec![write(7, false), write(8, true)]).unwrap();
    assert_eq!(output(&r, 1), None);
    r.execute(vec![select(7)]).unwrap();
    assert_eq!(output(&r, 2), Some(Value::Boolean(false)));
    assert_eq!(
        r.lifecycle()
            .boolean_value(&BooleanSource::Channel(key()), 7)
            .unwrap(),
        Some(false)
    );
    let trace = r.driver().trace.clone();
    r.execute(vec![select(7), write(8, false)]).unwrap();
    assert_eq!(r.driver().trace, trace);
    r.execute(vec![write(8, true), select(8)]).unwrap();
    for node in 1..=3 {
        assert_eq!(output(&r, node), Some(Value::Boolean(true)));
    }
    r.execute(vec![graph(Operation::Deliver {
        target: EventEndpoint { node: 0, event: IN },
        context: Some(Context::Integer(7)),
    })])
    .unwrap();
    assert_eq!(
        r.driver().properties.participant(object(0)).unwrap(),
        Some(8)
    );
}

#[test]
fn ready_input_writes_before_later_provider_listeners_read() {
    let mut r = runtime(Some(true), false, false);
    r.execute(vec![write(7, false), select(7)]).unwrap();
    assert_eq!(
        r.driver().trace[0],
        (1, Some(7), Some(Value::Boolean(false)))
    );
    assert_eq!(
        r.driver().trace[1],
        (1, Some(7), Some(Value::Boolean(true)))
    );
    for node in 1..=3 {
        assert_eq!(output(&r, node), Some(Value::Boolean(true)));
    }
    assert_eq!(
        r.lifecycle()
            .boolean_value(&BooleanSource::Channel(key()), 7)
            .unwrap(),
        Some(true)
    );
}

#[test]
fn nested_provider_change_keeps_outer_payload_but_source_callbacks_use_current_provider() {
    let mut r = runtime(None, true, false);
    r.execute(vec![write(7, true), write(8, true), select(7)])
        .unwrap();
    assert_eq!(
        r.driver().properties.participant(object(0)).unwrap(),
        Some(8)
    );
    // The input becomes ready after the nested notification. The remaining
    // outer writer still updates participant 7, not the now-selected 8.
    for p in [7, 8] {
        assert_eq!(
            r.lifecycle()
                .boolean_value(&BooleanSource::Channel(key()), p)
                .unwrap(),
            Some(false)
        );
    }
    let trace = r.driver().trace.clone();
    r.execute(vec![write(7, true)]).unwrap();
    assert_eq!(r.driver().trace, trace);
    assert_eq!(output(&r, 1), Some(Value::Boolean(false)));
    assert!(r.driver().properties.pending.is_empty());
    assert_eq!(r.driver().properties.providers[&object(0)].busy, 0);
}

#[test]
fn participant_destruction_clears_selection_without_publishing_then_can_select_again() {
    let mut r = runtime(None, false, false);
    r.execute(vec![write(7, true), select(7)]).unwrap();
    let trace = r.driver().trace.clone();
    r.execute(vec![
        graph(Operation::Deliver {
            target: EventEndpoint { node: 4, event: 77 },
            context: Some(Context::Participant(7)),
        }),
        write(7, false),
    ])
    .unwrap();
    assert_eq!(r.driver().properties.participant(object(0)).unwrap(), None);
    assert_eq!(r.driver().trace, trace);
    assert_eq!(output(&r, 1), Some(Value::Boolean(true)));
    r.execute(vec![select(7)]).unwrap();
    assert_eq!(output(&r, 1), Some(Value::Boolean(false)));
}

#[test]
fn missing_store_value_is_unready_and_retirement_detaches_both_subscriptions() {
    let mut r = runtime(None, false, false);
    r.execute(vec![select(0)]).unwrap();
    assert_eq!(
        r.driver().properties.participant(object(0)).unwrap(),
        Some(0)
    );
    assert_eq!(output(&r, 2), None);
    r.execute(vec![Step::Remove { id: 11 }, write(0, true)])
        .unwrap();
    assert!(r.driver().properties.providers.is_empty());
    assert!(r.driver().properties.accessors.is_empty());
    assert!(r.driver().properties.observers.is_empty());
}

#[test]
fn failed_callback_rolls_back_provider_store_outputs_and_pending_notifications() {
    let mut r = runtime(Some(true), false, true);
    r.execute(vec![write(7, false)]).unwrap();
    assert!(matches!(
        r.execute(vec![select(7)]),
        Err(Error::UnsupportedEvent)
    ));
    assert_eq!(r.driver().properties.participant(object(0)).unwrap(), None);
    assert!(r.driver().properties.pending.is_empty());
    assert_eq!(r.driver().properties.providers[&object(0)].busy, 0);
    assert!(r.driver().trace.is_empty());
    assert_eq!(output(&r, 1), None);
    assert_eq!(
        r.lifecycle()
            .boolean_value(&BooleanSource::Channel(key()), 7)
            .unwrap(),
        Some(false)
    );
}

#[test]
fn duplicate_install_and_invalid_links_preserve_existing_definitions() {
    let (instance, links, services) = fixture(None);
    let mut p = SharedProperties::new(17).unwrap();
    let sources = BTreeMap::from([(object(5), BooleanSource::Channel(key()))]);
    p.install(11, &services, instance.session.program(), &links, &sources)
        .unwrap();
    assert!(
        p.install(11, &services, instance.session.program(), &links, &sources)
            .is_err()
    );
    assert_eq!(p.providers.len(), 1);
    assert_eq!(p.accessors.len(), 3);
    let mut empty = SharedProperties::new(18).unwrap();
    assert!(
        empty
            .install(
                11,
                &services,
                instance.session.program(),
                &links,
                &BTreeMap::new()
            )
            .is_err()
    );
    assert!(empty.providers.is_empty());
    assert!(empty.accessors.is_empty());
}
