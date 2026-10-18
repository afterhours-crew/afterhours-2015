// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    events::{self, Connection, Delivery, Dispatch, Endpoint, Graph},
    math::Value,
    properties::{Definition, Kind, Port, Store},
};

#[derive(Clone, Debug, PartialEq)]
struct State {
    timers: Manager,
    props: Store,
    inputs: Vec<Inputs>,
    outputs: Vec<(u16, f32)>,
    triggers: Vec<u16>,
    mode: u8,
    nested: bool,
}
fn port(node: u16) -> Port {
    Port { node, field: TIME }
}
fn endpoint(node: u16, event: u32) -> Endpoint {
    Endpoint { node, event }
}
fn setup(mode: u8) -> (State, Graph) {
    let state = State {
        timers: Manager::new(3, &[(0, true), (1, true), (2, false)]).unwrap(),
        props: Store::new(
            3,
            (0..2)
                .map(|node| Definition {
                    kind: Kind::Float,
                    initial: Some(Value::Float(0.0)),
                    ports: vec![port(node)],
                    listeners: vec![port(node)],
                })
                .collect(),
        )
        .unwrap(),
        inputs: vec![
            Inputs {
                multiplier: 1.0,
                trigger_time: 1.0,
                time_scale: 1.0
            };
            3
        ],
        outputs: vec![],
        triggers: vec![],
        mode,
        nested: false,
    };
    let graph = Graph::new(
        3,
        (0..3)
            .map(|node| Connection {
                source: endpoint(node, TRIGGER),
                target: endpoint(node, 999),
                delivery: Delivery::Local,
            })
            .collect(),
    )
    .unwrap();
    (state, graph)
}
impl events::Handler for State {
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        self.triggers.push(target.node);
        if self.mode == 1 {
            dispatch.stopwatch_event(self, target.node, RESET)?;
        }
        Ok(())
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.props
    }
    fn property_changed(
        &mut self,
        target: Port,
        _: bool,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let Some(Value::Float(value)) = self.props.read(target)? else {
            return Err(Error::InvalidGraph);
        };
        self.outputs.push((target.node, value));
        if !self.nested && target.node == 0 {
            if self.mode == 2 {
                self.nested = true;
                dispatch.stopwatch_event(self, 1, STOP)?;
                dispatch.stopwatch_event(self, 2, START)?;
                self.inputs[1].multiplier = 3.0;
            } else if self.mode == 3 && value == 0.0 {
                self.nested = true;
                dispatch.update_stopwatches(self, 2.0)?;
            } else if self.mode == 4 {
                let _ = dispatch.stopwatch_event(self, 99, START);
            }
        }
        Ok(())
    }
}
impl Handler for State {
    fn stopwatches(&mut self) -> &mut Manager {
        &mut self.timers
    }
    fn stopwatch_inputs(&self, node: u16) -> Result<Inputs, Error> {
        self.inputs
            .get(node as usize)
            .copied()
            .ok_or(Error::UnknownNode)
    }
}
fn control(state: &State, graph: &Graph, node: u16, event: u32) -> State {
    graph
        .transaction(state, |s, d| d.stopwatch_event(s, node, event))
        .unwrap()
        .0
}
fn tick(state: &State, graph: &Graph, delta: f32) -> State {
    graph
        .transaction(state, |s, d| d.update_stopwatches(s, delta))
        .unwrap()
        .0
}

#[test]
fn threshold_is_strict_latched_and_callback_reset_changes_tick_publication() {
    let (initial, graph) = setup(1);
    assert!(!initial.timers.running(0).unwrap());
    let stopped = tick(&initial, &graph, 2.0);
    assert!(stopped.outputs.is_empty());
    let started = control(&initial, &graph, 0, START);
    let equal = tick(&started, &graph, 1.0);
    assert!(equal.triggers.is_empty());
    let triggered = tick(&equal, &graph, 0.5);
    assert_eq!(triggered.triggers, [0]);
    assert_eq!(triggered.outputs, [(0, 1.0), (0, 0.0), (0, 0.0)]);
    assert_eq!(triggered.timers.elapsed(0), Ok(0.0));
    assert!(triggered.timers.running(0).unwrap());
    assert_eq!(tick(&triggered, &graph, 2.0).triggers, [0, 0]);
    assert_eq!(initial.timers.elapsed(0), Ok(0.0));

    let (initial, graph) = setup(0);
    let started = control(&initial, &graph, 0, START);
    let one = tick(&started, &graph, 2.0);
    let two = tick(&one, &graph, 2.0);
    assert_eq!(two.triggers, [0]);
    assert_eq!(two.timers.elapsed(0), Ok(4.0));
}

