// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    collision::{Node, Outputs},
    properties::{Definition, Kind},
};

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Command(Command, bool),
    Property(u32, Value, bool),
    Event(EntityContext, bool),
}
#[derive(Clone, Debug, PartialEq)]
struct State {
    node: Node,
    store: Store,
    log: Vec<Entry>,
    restart: bool,
    fail: bool,
    loop_control: bool,
    swallow: bool,
}
impl Handler for State {
    fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
    fn handle_context(
        &mut self,
        target: Endpoint,
        context: EntityContext,
        _: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        assert_eq!(target, Endpoint { node: 1, event: 5 });
        self.log.push(Entry::Event(context, self.node.querying()));
        if self.fail {
            Err(Error::UnsupportedEvent)
        } else {
            Ok(())
        }
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.store
    }
    fn property_changed(&mut self, port: Port, _: bool, d: &mut Dispatch<'_>) -> Result<(), Error> {
        let value = self.store.read(port)?.unwrap();
        self.log.push(Entry::Property(
            port.field,
            value.clone(),
            self.node.querying(),
        ));
        if port.field == collision::QUERYING
            && value == Value::Boolean(false)
            && std::mem::take(&mut self.restart)
        {
            d.collision(self, 0, Action::Event(collision::BEGIN))?;
        }
        Ok(())
    }
}
impl CollisionHandler for State {
    fn collision_query(&mut self, node: u16) -> Result<&mut Node, Error> {
        if node == 0 {
            Ok(&mut self.node)
        } else {
            Err(Error::UnknownNode)
        }
    }
    fn collision_control(
        &mut self,
        _: u16,
        command: Command,
        d: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.log.push(Entry::Command(command, self.node.querying()));
        if self.loop_control {
            let event = match command {
                Command::Begin => collision::TERMINATE,
                Command::Terminate => collision::BEGIN,
            };
            let result = d.collision(self, 0, Action::Event(event));
            if !self.swallow {
                result?;
            }
        }
        Ok(())
    }
}
fn state() -> State {
    let cells = [
        (collision::QUERYING, Kind::Boolean),
        (collision::VALUE, Kind::Integer),
    ]
    .map(|(field, kind)| Definition {
        kind,
        initial: None,
        ports: vec![Port { node: 0, field }],
        listeners: vec![Port { node: 0, field }],
    })
    .to_vec();
    State {
        node: Node::new(Outputs {
            querying: true,
            value: true,
            suppress_equal_querying: true,
        }),
        store: Store::new(2, cells).unwrap(),
        log: vec![],
        restart: false,
        fail: false,
        loop_control: false,
        swallow: false,
    }
}
fn graph() -> Graph {
    Graph::new(
        2,
        vec![Connection {
            source: Endpoint {
                node: 0,
                event: collision::COLLISION,
            },
            target: Endpoint { node: 1, event: 5 },
            delivery: Delivery::Local,
        }],
    )
    .unwrap()
}
fn apply(state: &State, action: Action) -> Result<State, Error> {
    graph()
        .transaction(state, |s, d| d.collision(s, 0, action))
        .map(|(s, _)| s)
}
#[test]
fn controls_precede_boolean_publication_and_repeats_do_not_send_duplicate_commands() {
    let original = state();
    let mut s = apply(&original, Action::Event(collision::TERMINATE)).unwrap();
    assert_eq!(
        s.log,
        vec![Entry::Property(
            collision::QUERYING,
            Value::Boolean(false),
            false
        )]
    );
    s = apply(&s, Action::Event(collision::BEGIN)).unwrap();
    let after_begin = s.clone();
    s = apply(&s, Action::Event(collision::BEGIN)).unwrap();
    assert_eq!(s, after_begin);
    s = apply(&s, Action::Event(collision::TERMINATE)).unwrap();
    assert_eq!(
        &s.log[1..],
        &[
            Entry::Command(Command::Begin, true),
            Entry::Property(collision::QUERYING, Value::Boolean(true), true),
            Entry::Command(Command::Terminate, false),
            Entry::Property(collision::QUERYING, Value::Boolean(false), false),
        ]
    );
    assert_eq!(
        (Command::Begin.method(), Command::Terminate.method()),
        (1, 2)
    );
    assert!(!original.node.querying());
    assert!(original.log.is_empty());
}
#[test]
fn reports_finish_nested_restart_before_value_and_context_without_sending_terminate() {
    let mut s = apply(&state(), Action::Event(collision::BEGIN)).unwrap();
    s.log.clear();
    s.restart = true;
    let context = EntityContext { object: Some(7) };
    s = apply(&s, Action::Report { value: 5, context }).unwrap();
    assert_eq!(
        s.log,
        vec![
            Entry::Property(collision::QUERYING, Value::Boolean(false), false),
            Entry::Command(Command::Begin, true),
            Entry::Property(collision::QUERYING, Value::Boolean(true), true),
            Entry::Property(collision::VALUE, Value::Integer(5), true),
            Entry::Event(context, true),
        ]
    );
    assert!(s.node.querying());
    s = apply(&s, Action::Report { value: 5, context }).unwrap();
    s.log.clear();
    s = apply(&s, Action::Report { value: 5, context }).unwrap();
    assert_eq!(
        s.log,
        vec![
            Entry::Property(collision::VALUE, Value::Integer(5), false),
            Entry::Event(context, false)
        ]
    );
}
#[test]
fn optional_outputs_equality_configuration_and_unknown_events_keep_the_native_order() {
    let mut s = state();
    s.node.outputs.suppress_equal_querying = false;
    for _ in 0..2 {
        s = apply(&s, Action::Event(123)).unwrap();
    }
    assert_eq!(s.log.len(), 2);
    s.node.outputs.querying = false;
    s.node.outputs.value = false;
    s.store = Store::new(2, vec![]).unwrap();
    s.log.clear();
    let context = EntityContext { object: None };
    s = apply(
        &s,
        Action::Report {
            value: i32::MIN,
            context,
        },
    )
    .unwrap();
    assert_eq!(s.log, vec![Entry::Event(context, false)]);
}
#[test]
fn late_failures_invalid_context_and_recursive_controls_discard_state_and_commands() {
    let mut original = state();
    original.fail = true;
    let before = original.clone();
    assert_eq!(
        apply(
            &original,
            Action::Report {
                value: 3,
                context: EntityContext { object: None }
            }
        ),
        Err(Error::UnsupportedEvent)
    );
    assert_eq!(original, before);
    original.fail = false;
    for object in [0, 8192] {
        assert_eq!(
            apply(
                &original,
                Action::Report {
                    value: 3,
                    context: EntityContext {
                        object: Some(object)
                    }
                }
            ),
            Err(Error::InvalidGraph)
        );
    }
    original.loop_control = true;
    for swallow in [false, true] {
        original.swallow = swallow;
        let before = original.clone();
        assert_eq!(
            apply(&original, Action::Event(collision::BEGIN)),
            Err(Error::Bound)
        );
        assert_eq!(original, before);
    }
}
