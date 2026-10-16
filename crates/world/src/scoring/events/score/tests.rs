// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    properties::{Definition, Kind},
    score::{Binding, Context},
};
const INPUT: Port = Port {
    node: 0,
    field: score::SCORE,
};
#[derive(Clone, Debug, PartialEq)]
struct State {
    store: Store,
    binding: Binding,
    context: Context,
    requests: Vec<Request>,
    mutate_input: bool,
    recursive: bool,
    swallow: bool,
    fail_keeper: Option<u64>,
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, d: &mut Dispatch<'_>) -> Result<(), Error> {
        d.score(self, target.node, target.event)
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.store
    }
    fn property_changed(&mut self, _: Port, _: bool, _: &mut Dispatch<'_>) -> Result<(), Error> {
        Ok(())
    }
}
impl ScoreHandler for State {
    fn score_binding(&self, _: u16) -> Result<Binding, Error> {
        Ok(self.binding)
    }
    fn score_input(&self, _: u16) -> Result<Option<Value>, Error> {
        self.store.read(INPUT)
    }
    fn score_context(&self, _: u16) -> Result<Context, Error> {
        Ok(self.context.clone())
    }
    fn score_request(&mut self, request: Request, d: &mut Dispatch<'_>) -> Result<(), Error> {
        if self.requests.len() >= MAX_STEPS {
            return Err(Error::Bound);
        }
        self.requests.push(request);
        if self.fail_keeper == Some(request.keeper) {
            return Err(Error::UnsupportedEvent);
        }
        if self.mutate_input {
            d.publish(self, INPUT, Value::Integer(93))?;
            self.context.participant = 999;
        }
        if self.recursive {
            let result = d.score(self, 0, score::AWARD);
            if !self.swallow {
                result?;
            }
        }
        Ok(())
    }
}
fn state(value: Option<Value>) -> State {
    State {
        store: Store::new(
            2,
            vec![Definition {
                kind: Kind::Integer,
                initial: value,
                ports: vec![INPUT],
                listeners: vec![],
            }],
        )
        .unwrap(),
        binding: Binding {
            context: 1,
            default: 23,
        },
        context: Context {
            participant: 7,
            keepers: vec![10, 20, 10],
        },
        requests: vec![],
        mutate_input: false,
        recursive: false,
        swallow: false,
        fail_keeper: None,
    }
}
fn run(s: &State, event: u32) -> Result<State, Error> {
    Graph::new(2, vec![])?
        .transaction(s, |s, d| d.score(s, 0, event))
        .map(|(s, _)| s)
}
#[test]
fn defaults_repeats_zero_and_negative_amounts_are_not_deduplicated_or_recalculated() {
    for (value, expected) in [
        (None, 23),
        (Some(Value::Integer(0)), 0),
        (Some(Value::Integer(-3)), -3),
        (Some(Value::Integer(i32::MAX)), i32::MAX),
    ] {
        let fresh = state(value);
        let next = run(&fresh, score::AWARD).unwrap();
        assert_eq!(
            next.requests,
            [10, 20, 10].map(|keeper| Request {
                keeper,
                participant: 7,
                amount: expected
            })
        );
        assert!(fresh.requests.is_empty());
        assert_eq!(run(&next, score::AWARD).unwrap().requests.len(), 6);
        assert_eq!(run(&fresh, 0).unwrap(), fresh);
    }
}
#[test]
fn amount_and_participant_are_read_once_before_synchronous_keeper_callbacks() {
    let mut fresh = state(Some(Value::Integer(12)));
    fresh.mutate_input = true;
    let next = run(&fresh, score::AWARD).unwrap();
    assert!(
        next.requests
            .iter()
            .all(|r| r.amount == 12 && r.participant == 7)
    );
    assert_eq!(next.store.read(INPUT), Ok(Some(Value::Integer(93))));
    let twice = run(&next, score::AWARD).unwrap();
    assert!(
        twice.requests[3..]
            .iter()
            .all(|r| r.amount == 93 && r.participant == 999)
    );
}
#[test]
fn empty_context_and_separate_sessions_do_not_gain_implicit_awards() {
    let mut empty = state(None);
    empty.context.keepers.clear();
    assert_eq!(run(&empty, score::AWARD).unwrap(), empty);
    let first = state(Some(Value::Integer(8)));
    let mut other = first.clone();
    other.context.participant = 11;
    assert!(
        run(&first, score::AWARD)
            .unwrap()
            .requests
            .iter()
            .all(|r| r.participant == 7)
    );
    assert!(
        run(&other, score::AWARD)
            .unwrap()
            .requests
            .iter()
            .all(|r| r.participant == 11)
    );
    assert!(first.requests.is_empty() && other.requests.is_empty());
}
#[test]
fn late_failure_cycles_swallowed_errors_and_invalid_context_rollback_every_request() {
    let mut fresh = state(Some(Value::Integer(6)));
    fresh.fail_keeper = Some(20);
    let before = fresh.clone();
    assert_eq!(run(&fresh, score::AWARD), Err(Error::UnsupportedEvent));
    assert_eq!(fresh, before);
    fresh.fail_keeper = None;
    fresh.recursive = true;
    for swallow in [false, true] {
        fresh.swallow = swallow;
        assert_eq!(run(&fresh, score::AWARD), Err(Error::Bound));
        assert!(fresh.requests.is_empty());
    }
    fresh.recursive = false;
    fresh.context.keepers = vec![1; score::MAX_KEEPERS + 1];
    assert_eq!(run(&fresh, score::AWARD), Err(Error::Bound));
    fresh.context.keepers.clear();
    fresh.binding.context = 2;
    assert_eq!(run(&fresh, score::AWARD), Err(Error::UnknownNode));
    fresh.binding.context = 1;
    fresh.store = Store::new(
        2,
        vec![Definition {
            kind: Kind::Boolean,
            initial: Some(Value::Boolean(true)),
            ports: vec![INPUT],
            listeners: vec![],
        }],
    )
    .unwrap();
    assert_eq!(run(&fresh, score::AWARD), Err(Error::InvalidGraph));
}