#[test]
fn reset_clears_latch_after_publication_and_nested_callbacks() {
    let (initial, graph) = setup(0);
    let mut state = tick(&control(&initial, &graph, 0, START), &graph, 2.0);
    assert_eq!(state.triggers, [0]);
    state.mode = 3;
    let reset = control(&state, &graph, 0, RESET);
    // Reentrant update during Reset sees the old latch and cannot fire again.
    assert_eq!(reset.triggers, [0]);
    assert_eq!(reset.timers.elapsed(0), Ok(2.0));
    assert_eq!(reset.outputs, [(0, 2.0), (0, 0.0), (0, 2.0)]);
    // Reset's continuation cleared it after that nested update.
    assert_eq!(tick(&reset, &graph, 0.0).triggers, [0, 0]);
}

#[test]
fn nested_membership_changes_wait_for_next_snapshot_but_inputs_are_current() {
    let (initial, graph) = setup(2);
    let state = graph
        .transaction(&initial, |s, d| {
            d.stopwatch_event(s, 0, START)?;
            d.stopwatch_event(s, 1, START)?;
            d.stopwatch_event(s, 1, START)?;
            d.update_stopwatches(s, 1.0)
        })
        .unwrap()
        .0;
    assert_eq!(state.timers.elapsed(1), Ok(3.0));
    assert_eq!(state.timers.elapsed(2), Ok(0.0));
    assert!(!state.timers.running(1).unwrap());
    assert!(state.timers.running(2).unwrap());
    assert_eq!(state.outputs, [(0, 1.0), (1, 3.0)]);
    let next = tick(&state, &graph, 1.0);
    assert_eq!(next.timers.elapsed(1), Ok(3.0));
    assert_eq!(next.timers.elapsed(2), Ok(1.0));
    assert_eq!(next.outputs.last(), Some(&(0, 2.0)));
}

#[test]
fn pending_order_appends_additions_and_swap_removes_stopped_members() {
    let (mut state, graph) = setup(0);
    for node in 0..3 {
        state = control(&state, &graph, node, START);
    }
    state = tick(&state, &graph, 0.0);
    assert_eq!(state.timers.active, [0, 1, 2]);
    state = control(&state, &graph, 0, STOP);
    state = control(&state, &graph, 0, STOP);
    state = tick(&state, &graph, 0.0);
    assert_eq!(state.timers.active, [2, 1]);
    state = control(&state, &graph, 0, START);
    state = tick(&state, &graph, 0.0);
    assert_eq!(state.timers.active, [2, 1, 0]);
    state = control(&state, &graph, 1, STOP);
    state = control(&state, &graph, 1, START);
    state = tick(&state, &graph, 0.0);
    assert_eq!(state.timers.active, [2, 0, 1]);
}

#[test]
fn clock_scale_multiplier_zero_threshold_and_numeric_failure_are_explicit() {
    let (mut initial, graph) = setup(0);
    initial.inputs[0] = Inputs {
        multiplier: 2.0,
        time_scale: 0.5,
        trigger_time: 0.0,
    };
    let started = control(&initial, &graph, 0, START);
    let mut elapsed = tick(&started, &graph, 3.0);
    assert_eq!(elapsed.timers.elapsed(0), Ok(3.0));
    assert!(elapsed.triggers.is_empty());
    elapsed.inputs[0].multiplier = -1.0;
    assert_eq!(tick(&elapsed, &graph, 2.0).timers.elapsed(0), Ok(2.0));
    for delta in [-1.0, f32::NAN, f32::INFINITY] {
        assert!(
            graph
                .transaction(&started, |s, d| d.update_stopwatches(s, delta))
                .is_err()
        );
        assert_eq!(started.timers.elapsed(0), Ok(0.0));
    }
    let mut overflow = started.clone();
    overflow.inputs[0].multiplier = f32::MAX;
    assert_eq!(
        graph.transaction(&overflow, |s, d| d.update_stopwatches(s, 2.0)),
        Err(Error::NonFinite)
    );
    assert!(overflow.outputs.is_empty());
}

#[test]
fn invalid_nodes_swallowed_callback_errors_and_membership_flood_rollback() {
    let (initial, graph) = setup(4);
    let started = control(&initial, &graph, 0, START);
    assert_eq!(
        graph.transaction(&started, |s, d| d.update_stopwatches(s, 0.5)),
        Err(Error::UnknownNode)
    );
    assert_eq!(started.timers.elapsed(0), Ok(0.0));
    assert!(started.outputs.is_empty());
    assert!(Manager::new(3, &[(0, true), (0, false)]).is_err());
    assert!(Manager::new(3, &[(3, false)]).is_err());
    assert_eq!(
        graph.transaction(&initial, |s, d| d.stopwatch_event(s, 0, 42)),
        Err(Error::UnsupportedEvent)
    );
    assert_eq!(
        graph.transaction(&initial, |s, d| {
            for i in 0..=MAX_STEPS {
                d.stopwatch_event(s, 0, if i % 2 == 0 { START } else { STOP })?;
            }
            Ok(())
        }),
        Err(Error::Bound)
    );
    assert!(!initial.timers.running(0).unwrap());
}
