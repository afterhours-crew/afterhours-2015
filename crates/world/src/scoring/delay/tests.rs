// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::events::{self, Dispatch, Endpoint, Graph};
#[derive(Clone, Debug, PartialEq)]
struct State {
    delays: Manager<u32>,
    values: [f32; 3],
    scales: [f32; 3],
    outputs: Vec<(u16, u32, usize)>,
    mode: u8,
}
fn setup(definition: Definition) -> (State, Graph) {
    (
        State {
            delays: Manager::new(3, &[(0, definition), (1, definition), (2, definition)]).unwrap(),
            values: [1.0; 3],
            scales: [1.0; 3],
            outputs: vec![],
            mode: 0,
        },
        Graph::new(3, vec![]).unwrap(),
    )
}
fn definition() -> Definition {
    Definition {
        auto_start: false,
        run_once: false,
        remove_duplicates: false,
    }
}
impl events::Handler for State {
    fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        Ok(())
    }
}
impl Handler for State {
    type Context = u32;
    fn delays(&mut self) -> &mut Manager<u32> {
        &mut self.delays
    }
    fn delay_input(&self, node: u16) -> Result<f32, Error> {
        self.values
            .get(node as usize)
            .copied()
            .ok_or(Error::UnknownNode)
    }
    fn delay_scale(&self, node: u16) -> Result<f32, Error> {
        self.scales
            .get(node as usize)
            .copied()
            .ok_or(Error::UnknownNode)
    }
    fn delay_output(&mut self, node: u16, context: u32, d: &mut Dispatch<'_>) -> Result<(), Error> {
        self.outputs
            .push((node, context, self.delays.pending(node)?));
        if node == 0 {
            match self.mode {
                1 => {
                    d.delay_event(self, 2, IN, 99)?;
                }
                2 => {
                    d.delay_event(self, 1, RESET, 0)?;
                    d.delay_event(self, 2, IN, 99)?;
                }
                3 => {
                    d.delay_event(self, 0, IN, context + 1)?;
                }
                4 => {
                    let _ = d.update_delays(self, 1.0);
                }
                5 => {
                    return Err(Error::InvalidGraph);
                }
                _ => {}
            }
        }
        Ok(())
    }
}
fn initialize(s: &State, g: &Graph) -> State {
    g.transaction(s, |s, d| {
        for node in 0..3 {
            d.initialize_delay(s, node, 0)?;
        }
        Ok(())
    })
    .unwrap()
    .0
}
fn event(s: &State, g: &Graph, node: u16, event: u32, context: u32) -> State {
    g.transaction(s, |s, d| d.delay_event(s, node, event, context))
        .unwrap()
        .0
}
fn tick(s: &State, g: &Graph, delta: f32) -> State {
    g.transaction(s, |s, d| d.update_delays(s, delta))
        .unwrap()
        .0
}

