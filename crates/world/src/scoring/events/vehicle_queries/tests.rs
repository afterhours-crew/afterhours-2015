// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    properties::Definition,
    vehicle_query::{DriftSample, IDENTITY},
};

fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
#[derive(Clone, Debug, PartialEq)]
struct State {
    store: Store,
    bindings: Bindings,
    owner: Option<u16>,
    physics: Vec<(u32, Value)>,
    drift: DriftSample,
    reads: Vec<(u16, u32)>,
    trace: Vec<(u32, Value, bool)>,
    feedback: bool,
    loop_update: bool,
    fail: Option<u32>,
}
impl Handler for State {
    fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        Err(Error::UnsupportedEvent)
    }
}
impl PropertyHandler for State {
    fn properties(&mut self) -> &mut Store {
        &mut self.store
    }
    fn property_changed(
        &mut self,
        p: Port,
        first: bool,
        d: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.trace.push((
            p.field,
            self.store.read(p)?.ok_or(Error::MissingInput)?,
            first,
        ));
        if p.field == query::VELOCITY {
            if self.feedback {
                self.feedback = false;
                self.owner = Some(99);
                self.drift.velocity = [-10., 0., 1.];
                self.physics[0].1 = Value::Transform([[2.; 3]; 4]);
                self.physics[5].1 = Value::Float(9000.);
            }
            if self.loop_update {
                // A swallowed nested failure must still fail the outer candidate.
                let _ = d.vehicle_query(self, 0);
            }
        }
        if self.fail == Some(p.field) {
            return Err(Error::UnsupportedEvent);
        }
        Ok(())
    }
}
impl QueryHandler for State {
    type Vehicle = u16;
    fn vehicle_query(&self, node: u16) -> Result<Bindings, Error> {
        if node != 0 {
            return Err(Error::UnknownNode);
        }
        Ok(self.bindings)
    }
    fn query_vehicle(&mut self, _: u16) -> Result<Option<u16>, Error> {
        Ok(self.owner)
    }
    fn physics_measurement(&mut self, vehicle: u16, field: u32) -> Result<Value, Error> {
        self.reads.push((vehicle, field));
        self.physics
            .iter()
            .find(|p| p.0 == field)
            .map(|p| p.1)
            .ok_or(Error::MissingInput)
    }
    fn drift_measurement(&mut self, vehicle: u16) -> Result<DriftSample, Error> {
        self.reads.push((vehicle, query::VELOCITY));
        Ok(self.drift)
    }
}
fn state(query: Query, suppression: bool, seeded: bool) -> State {
    let cells = query
        .outputs()
        .iter()
        .map(|o| {
            let p = port(0, o.field);
            let listener = port(1, o.field);
            Definition {
                kind: o.kind,
                initial: if seeded { o.initial } else { None },
                ports: vec![p, listener],
                listeners: vec![listener],
            }
        })
        .collect();
    State {
        store: Store::new(2, cells).unwrap(),
        bindings: Bindings::new(
            query,
            &query
                .outputs()
                .iter()
                .map(|o| (o.field, suppression))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        owner: Some(7),
        physics: Query::Physics
            .outputs()
            .iter()
            .map(|o| (o.field, o.initial.unwrap_or(Value::Float(0.))))
            .collect(),
        drift: DriftSample {
            transform: IDENTITY,
            velocity: [3., 0., 4.],
        },
        reads: vec![],
        trace: vec![],
        feedback: false,
        loop_update: false,
        fail: None,
    }
}
fn apply(s: &State) -> Result<State, Error> {
    Graph::new(2, vec![])
        .unwrap()
        .transaction(s, |s, d| d.vehicle_query(s, 0))
        .map(|p| p.0)
}

#[test]
fn construction_values_are_distinct_from_readiness_and_missing_vehicle_preserves_them() {
    let mut s = state(Query::Physics, true, true);
    assert_eq!(
        s.store.read(port(0, query::TRANSFORM)).unwrap(),
        Some(Value::Transform(IDENTITY))
    );
    assert_eq!(s.store.read(port(0, query::MAX_RPM)).unwrap(), None);
    s.owner = None;
    assert_eq!(apply(&s).unwrap(), s);
    s.owner = Some(7);
    let s = apply(&s).unwrap();
    // Constructor seeds suppress equal measurements, but unready floats publish zero.
    assert_eq!(
        s.trace,
        [
            (query::MAX_RPM, Value::Float(0.), true),
            (query::RPM, Value::Float(0.), true)
        ]
    );
    let mut repeated = apply(&s).unwrap();
    repeated.reads = s.reads.clone();
    assert_eq!(repeated, s);
    assert_eq!(Query::Drift.outputs()[1].initial, Some(Value::Float(0.)));
}

#[test]
fn physics_resolves_owner_once_and_reads_later_measurements_after_callbacks() {
    let mut s = state(Query::Physics, true, false);
    s.feedback = true;
    let result = apply(&s).unwrap();
    let fields = [
        query::VELOCITY,
        query::TRANSFORM,
        query::AIRBORNE,
        query::STATE_3C230E35,
        query::STATE_CD5AE2F0,
        query::MAX_RPM,
        query::RPM,
    ];
    assert_eq!(result.reads, fields.map(|f| (7, f)));
    assert_eq!(result.trace.iter().map(|r| r.0).collect::<Vec<_>>(), fields);
    assert!(result.trace.iter().all(|r| r.2));
    assert_eq!(result.trace[1].1, Value::Transform([[2.; 3]; 4]));
    assert_eq!(result.trace[5].1, Value::Float(9000.));
    assert_eq!(result.owner, Some(99));
    // The independent input candidate retains its original owner and properties.
    assert_eq!(s.owner, Some(7));
    assert_eq!(s.store.read(port(0, query::TRANSFORM)).unwrap(), None);
}

#[test]
fn drift_snapshots_before_velocity_callbacks_and_preserves_angle_sign() {
    let mut s = state(Query::Drift, true, false);
    s.feedback = true;
    let result = apply(&s).unwrap();
    assert_eq!(result.reads, [(7, query::VELOCITY)]);
    assert_eq!(
        result.trace[0],
        (query::VELOCITY, Value::Vec3([3., 0., 4.]), true)
    );
    assert_eq!(
        result.trace[1],
        (query::DRIFT_ANGLE, Value::Float(0.75_f32.atan()), true)
    );
    assert_eq!(result.drift.velocity, [-10., 0., 1.]);
    for (velocity, expected) in [
        ([1., 0., 1.], std::f32::consts::FRAC_PI_4),
        ([-1., 0., 1.], -std::f32::consts::FRAC_PI_4),
        ([1., 0., -1.], std::f32::consts::FRAC_PI_4),
        ([1., 0., 0.], std::f32::consts::FRAC_PI_4),
        ([0., 0., 0.], 0.),
    ] {
        let angle = DriftSample {
            transform: IDENTITY,
            velocity,
        }
        .angle()
        .unwrap();
        assert!((angle - expected).abs() < 1e-7, "{velocity:?}: {angle}");
    }
    // Exact epsilon divides; the immediately lower representable value does not.
    let epsilon = f32::EPSILON;
    let at = DriftSample {
        transform: IDENTITY,
        velocity: [epsilon, 0., epsilon],
    }
    .angle()
    .unwrap();
    let below = DriftSample {
        transform: IDENTITY,
        velocity: [epsilon, 0., f32::from_bits(epsilon.to_bits() - 1)],
    }
    .angle()
    .unwrap();
    assert!((at - std::f32::consts::FRAC_PI_4).abs() < 1e-7);
    assert_eq!(below, epsilon.atan());
}

#[test]
fn equality_flags_optional_outputs_and_signed_zero_do_not_invent_publications() {
    let mut s = state(Query::Physics, true, true);
    s.bindings = Bindings::new(
        Query::Physics,
        &[(query::VELOCITY, true), (query::AIRBORNE, false)],
    )
    .unwrap();
    s.physics[1].1 = Value::Vec3([-0., 0., -0.]);
    let result = apply(&s).unwrap();
    assert_eq!(
        result.trace,
        [(query::AIRBORNE, Value::Boolean(false), false)]
    );
    assert_eq!(apply(&result).unwrap().trace.len(), 2);
    s.bindings = Bindings::new(Query::Physics, &[]).unwrap();
    assert!(apply(&s).unwrap().trace.is_empty());
    assert!(Bindings::new(Query::Drift, &[(query::RPM, true)]).is_err());
    assert!(
        Bindings::new(
            Query::Drift,
            &[(query::VELOCITY, true), (query::VELOCITY, false)]
        )
        .is_err()
    );
    assert!(Bindings::new(Query::Drift, &[(query::VELOCITY, true); 3]).is_err());
}

#[test]
fn late_failure_nonfinite_wrong_types_and_swallowed_cycles_discard_candidate() {
    let mut s = state(Query::Physics, false, false);
    s.fail = Some(query::RPM);
    assert_eq!(apply(&s), Err(Error::UnsupportedEvent));
    assert!(s.trace.is_empty());
    s.fail = None;
    s.physics[6].1 = Value::Float(f32::INFINITY);
    assert_eq!(apply(&s), Err(Error::NonFinite));
    s.physics[6].1 = Value::Boolean(false);
    assert_eq!(apply(&s), Err(Error::InvalidGraph));
    s.physics[6].1 = Value::Float(0.);
    s.loop_update = true;
    assert_eq!(apply(&s), Err(Error::Bound));
    let mut drift = state(Query::Drift, true, false);
    drift.drift.velocity = [f32::MAX; 3];
    drift.drift.transform = [[2.; 3]; 4];
    assert_eq!(apply(&drift), Err(Error::NonFinite));
    assert!(drift.trace.is_empty());
    let graph = Graph::new(2, vec![]).unwrap();
    assert_eq!(
        graph.transaction(&drift, |s, d| d.vehicle_query(s, 2)),
        Err(Error::UnknownNode)
    );
}
