// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{properties::Definition, synced::Node};

const INPUT: Port = Port {
    node: 0,
    field: synced::IN,
};
const OUTPUT: Port = Port {
    node: 0,
    field: synced::OUT,
};
const OBSERVER: Port = Port { node: 1, field: 99 };

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Value(Value, bool),
    Event(u32, bool),
}
#[derive(Clone, Debug, PartialEq)]
struct State {
    properties: Store,
    node: Node,
    input_present: bool,
    feedback: Option<Value>,
    loop_feedback: bool,
    swallow: bool,
    fail_after_dirty: bool,
    log: Vec<Entry>,
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        assert_eq!(target.node, 1);
        self.log.push(Entry::Event(target.event, self.node.dirty()));
        if self.fail_after_dirty {
            Err(Error::UnsupportedEvent)
        } else {
            Ok(())
        }
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.properties
    }
    fn property_changed(
        &mut self,
        target: Port,
        _: bool,
        d: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        if target == INPUT {
            return d.synchronized(self, 0, Action::InputChanged).map(|_| ());
        }
        assert_eq!(target, OBSERVER);
        let value = self.properties.read(OUTPUT)?.ok_or(Error::MissingInput)?;
        self.log
            .push(Entry::Value(value.clone(), self.node.dirty()));
        let feedback = if self.loop_feedback {
            let Value::Boolean(value) = value else {
                return Err(Error::InvalidGraph);
            };
            Some(Value::Boolean(!value))
        } else {
            self.feedback.take()
        };
        if let Some(value) = feedback {
            let result = d.publish(self, INPUT, value);
            if !self.swallow {
                result?;
            }
        }
        Ok(())
    }
}
impl SyncedHandler for State {
    fn synchronized_node(&mut self, node: u16) -> Result<&mut Node, Error> {
        if node == 0 {
            Ok(&mut self.node)
        } else {
            Err(Error::UnknownNode)
        }
    }
    fn synchronized_input(&self, _: u16) -> Result<Option<Value>, Error> {
        if self.input_present {
            self.properties.read(INPUT)
        } else {
            Ok(None)
        }
    }
}
fn state(kind: Kind, initial: Option<Value>) -> State {
    State {
        properties: Store::new(
            2,
            vec![
                Definition {
                    kind: kind.property_kind(),
                    initial,
                    ports: vec![INPUT],
                    listeners: vec![INPUT],
                },
                Definition {
                    kind: kind.property_kind(),
                    initial: None,
                    ports: vec![OUTPUT, OBSERVER],
                    listeners: vec![OBSERVER],
                },
            ],
        )
        .unwrap(),
        node: Node::new(kind),
        input_present: true,
        feedback: None,
        loop_feedback: false,
        swallow: false,
        fail_after_dirty: false,
        log: vec![],
    }
}
fn graph() -> Graph {
    Graph::new(
        2,
        [synced::ON_FALSE, synced::ON_TRUE]
            .into_iter()
            .map(|event| Connection {
                source: Endpoint { node: 0, event },
                target: Endpoint { node: 1, event },
                delivery: Delivery::Local,
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn strings_preserve_unready_empty_and_content_equality_without_boolean_events() {
    use crate::scoring::value::Text;
    let text = |v| Value::Text(Text::new(v).unwrap());
    let fresh = state(Kind::Text, None);
    assert_eq!(apply(&fresh, Action::Bind).unwrap(), fresh);
    let (mut empty, _) = graph().publish(&fresh, INPUT, text("")).unwrap();
    assert_eq!(empty.log, [Entry::Value(text(""), false)]);
    assert_eq!(empty.properties.read(OUTPUT).unwrap(), Some(text("")));
    assert!(empty.node.take_dirty());
    empty.log.clear();
    let (equal, _) = graph().publish(&empty, INPUT, text("")).unwrap();
    assert_eq!(equal, empty);
    let (mut named, _) = graph()
        .publish(&empty, INPUT, text("In The Driver's Seat"))
        .unwrap();
    assert_eq!(
        named.log,
        [Entry::Value(text("In The Driver's Seat"), false)]
    );
    assert!(named.node.take_dirty());
    named.log.clear();
    let independently_owned = String::from("In The Driver's Seat");
    let (equal, _) = graph()
        .publish(&named, INPUT, text(&independently_owned))
        .unwrap();
    assert_eq!(equal, named);
    assert_eq!(
        apply(&named, Action::Event(synced::SET_TRUE)).unwrap(),
        named
    );
    assert!(graph().publish(&named, INPUT, Value::Integer(0)).is_err());
}
fn apply(state: &State, action: Action) -> Result<State, Error> {
    graph()
        .transaction(state, |s, d| d.synchronized(s, 0, action).map(|_| ()))
        .map(|(s, _)| s)
}

#[test]
fn fresh_false_publishes_before_dirty_and_false_event_then_equal_values_suppress() {
    let fresh = state(Kind::Boolean, Some(Value::Boolean(false)));
    let bound = apply(&fresh, Action::Bind).unwrap();
    assert_eq!(
        bound.log,
        [
            Entry::Value(Value::Boolean(false), false),
            Entry::Event(synced::ON_FALSE, true)
        ]
    );
    assert!(bound.node.dirty());
    assert_eq!(fresh.properties.read(OUTPUT), Ok(None));
    let mut clean = bound.clone();
    assert!(clean.node.take_dirty());
    assert!(!clean.node.take_dirty());
    clean.log.clear();
    for action in [
        Action::Bind,
        Action::InputChanged,
        Action::Event(synced::SET_FALSE),
    ] {
        assert_eq!(apply(&clean, action).unwrap(), clean);
    }
    let (changed, _) = graph()
        .publish(&clean, INPUT, Value::Boolean(true))
        .unwrap();
    assert_eq!(
        changed.log,
        [
            Entry::Value(Value::Boolean(true), false),
            Entry::Event(synced::ON_TRUE, true)
        ]
    );
}

#[test]
fn binding_without_ready_input_keeps_output_unready_and_activation_is_not_needed() {
    let mut fresh = state(Kind::Integer, None);
    assert_eq!(apply(&fresh, Action::Bind).unwrap(), fresh);
    assert_eq!(
        apply(&fresh, Action::InputChanged),
        Err(Error::MissingInput)
    );
    fresh.input_present = false;
    assert_eq!(apply(&fresh, Action::Bind).unwrap(), fresh);
    fresh.input_present = true;
    let (changed, _) = graph().publish(&fresh, INPUT, Value::Integer(-17)).unwrap();
    assert_eq!(changed.log, [Entry::Value(Value::Integer(-17), false)]);
    assert_eq!(
        changed.properties.read(OUTPUT),
        Ok(Some(Value::Integer(-17)))
    );
    assert!(changed.node.dirty());
}

#[test]
fn numeric_zeros_extremes_and_signed_zero_deduplication_keep_exact_output_bits() {
    for (kind, value) in [
        (Kind::Float, Value::Float(-0.0)),
        (Kind::Integer, Value::Integer(0)),
        (Kind::Integer, Value::Integer(i32::MIN)),
        (Kind::Integer, Value::Integer(i32::MAX)),
    ] {
        let mut bound = apply(&state(kind, Some(value.clone())), Action::Bind).unwrap();
        assert_eq!(bound.log, [Entry::Value(value, false)]);
        bound.node.take_dirty();
        bound.log.clear();
        let repeated = apply(&bound, Action::InputChanged).unwrap();
        assert_eq!(repeated, bound);
        assert_eq!(
            apply(&bound, Action::Event(synced::SET_TRUE)).unwrap(),
            bound
        );
    }
    let mut bound = apply(&state(Kind::Float, Some(Value::Float(-0.0))), Action::Bind).unwrap();
    bound.node.take_dirty();
    bound.log.clear();
    let (zero, _) = graph().publish(&bound, INPUT, Value::Float(0.0)).unwrap();
    assert!(zero.log.is_empty());
    assert!(!zero.node.dirty());
    let Some(Value::Float(value)) = zero.properties.read(OUTPUT).unwrap() else {
        panic!()
    };
    assert_eq!(value.to_bits(), (-0.0f32).to_bits());
    let (next, _) = graph()
        .publish(&zero, INPUT, Value::Float(f32::from_bits(1)))
        .unwrap();
    assert!(next.node.dirty());
    assert_eq!(next.log.len(), 1);
}

#[test]
fn boolean_commands_use_output_and_unready_toggle_is_true() {
    for (command, expected) in [
        (synced::SET_TRUE, true),
        (synced::SET_FALSE, false),
        (synced::TOGGLE, true),
    ] {
        let fresh = state(Kind::Boolean, None);
        let mut next = apply(&fresh, Action::Event(command)).unwrap();
        assert_eq!(
            next.properties.read(OUTPUT),
            Ok(Some(Value::Boolean(expected)))
        );
        next.log.clear();
        let toggled = apply(&next, Action::Event(synced::TOGGLE)).unwrap();
        assert_eq!(
            toggled.properties.read(OUTPUT),
            Ok(Some(Value::Boolean(!expected)))
        );
        assert_eq!(
            toggled.log.last(),
            Some(&Entry::Event(
                if expected {
                    synced::ON_FALSE
                } else {
                    synced::ON_TRUE
                },
                true
            ))
        );
        assert_eq!(apply(&next, Action::Event(0)).unwrap(), next);
    }
}

#[test]
fn nested_input_feedback_changes_outer_property_event_but_not_explicit_command_event() {
    for action in [
        Action::InputChanged,
        Action::Bind,
        Action::Event(synced::SET_TRUE),
    ] {
        let mut fresh = state(Kind::Boolean, Some(Value::Boolean(true)));
        fresh.feedback = Some(Value::Boolean(false));
        let next = apply(&fresh, action).unwrap();
        assert_eq!(
            next.properties.read(OUTPUT),
            Ok(Some(Value::Boolean(false)))
        );
        let outer_event = if matches!(action, Action::Event(_)) {
            synced::ON_TRUE
        } else {
            synced::ON_FALSE
        };
        assert_eq!(
            next.log,
            [
                Entry::Value(Value::Boolean(true), false),
                Entry::Value(Value::Boolean(false), false),
                Entry::Event(synced::ON_FALSE, true),
                Entry::Event(outer_event, true)
            ]
        );
    }
}

#[test]
fn nested_failures_rollback_values_dirty_flags_events_and_swallowed_errors() {
    let mut fresh = state(Kind::Boolean, Some(Value::Boolean(true)));
    fresh.fail_after_dirty = true;
    let before = fresh.clone();
    assert_eq!(apply(&fresh, Action::Bind), Err(Error::UnsupportedEvent));
    assert_eq!(fresh, before);
    for swallow in [false, true] {
        fresh.fail_after_dirty = false;
        fresh.loop_feedback = true;
        fresh.swallow = swallow;
        assert_eq!(apply(&fresh, Action::Bind), Err(Error::Bound));
        assert_eq!(fresh.properties.read(OUTPUT), Ok(None));
        assert!(!fresh.node.dirty());
        assert!(fresh.log.is_empty());
    }
    let mut wrong = state(Kind::Float, Some(Value::Float(1.0)));
    wrong.node = Node::new(Kind::Boolean);
    assert_eq!(apply(&wrong, Action::Bind), Err(Error::InvalidGraph));
    let finite = state(Kind::Float, None);
    for value in [f32::NAN, f32::INFINITY] {
        assert_eq!(
            graph().publish(&finite, INPUT, Value::Float(value)),
            Err(Error::NonFinite)
        );
    }
    assert_eq!(
        graph().transaction(&finite, |s, d| {
            let _ = d.synchronized(s, 2, Action::Bind);
            Ok(())
        }),
        Err(Error::UnknownNode)
    );
}
