// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    commands::Command,
    tests::{definition, plan},
};
use super::*;

fn source() -> Property {
    Property {
        channel: "fixture/float".into(),
        port: 3,
    }
}
fn write(value: f32) -> Command {
    Command::Float {
        source: source(),
        participant: 7,
        value,
    }
}
fn observe(observer: u64, subscribe: bool) -> Command {
    Command::FloatObserver {
        source: source(),
        observer,
        subscribe,
    }
}
fn host() -> Host {
    Host::prepare(plan(definition()), SceneInputs::default()).unwrap()
}
#[test]
fn float_callbacks_keep_outer_values_and_see_live_observer_changes() {
    let mut h = host();
    h.dispatch(vec![observe(1, true), observe(2, true)], &mut |_, _| {
        Ok(vec![])
    })
    .unwrap();
    let mut calls = vec![];
    h.dispatch(vec![write(12.5)], &mut |_, e| {
        let Effect::Float(n) = e else {
            return Ok(vec![]);
        };
        calls.push((n.observer, n.value));
        Ok(if n.observer == 1 && n.value == 12.5 {
            vec![observe(2, false), observe(3, true), write(20.25)]
        } else {
            vec![]
        })
    })
    .unwrap();
    assert_eq!(calls, [(1, 12.5), (1, 20.25), (3, 20.25), (3, 12.5)]);
    assert_eq!(h.float_value(&source(), 7), Some(20.25));
}
#[test]
fn float_zero_is_ready_equal_signed_zero_is_silent_and_nonfinite_rolls_back() {
    let mut h = host();
    let first = h
        .dispatch(vec![observe(1, true), write(-0.)], &mut |_, _| Ok(vec![]))
        .unwrap();
    assert!(
        matches!(first.as_slice(), [Effect::Float(n)] if n.value.to_bits() == (-0f32).to_bits())
    );
    assert!(
        h.dispatch(vec![write(0.)], &mut |_, _| panic!(
            "equal signed zero notified"
        ))
        .unwrap()
        .is_empty()
    );
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            h.dispatch(vec![write(2.), write(value)], &mut |_, _| Ok(vec![])),
            Err(Error::Invalid)
        );
        assert_eq!(
            h.float_value(&source(), 7).unwrap().to_bits(),
            (-0f32).to_bits()
        );
    }
    assert_eq!(
        h.dispatch(vec![write(3.)], &mut |_, _| Ok(vec![
            observe(1, false),
            observe(0, true)
        ])),
        Err(Error::Invalid)
    );
    assert!(
        matches!(h.dispatch(vec![write(4.)], &mut |_, _| Ok(vec![])).unwrap().as_slice(), [Effect::Float(n)] if n.observer == 1)
    );
}
