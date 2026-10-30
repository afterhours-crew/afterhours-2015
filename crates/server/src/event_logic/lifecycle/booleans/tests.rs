// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    commands::Command,
    tests::{definition, identity, plan},
};
use super::*;

fn key() -> Property {
    Property {
        channel: "fixture/booleans".into(),
        port: 7,
    }
}
fn source() -> BooleanSource {
    BooleanSource::Channel(key())
}
fn observe(observer: u64, subscribe: bool) -> Command {
    Command::BooleanObserver {
        source: source(),
        observer,
        subscribe,
    }
}
fn write(value: bool) -> Command {
    Command::Boolean {
        key: key(),
        participant: 7,
        value,
    }
}
fn host() -> Host {
    let mut d = definition();
    d.transitions.clear();
    d.bindings.sets.push(data::bindings::ParticipantSet {
        identity: identity(20),
        kind: SetKind::Boolean(Link {
            identity: identity(80),
            port: 7,
            class: "fixture".into(),
            channel: Some(key()),
        }),
    });
    d.bindings.order = vec![Observer::Set(0)];
    Host::prepare(plan(d), SceneInputs::default()).unwrap()
}
#[test]
fn callbacks_follow_activation_order_and_nested_writes_keep_outer_payload() {
    let mut h = host();
    h.dispatch(
        vec![
            observe(1, true),
            Command::Activate {
                observer: Observer::Set(0),
                binding: false,
            },
            observe(2, true),
        ],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    let mut trace = vec![];
    h.dispatch(vec![write(false)], &mut |_, effect| {
        match effect {
            Effect::BooleanCallback(n) => {
                trace.push((n.observer, n.value));
                if n.observer == 1 && !n.value {
                    return Ok(vec![observe(2, false), observe(3, true), write(true)]);
                }
            }
            Effect::Set { member, .. } => trace.push((20, *member)),
            _ => {}
        }
        Ok(vec![])
    })
    .unwrap();
    assert_eq!(
        trace,
        [
            (1, false),
            (1, true),
            (20, true),
            (3, true),
            (20, false),
            (3, false)
        ]
    );
    assert_eq!(h.boolean_value(&source(), 7), Ok(Some(true)));
    assert!(h.set_members(&identity(20)).unwrap().is_empty());
    assert!(
        h.dispatch(vec![write(true)], &mut |_, _| panic!(
            "equal value notified"
        ))
        .unwrap()
        .is_empty()
    );
}
#[test]
fn absent_false_notifies_once_and_callback_failure_restores_subscription() {
    let mut h = host();
    assert_eq!(h.boolean_value(&source(), 7), Ok(None));
    h.dispatch(
        vec![observe(1, true), observe(1, true), observe(1, false)],
        &mut |_, _| Ok(vec![]),
    )
    .unwrap();
    assert_eq!(
        h.dispatch(vec![write(false)], &mut |_, _| Ok(vec![
            observe(1, false),
            observe(0, true)
        ])),
        Err(Error::Invalid)
    );
    assert_eq!(h.boolean_value(&source(), 7), Ok(None));
    let effects = h
        .dispatch(vec![write(false)], &mut |_, _| Ok(vec![]))
        .unwrap();
    assert_eq!(
        effects,
        [Effect::BooleanCallback(Notification {
            observer: 1,
            source: source(),
            participant: 7,
            value: false
        })]
    );
    assert_eq!(h.boolean_value(&source(), 7), Ok(Some(false)));
    assert!(
        h.dispatch(vec![observe(2, true)], &mut |_, _| panic!(
            "registration seeded"
        ))
        .unwrap()
        .is_empty()
    );
}
#[test]
fn dynamically_appended_observers_are_bounded_and_invalid_sources_rejected() {
    let mut h = host();
    assert_eq!(
        h.dispatch(vec![observe(1, true), write(true)], &mut |_, effect| Ok(
            if matches!(effect, Effect::BooleanCallback(_)) {
                vec![observe(1, true)]
            } else {
                vec![]
            }
        )),
        Err(Error::Bound)
    );
    assert_eq!(h.boolean_value(&source(), 7), Ok(None));
    assert!(
        h.dispatch(vec![write(false)], &mut |_, _| panic!(
            "failed subscription escaped"
        ))
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        h.boolean_value(&BooleanSource::Local(999), 7),
        Err(Error::Invalid)
    );
}