#[test]
fn exact_expiry_removes_pending_before_forwarding_copied_context() {
    let (s, g) = setup(definition());
    let s = initialize(&s, &g);
    let s = event(&s, &g, 0, IN, 41);
    let s = event(&s, &g, 0, IN, 42);
    assert_eq!(s.delays.pending(0), Ok(2));
    let halfway = tick(&s, &g, 0.5);
    assert!(halfway.outputs.is_empty());
    let due = tick(&halfway, &g, 0.5);
    assert_eq!(due.outputs, [(0, 41, 1), (0, 42, 0)]);
    assert_eq!(s.delays.pending(0), Ok(2));
}
#[test]
fn duplicate_replacement_retrigger_and_reset_use_current_pending_queue() {
    let (mut s, g) = setup(Definition {
        remove_duplicates: true,
        ..definition()
    });
    let initialized = initialize(&s, &g);
    s = event(&initialized, &g, 0, IN, 1);
    s = tick(&s, &g, 0.75);
    s = event(&s, &g, 0, IN, 2);
    assert_eq!(s.delays.pending(0), Ok(1));
    assert!(tick(&s, &g, 0.5).outputs.is_empty());
    assert_eq!(tick(&s, &g, 1.0).outputs, [(0, 2, 0)]);
    let (s, g) = setup(definition());
    let s = initialize(&s, &g);
    let s = event(&event(&s, &g, 0, IN, 1), &g, 0, IN, 2);
    let replaced = event(&s, &g, 0, RETRIGGER, 3);
    assert_eq!(replaced.delays.pending(0), Ok(1));
    assert_eq!(tick(&replaced, &g, 1.0).outputs, [(0, 3, 0)]);
    assert_eq!(event(&s, &g, 0, RESET, 0).delays.pending(0), Ok(0));
}
#[test]
fn nonpositive_explicit_events_are_immediate_and_do_not_cancel_pending() {
    let (s, g) = setup(Definition {
        remove_duplicates: true,
        ..definition()
    });
    let s = initialize(&s, &g);
    let mut s = event(&s, &g, 0, IN, 7);
    s.values[0] = 0.0;
    let s = event(&s, &g, 0, RETRIGGER, 8);
    assert_eq!(s.outputs, [(0, 8, 1)]);
    let mut s = event(&s, &g, 0, IN, 9);
    s.values[0] = -1.0;
    let s = event(&s, &g, 0, IN, 10);
    assert_eq!(s.delays.pending(0), Ok(1));
    assert_eq!(
        tick(&s, &g, 1.0).outputs,
        [(0, 8, 1), (0, 9, 1), (0, 10, 1), (0, 7, 0)]
    );
}
#[test]
fn autostart_zero_waits_for_update_and_run_once_is_resettable() {
    let (mut s, g) = setup(Definition {
        auto_start: true,
        run_once: true,
        remove_duplicates: true,
    });
    s.values = [0.0; 3];
    let s = initialize(&s, &g);
    assert!(s.outputs.is_empty());
    assert_eq!(s.delays.pending(0), Ok(1));
    assert!(event(&s, &g, 0, IN, 1).outputs.is_empty());
    let s = tick(&s, &g, 0.0);
    assert_eq!(s.outputs, [(0, 0, 0), (1, 0, 0), (2, 0, 0)]);
    let reset = event(&s, &g, 0, RESET, 0);
    let fired = event(&reset, &g, 0, IN, 2);
    assert_eq!(fired.outputs.last(), Some(&(0, 2, 0)));
    assert_eq!(event(&fired, &g, 0, IN, 3).outputs, fired.outputs);
    // Alternate input ignores RunOnce but does not itself set its latch.
    assert_eq!(
        event(&fired, &g, 0, RETRIGGER, 4).outputs.last(),
        Some(&(0, 4, 0))
    );
}
#[test]
fn newly_appended_entry_runs_same_update_only_with_an_existing_next_entry() {
    let (mut s, g) = setup(definition());
    s.mode = 1;
    let s = initialize(&s, &g);
    let one = event(&s, &g, 0, IN, 1);
    let due = tick(&one, &g, 1.0);
    assert_eq!(due.outputs, [(0, 1, 0)]);
    assert_eq!(due.delays.pending(2), Ok(1));
    let two = event(&one, &g, 1, IN, 2);
    let due = tick(&two, &g, 1.0);
    assert_eq!(due.outputs, [(0, 1, 0), (1, 2, 0), (2, 99, 0)]);
}
#[test]
fn cancelling_next_repairs_cursor_and_last_entry_rearm_waits() {
    let (mut s, g) = setup(definition());
    s.mode = 2;
    let s = initialize(&s, &g);
    let s = event(&event(&s, &g, 0, IN, 1), &g, 1, IN, 2);
    let due = tick(&s, &g, 1.0);
    assert_eq!(due.outputs, [(0, 1, 0)]);
    assert_eq!(due.delays.pending(2), Ok(1));
    let (mut s, g) = setup(definition());
    s.mode = 3;
    let s = initialize(&s, &g);
    let s = event(&s, &g, 0, IN, 1);
    let due = tick(&s, &g, 1.0);
    assert_eq!(due.outputs, [(0, 1, 0)]);
    assert_eq!(due.delays.pending(0), Ok(1));
    assert_eq!(tick(&due, &g, 1.0).outputs, [(0, 1, 0), (0, 2, 0)]);
}
#[test]
fn scale_and_delay_are_explicit_and_invalid_work_rolls_back() {
    let (mut s, g) = setup(definition());
    s.scales[0] = 0.5;
    let s = initialize(&s, &g);
    let s = event(&s, &g, 0, IN, 11);
    assert!(tick(&s, &g, 1.0).outputs.is_empty());
    assert_eq!(tick(&s, &g, 2.0).outputs, [(0, 11, 0)]);
    for delta in [-1.0, f32::NAN, f32::INFINITY] {
        assert!(g.transaction(&s, |s, d| d.update_delays(s, delta)).is_err());
    }
    for mode in [4, 5] {
        let mut invalid = s.clone();
        invalid.mode = mode;
        assert_eq!(
            g.transaction(&invalid, |s, d| d.update_delays(s, 2.0)),
            Err(Error::InvalidGraph)
        );
        assert_eq!(invalid.delays.pending(0), Ok(1));
        assert!(invalid.outputs.is_empty());
    }
    let mut bad = s.clone();
    bad.values[0] = f32::INFINITY;
    assert_eq!(
        g.transaction(&bad, |s, d| d.delay_event(s, 0, IN, 0)),
        Err(Error::NonFinite)
    );
    bad.scales[0] = f32::MAX;
    assert_eq!(
        g.transaction(&bad, |s, d| d.update_delays(s, 2.0)),
        Err(Error::NonFinite)
    );
}
#[test]
fn per_node_bound_prevents_native_counter_wrap_and_context_is_bounded() {
    let (s, g) = setup(definition());
    let s = initialize(&s, &g);
    assert_eq!(
        g.transaction(&s, |s, d| {
            for c in 0..=MAX_PER_NODE {
                d.delay_event(s, 0, IN, c as u32)?;
            }
            Ok(())
        }),
        Err(Error::Bound)
    );
    assert_eq!(s.delays.pending(0), Ok(0));
    assert!(Manager::<[u8; 257]>::new(1, &[(0, definition())]).is_err());
    assert!(Manager::<u32>::new(1, &[(1, definition())]).is_err());
    assert!(Manager::<u32>::new(1, &[(0, definition()), (0, definition())]).is_err());
}
