// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    properties::{Definition, Kind},
    road::{Configuration, Node, Snapshot},
};
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Limit(f32),
    Event(u32, Option<i32>, Snapshot),
}
#[derive(Clone, Debug, PartialEq)]
struct State {
    query: Node,
    store: Store,
    trace: Vec<Entry>,
    fail: bool,
    recurse: bool,
    swallow: bool,
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        self.trace
            .push(Entry::Event(target.event, None, self.query.snapshot()));
        if self.fail {
            Err(Error::UnsupportedEvent)
        } else {
            Ok(())
        }
    }
    fn handle_integer(
        &mut self,
        target: Endpoint,
        value: i32,
        _: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.trace.push(Entry::Event(
            target.event,
            Some(value),
            self.query.snapshot(),
        ));
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
        let Some(Value::Float(value)) = self.store.read(port)? else {
            return Err(Error::InvalidGraph);
        };
        self.trace.push(Entry::Limit(value));
        assert_eq!(self.query.snapshot().speed_limit, value);
        if self.recurse {
            let mut sample = sample(30., false);
            sample.speed_limit = if value == 60. { 61. } else { 60. };
            let result = d.road(
                self,
                0,
                Frame {
                    now: 2.,
                    elapsed: 0.25,
                },
                Some(sample),
            );
            if !self.swallow {
                result?;
            }
        }
        Ok(())
    }
}
impl RoadHandler for State {
    fn road_query(&mut self, node: u16) -> Result<&mut Node, Error> {
        if node == 0 {
            Ok(&mut self.query)
        } else {
            Err(Error::UnknownNode)
        }
    }
}
fn configuration() -> Configuration {
    Configuration {
        max_distance_to_segment: 10.,
        speeding_min_time: 0.5,
        speeding_over_limit: 0.,
        excessive_speeding_over_limit: 20.,
        wrong_way_begin_distance: 10.,
        wrong_way_end_distance: 5.,
        wrong_way_min_speed: 0.,
        speeding_debounce_time: 1.,
    }
}
fn snapshot() -> Snapshot {
    // Constructed test state, not a claim about native fresh allocation.
    Snapshot {
        last_speeding_time: 0.,
        last_not_speeding_time: 0.,
        speeding_duration: 0.,
        excessive_duration: 0.,
        not_speeding_duration: 0.,
        wrong_way_distance: 0.,
        speed_limit: 0.,
        wrong_way: false,
        immediate_speeding_check: false,
    }
}
fn state_with(c: Configuration, initial: Snapshot) -> State {
    let port = Port {
        node: 0,
        field: road::SPEED_LIMIT,
    };
    State {
        query: Node::from_state(c, initial, true).unwrap(),
        store: Store::new(
            2,
            vec![Definition {
                kind: Kind::Float,
                initial: None,
                ports: vec![port],
                listeners: vec![port],
            }],
        )
        .unwrap(),
        trace: vec![],
        fail: false,
        recurse: false,
        swallow: false,
    }
}
fn state() -> State {
    state_with(configuration(), snapshot())
}
fn graph() -> Graph {
    Graph::new(
        2,
        [
            road::SPEEDING,
            road::EXCESSIVE_SPEEDING,
            road::NOT_SPEEDING,
            road::WRONG_WAY_BEGIN,
            road::WRONG_WAY_END,
        ]
        .map(|event| Connection {
            source: Endpoint { node: 0, event },
            target: Endpoint { node: 1, event },
            delivery: Delivery::Local,
        })
        .to_vec(),
    )
    .unwrap()
}
fn sample(speed_mps: f32, wrong_direction: bool) -> Sample {
    Sample {
        speed_limit: 60.,
        speed_limit_override: None,
        distance_to_segment: 1.,
        speed_metres_per_second: speed_mps,
        wrong_direction,
    }
}
fn tick(state: &State, now: f64, dt: f32, sample: Option<Sample>) -> Result<State, Error> {
    graph()
        .transaction(state, |s, d| {
            d.road(s, 0, Frame { now, elapsed: dt }, sample)
        })
        .map(|(s, _)| s)
}
fn events(s: &State) -> Vec<(u32, Option<i32>)> {
    s.trace
        .iter()
        .filter_map(|e| match e {
            Entry::Event(id, value, _) => Some((*id, *value)),
            _ => None,
        })
        .collect()
}
#[test]
fn strict_duration_and_debounce_use_mph_and_excessive_has_priority() {
    let mut s = state();
    // 30 metres/second is about67mph: over60, below excessive80.
    for now in [2., 2.25] {
        s = tick(&s, now, 0.25, Some(sample(30., false))).unwrap();
    }
    assert!(events(&s).is_empty()); // exactly0.5seconds is not enough.
    s = tick(&s, 2.5, 0.25, Some(sample(30., false))).unwrap();
    assert_eq!(events(&s), vec![(road::SPEEDING, None)]);
    assert!(matches!(s.trace[0], Entry::Limit(60.)));
    assert_eq!(s.query.snapshot().last_speeding_time, 2.5);
    s = tick(&s, 3.5, 0.75, Some(sample(70., false))).unwrap();
    assert_eq!(events(&s).len(), 1); // exactly1second debounce remains closed.
    s = tick(&s, 3.500_000_01, 0.75, Some(sample(70., false))).unwrap();
    assert_eq!(
        events(&s),
        vec![(road::SPEEDING, None), (road::EXCESSIVE_SPEEDING, Some(7))]
    );
    let snap = s.query.snapshot();
    assert_eq!(
        (
            snap.speeding_duration,
            snap.excessive_duration,
            snap.not_speeding_duration
        ),
        (0., 0., 0.)
    );
    // End notification updates its own debounce timestamp, not start's.
    s = tick(&s, 5., 0.75, Some(sample(0., false))).unwrap();
    assert_eq!(events(&s).last(), Some(&(road::NOT_SPEEDING, None)));
    assert_eq!(s.query.snapshot().last_speeding_time, 3.500_000_01);
    assert_eq!(s.query.snapshot().last_not_speeding_time, 5.);
}
#[test]
fn wrong_way_distance_hysteresis_counts_metres_and_uses_strict_boundaries() {
    let mut s = tick(&state(), 2., 1., Some(sample(10., true))).unwrap();
    assert!(!s.query.snapshot().wrong_way); // exactly10metres.
    s = tick(&s, 2.1, 0.1, Some(sample(10., true))).unwrap();
    assert!(s.query.snapshot().wrong_way);
    assert_eq!(s.query.snapshot().wrong_way_distance, 0.);
    s = tick(&s, 2.2, 0.1, Some(sample(10., true))).unwrap();
    assert_eq!(s.query.snapshot().wrong_way_distance, 0.);
    s = tick(&s, 2.7, 0.5, Some(sample(10., false))).unwrap();
    assert!(s.query.snapshot().wrong_way); // exactly5metres recovery.
    s = tick(&s, 2.8, 0.1, Some(sample(10., false))).unwrap();
    assert!(!s.query.snapshot().wrong_way);
    let wrong_events: Vec<_> = events(&s)
        .into_iter()
        .filter(|(id, _)| [road::WRONG_WAY_BEGIN, road::WRONG_WAY_END].contains(id))
        .collect();
    assert_eq!(
        wrong_events,
        vec![(road::WRONG_WAY_BEGIN, None), (road::WRONG_WAY_END, None)]
    );
}
#[test]
fn missing_road_preserves_state_and_distance_boundary_ends_wrong_way_without_speed_end() {
    let initial = Snapshot {
        wrong_way: true,
        speeding_duration: 0.3,
        excessive_duration: 0.2,
        not_speeding_duration: 0.1,
        wrong_way_distance: 2.,
        immediate_speeding_check: true,
        ..snapshot()
    };
    let mut s = tick(&state_with(configuration(), initial), 2., 0.25, None).unwrap();
    assert_eq!(s.query.snapshot(), initial);
    assert!(s.trace.is_empty());
    let mut outside = sample(30., false);
    outside.distance_to_segment = -10.;
    s = tick(&s, 3., 0.25, Some(outside)).unwrap();
    assert_eq!(events(&s), vec![(road::WRONG_WAY_END, None)]);
    assert!(matches!(s.trace[0], Entry::Limit(60.)));
    let snap = s.query.snapshot();
    assert_eq!(
        (
            snap.speeding_duration,
            snap.excessive_duration,
            snap.not_speeding_duration,
            snap.wrong_way_distance
        ),
        (0., 0., 0., 0.)
    );
    assert!(snap.immediate_speeding_check); // off-road does not clear this flag.
}
#[test]
fn immediate_speeding_is_explicit_and_low_speed_ends_wrong_way_with_minimum_equality_preserved() {
    let initial = Snapshot {
        immediate_speeding_check: true,
        ..snapshot()
    };
    let s = tick(
        &state_with(configuration(), initial),
        2.,
        0.,
        Some(sample(70., false)),
    )
    .unwrap();
    assert_eq!(events(&s), vec![(road::EXCESSIVE_SPEEDING, Some(7))]);
    assert!(!s.query.snapshot().immediate_speeding_check);
    let c = Configuration {
        wrong_way_min_speed: 10. * road::METRES_PER_SECOND_TO_MPH,
        ..configuration()
    };
    let initial = Snapshot {
        wrong_way: true,
        ..snapshot()
    };
    let s = tick(&state_with(c, initial), 2., 0., Some(sample(10., true))).unwrap();
    assert!(s.query.snapshot().wrong_way);
    let s = tick(&s, 3., 0., Some(sample(9.9, true))).unwrap();
    assert_eq!(events(&s), vec![(road::WRONG_WAY_END, None)]);
}
#[test]
fn positive_override_only_limit_equality_and_unbound_output_do_not_invent_readiness() {
    let mut s = state();
    for (i, override_value) in [None, Some(-1.), Some(0.), Some(75.), Some(75.)]
        .into_iter()
        .enumerate()
    {
        let mut measurement = sample(0., false);
        measurement.speed_limit_override = override_value;
        s = tick(&s, 2. + i as f64, 0., Some(measurement)).unwrap();
    }
    assert_eq!(s.trace, vec![Entry::Limit(60.), Entry::Limit(75.)]);
    let initial = Snapshot {
        speed_limit: -0.,
        ..snapshot()
    };
    let mut s = state_with(configuration(), initial);
    let measurement = Sample {
        speed_limit: 0.,
        ..sample(0., false)
    };
    s = tick(&s, 2., 0., Some(measurement)).unwrap();
    assert_eq!(
        s.query.snapshot().speed_limit.to_bits(),
        (-0.0f32).to_bits()
    );
    assert!(s.trace.is_empty());
    s.query = Node::from_state(configuration(), snapshot(), false).unwrap();
    s.store = Store::new(2, vec![]).unwrap();
    s = tick(&s, 2., 0., Some(sample(0., false))).unwrap();
    assert_eq!(s.query.snapshot().speed_limit, 60.);
    assert!(s.trace.is_empty());
}
#[test]
fn candidate_rolls_back_invalid_numbers_clock_regression_late_events_and_swallowed_cycles() {
    let s = state();
    for value in [f32::NAN, f32::INFINITY, -1.] {
        assert!(tick(&s, 2., value, Some(sample(30., false))).is_err());
        assert!(tick(&s, 2., 0.25, Some(sample(value, false))).is_err());
    }
    assert!(tick(&s, f64::NAN, 0.25, None).is_err());
    let s = tick(&s, 2., 0., None).unwrap();
    assert_eq!(tick(&s, 1., 0.25, None), Err(Error::InvalidGraph));
    let mut s = state();
    s.fail = true;
    let original = s.clone();
    assert_eq!(
        tick(&s, 2., 0.75, Some(sample(70., true))),
        Err(Error::UnsupportedEvent)
    );
    assert_eq!(s, original);
    for swallow in [false, true] {
        let mut s = state();
        s.recurse = true;
        s.swallow = swallow;
        let before = s.clone();
        assert_eq!(
            tick(&s, 2., 0.25, Some(sample(30., false))),
            Err(Error::Bound)
        );
        assert_eq!(s, before);
    }
    assert_eq!(
        tick(&state(), 2., 0.25, Some(sample(f32::MAX, false))),
        Err(Error::NonFinite)
    );
}
