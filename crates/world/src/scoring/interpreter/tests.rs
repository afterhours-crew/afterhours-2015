// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
mod ai;
use crate::scoring::properties::{DefaultMode, Kind};
use crate::scoring::{
    events::{Connection, Delivery},
    nodes::{ACCUMULATE, Definition as Scalar, FLOAT_VALUE, INPUT, OUT, OUTPUT, OUTPUT_CHANGED},
    program::{Identity, Keeper, Node},
    properties::Registration,
};

fn identity(index: u32) -> Identity {
    Identity {
        asset: "test".into(),
        index,
        instances: Vec::new(),
    }
}
pub(super) fn node(index: u32, behavior: Behavior) -> Node {
    Node {
        identity: identity(index),
        class: "Test".into(),
        created: true,
        events: true,
        properties: true,
        forward: None,
        defaults: vec![],
        behavior,
    }
}
pub(super) fn cell(kind: Kind, ports: &[Port]) -> Definition {
    Definition {
        kind,
        initial: None,
        ports: ports.to_vec(),
        listeners: vec![],
    }
}
pub(super) fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
pub(super) fn inputs() -> Inputs {
    Inputs {
        participant: 77,
        vehicle: None,
        others: vec![],
        participants: vec![],
        road: None,
        stats: None,
        now: 0.0,
        time_scale: 1.0,
    }
}

#[test]
fn scale_clamp_stays_unready_until_a_property_callback() {
    use crate::scoring::scale_clamp as scale;
    let mut clamp = node(1, Behavior::ScaleClamp);
    clamp.defaults = scale::INPUTS
        .into_iter()
        .zip([0., 0., 10., 10., 20.].into_iter().map(Value::Float))
        .collect();
    let base = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            clamp,
            node(
                2,
                Behavior::Constant {
                    field: 1,
                    value: Value::Float(5.),
                },
            ),
        ],
        cells: vec![
            cell(Kind::Float, &[port(1, scale::OUT)]),
            cell(Kind::Float, &[port(2, 1), port(1, scale::INPUT)]),
        ],
        connections: vec![],
        startup: vec![Step::Bind(1), Step::Activate(1)],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(base.clone())).unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(s.read(port(1, scale::OUT)).unwrap(), None);
    let mut connected = base;
    connected.startup = vec![
        Step::Bind(1),
        Step::Register(Registration::Notify(port(1, scale::INPUT))),
        Step::Bind(2),
        Step::Activate(1),
    ];
    let mut s = Session::new(Arc::new(connected)).unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(
        s.read(port(1, scale::OUT)).unwrap(),
        Some(Value::Float(15.))
    );
}

#[test]
fn float_select_binds_then_refreshes_on_a_connected_value_and_ignores_events() {
    use crate::scoring::float_select as select;
    let mut selector = node(1, Behavior::FloatSelect);
    selector.defaults = vec![
        (select::POSITIVE, Value::Float(2.)),
        (select::NEGATIVE, Value::Float(3.)),
        (select::SELECT, Value::Float(1.)),
    ];
    let p = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            selector,
            node(
                2,
                Behavior::Constant {
                    field: 1,
                    value: Value::Float(-1.),
                },
            ),
            node(
                3,
                Behavior::Constant {
                    field: 0xc5e4_e950,
                    value: Value::Text(super::super::value::Text::new("Restart").unwrap()),
                },
            ),
        ],
        cells: vec![
            cell(Kind::Float, &[port(1, select::OUT)]),
            cell(Kind::Float, &[port(2, 1), port(1, select::SELECT)]),
            cell(Kind::Text, &[port(3, 0xc5e4_e950)]),
        ],
        connections: vec![Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint { node: 1, event: 1 },
            delivery: Delivery::Local,
        }],
        startup: vec![
            Step::Bind(1),
            Step::Register(Registration::Notify(port(1, select::SELECT))),
            Step::Bind(2),
            Step::Bind(3),
            Step::Activate(1),
        ],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(
        s.read(port(1, select::OUT)).unwrap(),
        Some(Value::Float(3.))
    );
    assert_eq!(
        s.read(port(3, 0xc5e4_e950)).unwrap(),
        Some(Value::Text(
            super::super::value::Text::new("Restart").unwrap()
        ))
    );
    assert_eq!(s.event(0, 1, &inputs()).unwrap(), Output::default());
}

#[test]
fn random_float_uses_injected_identity_and_context_and_repeats_on_every_event() {
    use crate::scoring::random_float as random;
    let mut random_node = node(1, Behavior::RandomFloat { true_random: false });
    random_node.defaults = vec![
        (random::MIN, Value::Float(0.)),
        (random::MAX, Value::Float(1.)),
    ];
    let p = Arc::new(Program {
        nodes: vec![node(0, Behavior::Inert), random_node],
        cells: vec![cell(Kind::Float, &[port(1, random::OUT)])],
        connections: [1, 999]
            .into_iter()
            .map(|event| Connection {
                source: Endpoint { node: 0, event },
                target: Endpoint { node: 1, event },
                delivery: Delivery::Local,
            })
            .collect(),
        startup: vec![Step::Bind(1), Step::Activate(1)],
        unmodeled: vec![],
    });
    let mut s = Session::new(p.clone()).unwrap();
    assert!(s.start(&inputs()).is_err());
    s.bind_random_entity(1, 1).unwrap();
    s.set_random_context(random::Context {
        deterministic: 0,
        epoch: 0,
    });
    s.start(&inputs()).unwrap();
    assert_eq!(
        s.read(port(1, random::OUT)).unwrap(),
        Some(Value::Float(random::unit(1)))
    );
    assert!(s.bind_random_entity(1, 2).is_err());
    for event in [1, 999, 999] {
        s.event(0, event, &inputs()).unwrap();
        assert_eq!(
            s.read(port(1, random::OUT)).unwrap(),
            Some(Value::Float(random::unit(1)))
        );
    }
    s.set_random_context(random::Context {
        deterministic: 1,
        epoch: 0,
    });
    s.event(0, 999, &inputs()).unwrap();
    assert_eq!(
        s.read(port(1, random::OUT)).unwrap(),
        Some(Value::Float(random::unit(2)))
    );

    let mut p = (*p).clone();
    p.nodes[1].behavior = Behavior::RandomFloat { true_random: true };
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.bind_random_entity(1, 1).unwrap();
    s.queue_random_seeds(&[0, 1]).unwrap();
    s.start(&inputs()).unwrap();
    s.event(0, 1, &inputs()).unwrap();
    assert_eq!(
        s.read(port(1, random::OUT)).unwrap(),
        Some(Value::Float(random::unit(2)))
    );
    let before = s.state.clone();
    assert!(s.event(0, 1, &inputs()).is_err());
    assert_eq!(s.state, before);
    assert!(
        s.queue_random_seeds(&vec![0; random::MAX_PENDING_SEEDS + 1])
            .is_err()
    );
    s.queue_random_seeds(&[2]).unwrap();
    s.event(0, 1, &inputs()).unwrap();
    assert_eq!(
        s.read(port(1, random::OUT)).unwrap(),
        Some(Value::Float(random::unit(3)))
    );
}

#[test]
fn item_info_reads_at_activation_suppresses_equal_values_and_retains_detached_text() {
    use crate::scoring::{
        item_info::{ITEM_ID, ITEM_NAME, Sample, TEXT, VALID},
        value::Text,
    };
    let source = participant_properties::Property {
        channel: "items".into(),
        port: 19,
    };
    let p = Arc::new(Program {
        nodes: vec![
            node(0, Behavior::ParticipantProvider),
            node(
                1,
                Behavior::ItemInfo {
                    source: source.clone(),
                    provider: 0,
                },
            ),
            node(
                2,
                Behavior::Scalar(Scalar::CompareBoolean {
                    on_change: true,
                    on_start: false,
                    always_send: true,
                }),
            ),
        ],
        cells: vec![
            cell(Kind::Unsigned64, &[port(1, ITEM_ID)]),
            cell(Kind::Text, &[port(1, ITEM_NAME)]),
            cell(Kind::Text, &[port(1, TEXT)]),
            cell(Kind::Boolean, &[port(1, VALID), port(2, nodes::BOOL)]),
        ],
        connections: [nodes::ON_TRUE, nodes::ON_FALSE]
            .into_iter()
            .enumerate()
            .map(|(i, event)| Connection {
                source: Endpoint { node: 2, event },
                target: Endpoint { node: 0, event: 8 },
                delivery: Delivery::NetworkedClient(super::super::events::Remote {
                    bus: 0,
                    index: i as u16,
                }),
            })
            .collect(),
        startup: vec![
            Step::Bind(0),
            Step::Bind(2),
            Step::Register(Registration::Notify(port(2, nodes::BOOL))),
            Step::Bind(1),
            Step::Activate(0),
            Step::Activate(1),
            Step::Activate(2),
        ],
        unmodeled: vec![],
    });
    let sample = Sample {
        id: u64::MAX,
        name: Text::new("Vehicle/Owned").unwrap(),
        text: Text::new("own-label").unwrap(),
    };
    let mut session = Session::new(p.clone()).unwrap();
    session
        .notify_item_info(&source, 77, Some(sample.clone()), &inputs())
        .unwrap();
    assert_eq!(session.start(&inputs()).unwrap().remote.len(), 1);
    assert_eq!(
        session.read(port(1, ITEM_ID)).unwrap(),
        Some(Value::Unsigned64(u64::MAX))
    );
    assert_eq!(
        session.read(port(1, TEXT)).unwrap(),
        Some(Value::Text(sample.text.clone()))
    );
    assert!(
        session
            .notify_item_info(&source, 77, Some(sample.clone()), &inputs())
            .unwrap()
            .remote
            .is_empty()
    );
    assert!(
        session
            .notify_item_info(&source, 88, None, &inputs())
            .unwrap()
            .remote
            .is_empty()
    );
    assert_eq!(
        session.read(port(1, VALID)).unwrap(),
        Some(Value::Boolean(true))
    );
    assert_eq!(
        session
            .notify_item_info(&source, 77, None, &inputs())
            .unwrap()
            .remote
            .len(),
        1
    );
    assert_eq!(
        session.read(port(1, TEXT)).unwrap(),
        Some(Value::Text(sample.text))
    );
    assert_eq!(
        session.read(port(1, VALID)).unwrap(),
        Some(Value::Boolean(false))
    );
    assert!(
        session
            .notify_item_info(&source, 77, None, &inputs())
            .unwrap()
            .remote
            .is_empty()
    );
    let mut empty = Session::new(p).unwrap();
    empty.start(&inputs()).unwrap();
    assert_eq!(empty.read(port(1, ITEM_ID)).unwrap(), None);
    assert_eq!(empty.read(port(1, TEXT)).unwrap(), None);
    assert_eq!(
        empty.read(port(1, VALID)).unwrap(),
        Some(Value::Boolean(false))
    );
}

#[test]
fn vehicle_info_binding_and_notifications_preserve_signed_hash_and_empty_state() {
    use crate::scoring::{
        value::Text,
        vehicle_info::{BUNDLE, BUNDLE_SIGNED, CHANGED, SHORT_NAME, Sample},
    };
    let p = Arc::new(Program {
        nodes: vec![node(0, Behavior::VehicleInfo), node(1, Behavior::Inert)],
        cells: vec![
            cell(Kind::Unsigned32, &[port(0, BUNDLE)]),
            cell(Kind::Integer, &[port(0, BUNDLE_SIGNED)]),
            cell(Kind::Text, &[port(0, SHORT_NAME)]),
        ],
        connections: vec![Connection {
            source: Endpoint {
                node: 0,
                event: CHANGED,
            },
            target: Endpoint { node: 1, event: 8 },
            delivery: Delivery::NetworkedClient(super::super::events::Remote { bus: 0, index: 17 }),
        }],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    let mut s = Session::new(p).unwrap();
    let info = Sample {
        bundle: 0xfffffff1,
        short_name: Text::new("ID_SHORT_CAR").unwrap(),
    };
    assert!(
        s.notify_vehicle_info(Some(&info), &inputs())
            .unwrap()
            .remote
            .is_empty()
    );
    assert!(s.start(&inputs()).unwrap().remote.is_empty());
    assert_eq!(
        s.read(port(0, SHORT_NAME)).unwrap(),
        Some(Value::Text(Text::new("").unwrap()))
    );
    for _ in 0..2 {
        assert_eq!(
            s.notify_vehicle_info(Some(&info), &inputs())
                .unwrap()
                .remote
                .len(),
            1
        );
        assert_eq!(
            s.read(port(0, BUNDLE)).unwrap(),
            Some(Value::Unsigned32(0xfffffff1))
        );
        assert_eq!(
            s.read(port(0, BUNDLE_SIGNED)).unwrap(),
            Some(Value::Integer(-15))
        );
        assert_eq!(
            s.read(port(0, SHORT_NAME)).unwrap(),
            Some(Value::Text(info.short_name.clone()))
        );
    }
    for _ in 0..2 {
        assert_eq!(
            s.notify_vehicle_info(None, &inputs()).unwrap().remote.len(),
            1
        );
        assert_eq!(s.read(port(0, BUNDLE)).unwrap(), Some(Value::Unsigned32(0)));
        assert_eq!(
            s.read(port(0, BUNDLE_SIGNED)).unwrap(),
            Some(Value::Integer(0))
        );
        assert_eq!(
            s.read(port(0, SHORT_NAME)).unwrap(),
            Some(Value::Text(Text::new("").unwrap()))
        );
    }
}

#[test]
fn burnout_tick_missing_sample_reset_and_vehicle_change_follow_native_outputs() {
    use crate::scoring::burnout::{ACTIVE, COUNT, Configuration, Controls, RESET, TIME};
    let config = Configuration {
        max_linear_mph: 20.,
        max_angular: 1.,
        min_throttle: 0.7,
        min_time: 0.5,
        min_slip_front: 1.,
        min_slip_rear: 1.,
    };
    let program = Arc::new(Program {
        nodes: vec![node(0, Behavior::Burnout(config))],
        cells: vec![
            cell(Kind::Boolean, &[port(0, ACTIVE)]),
            cell(Kind::Integer, &[port(0, COUNT)]),
            cell(Kind::Float, &[port(0, TIME)]),
        ],
        connections: vec![Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint {
                node: 0,
                event: RESET,
            },
            delivery: Delivery::Local,
        }],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    let mut s = Session::new(program).unwrap();
    let mut input = inputs();
    s.start(&input).unwrap();
    input.vehicle = Some(Vehicle {
        identity: 9,
        transform: vehicle_query::IDENTITY,
        velocity: [0.; 3],
        angular_velocity: [0.; 3],
        airborne: None,
        state_3c230e35: None,
        state_cd5ae2f0: None,
        max_rpm: None,
        rpm: None,
        nos: None,
        burnout: Some(Controls {
            throttle: 1.,
            slip: [0., 0., 2., 2.],
        }),
    });
    s.tick(0.6, &input).unwrap();
    assert_eq!(s.read(port(0, ACTIVE)).unwrap(), Some(Value::Boolean(true)));
    assert_eq!(s.read(port(0, COUNT)).unwrap(), Some(Value::Integer(0)));
    input.vehicle.as_mut().unwrap().burnout = None;
    s.tick(0.4, &input).unwrap();
    assert_eq!(s.read(port(0, TIME)).unwrap(), Some(Value::Float(0.6)));
    input.vehicle.as_mut().unwrap().burnout = Some(Controls {
        throttle: 0.,
        slip: [0.; 4],
    });
    s.tick(0.1, &input).unwrap();
    assert_eq!(s.read(port(0, COUNT)).unwrap(), Some(Value::Integer(1)));
    assert_eq!(s.read(port(0, TIME)).unwrap(), Some(Value::Float(0.)));
    s.vehicle_changed(&input).unwrap();
    assert_eq!(s.read(port(0, COUNT)).unwrap(), Some(Value::Integer(0)));
    input.vehicle.as_mut().unwrap().burnout = Some(Controls {
        throttle: 1.,
        slip: [2.; 4],
    });
    s.tick(1., &input).unwrap();
    s.event(0, 1, &input).unwrap();
    assert_eq!(
        s.read(port(0, ACTIVE)).unwrap(),
        Some(Value::Boolean(false))
    );
    assert_eq!(s.read(port(0, TIME)).unwrap(), Some(Value::Float(0.)));
}

#[test]
fn burnout_binding_replacement_and_retirement_keep_sessions_isolated() {
    use crate::scoring::burnout_override::{Change, ENABLED, VALUE};
    let mut control = node(0, Behavior::BurnoutOverride);
    control.defaults = vec![(ENABLED, Value::Boolean(true)), (VALUE, Value::Float(0.8))];
    let program = Arc::new(Program {
        nodes: vec![control],
        cells: vec![],
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    let mut a = Session::new(program.clone()).unwrap();
    let mut b = Session::new(program).unwrap();
    let mut input = inputs();
    assert!(a.start(&input).unwrap().burnout_overrides.is_empty());
    input.vehicle = Some(Vehicle {
        identity: 17,
        transform: vehicle_query::IDENTITY,
        velocity: [0.; 3],
        angular_velocity: [0.; 3],
        airborne: None,
        state_3c230e35: None,
        state_cd5ae2f0: None,
        max_rpm: None,
        rpm: None,
        nos: None,
        burnout: None,
    });
    assert_eq!(
        b.start(&input).unwrap().burnout_overrides,
        [Change {
            vehicle: 17,
            value: Some(0.8)
        }]
    );
    assert_eq!(
        a.vehicle_changed(&input).unwrap().burnout_overrides,
        [Change {
            vehicle: 17,
            value: Some(0.8)
        }]
    );
    input.vehicle.as_mut().unwrap().identity = 18;
    assert_eq!(
        a.vehicle_changed(&input).unwrap().burnout_overrides,
        [
            Change {
                vehicle: 17,
                value: None
            },
            Change {
                vehicle: 18,
                value: Some(0.8)
            }
        ]
    );
    assert!(
        a.vehicle_changed(&input)
            .unwrap()
            .burnout_overrides
            .is_empty()
    );
    assert_eq!(
        a.retire(&input).unwrap().burnout_overrides,
        [Change {
            vehicle: 18,
            value: None
        }]
    );
    assert_eq!(
        b.retire(&input).unwrap().burnout_overrides,
        [Change {
            vehicle: 17,
            value: None
        }]
    );
}

#[test]
fn context_reporter_binds_before_categories_and_observes_only_its_participant() {
    use crate::scoring::participant_properties::Property;
    let source = Property {
        channel: "context".into(),
        port: 8,
    };
    let mut reporter = node(
        1,
        Behavior::ContextReporter {
            provider: 0,
            source: source.clone(),
            categories: vec![2, 4],
        },
    );
    reporter
        .defaults
        .push((CONTEXT_ACTIVE, Value::Boolean(true)));
    let p = Arc::new(Program {
        nodes: vec![node(0, Behavior::ParticipantProvider), reporter],
        cells: vec![cell(Kind::Boolean, &[port(1, CONTEXT_MATCH)])],
        connections: vec![],
        startup: vec![Step::Bind(1), Step::Activate(1)],
        unmodeled: vec![],
    });
    let mut s = Session::new(p.clone()).unwrap();
    s.import_context_categories(&source, 77, &[], &inputs())
        .unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(
        s.read(port(1, CONTEXT_MATCH)).unwrap(),
        Some(Value::Boolean(true))
    );
    s.import_context_categories(&source, 88, &[], &inputs())
        .unwrap();
    assert_eq!(
        s.read(port(1, CONTEXT_MATCH)).unwrap(),
        Some(Value::Boolean(true))
    );
    for (categories, expected) in [(&[2][..], false), (&[2, 4, 9][..], true), (&[][..], false)] {
        s.import_context_categories(&source, 77, categories, &inputs())
            .unwrap();
        assert_eq!(
            s.read(port(1, CONTEXT_MATCH)).unwrap(),
            Some(Value::Boolean(expected))
        );
    }
    let mut absent = Session::new(p).unwrap();
    absent.start(&inputs()).unwrap();
    assert_eq!(
        absent.read(port(1, CONTEXT_MATCH)).unwrap(),
        Some(Value::Boolean(false))
    );
}

#[test]
fn tuning_query_starts_neutral_and_filters_manager_and_participant_notifications() {
    use crate::scoring::participant_properties::Property;
    let source = Property {
        channel: "items".into(),
        port: 7,
    };
    let p = Arc::new(Program {
        nodes: vec![
            node(0, Behavior::ParticipantProvider),
            node(
                1,
                Behavior::VehicleTuning {
                    source: source.clone(),
                    provider: 0,
                },
            ),
        ],
        cells: vec![cell(Kind::Float, &[port(1, VEHICLE_TUNING)])],
        connections: vec![],
        startup: vec![Step::Bind(1), Step::Activate(1)],
        unmodeled: vec![],
    });
    let mut s = Session::new(p.clone()).unwrap();
    s.notify_vehicle_tuning(&source, 77, Some(2.0), &inputs())
        .unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(
        s.read(port(1, VEHICLE_TUNING)).unwrap(),
        Some(Value::Float(5.0))
    );
    s.notify_vehicle_tuning(&source, 88, Some(1.0), &inputs())
        .unwrap();
    let other = Property {
        channel: "other items".into(),
        port: 7,
    };
    s.notify_vehicle_tuning(&other, 77, Some(1.0), &inputs())
        .unwrap();
    assert_eq!(
        s.read(port(1, VEHICLE_TUNING)).unwrap(),
        Some(Value::Float(5.0))
    );
    s.notify_vehicle_tuning(&source, 77, Some(9.25), &inputs())
        .unwrap();
    assert_eq!(
        s.read(port(1, VEHICLE_TUNING)).unwrap(),
        Some(Value::Float(9.25))
    );
    let before = s.state.clone();
    assert!(
        s.notify_vehicle_tuning(&source, 77, Some(f32::NAN), &inputs())
            .is_err()
    );
    assert_eq!(s.state, before);
    s.notify_vehicle_tuning(&source, 77, None, &inputs())
        .unwrap();
    assert_eq!(
        s.read(port(1, VEHICLE_TUNING)).unwrap(),
        Some(Value::Float(5.0))
    );
    let mut isolated = Session::new(p).unwrap();
    isolated.start(&inputs()).unwrap();
    s.notify_vehicle_tuning(&source, 77, Some(1.0), &inputs())
        .unwrap();
    assert_eq!(
        isolated.read(port(1, VEHICLE_TUNING)).unwrap(),
        Some(Value::Float(5.0))
    );
}

#[test]
fn transitions_and_channel_events_use_bound_participant_and_lifetime() {
    use crate::scoring::{
        participant_properties::Property,
        transitions::{self, ChannelEvent, Request},
    };
    let source = Property {
        channel: "mode".into(),
        port: 9,
    };
    let p = Arc::new(Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(1, Behavior::ParticipantProvider),
            node(
                2,
                Behavior::ParticipantTransition {
                    provider: 1,
                    sources: vec![source.clone(), source.clone()],
                    target: None,
                },
            ),
            node(3, Behavior::ParticipantEventChannel),
        ],
        cells: vec![],
        connections: vec![
            Connection {
                source: Endpoint { node: 0, event: 1 },
                target: Endpoint {
                    node: 2,
                    event: transitions::TRIGGER,
                },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint { node: 0, event: 1 },
                target: Endpoint {
                    node: 3,
                    event: transitions::CHANNEL_IN,
                },
                delivery: Delivery::Local,
            },
        ],
        startup: vec![],
        unmodeled: vec![],
    });
    let mut s = Session::new(p.clone()).unwrap();
    s.bind_event_channel(3, 400, 88).unwrap();
    assert!(s.bind_event_channel(3, 500, 99).is_err());
    s.start(&inputs()).unwrap();
    for _ in 0..2 {
        let out = s
            .transaction(&inputs(), |c, d| {
                c.emit_with(
                    Endpoint { node: 0, event: 1 },
                    Some(Context::Participant(123)),
                    d,
                )
            })
            .unwrap();
        assert_eq!(
            out.transitions,
            vec![Request {
                node: 2,
                selection: transitions::Selection::FirstCompleted(77),
                sources: vec![source.clone(), source.clone()],
                target: None
            }]
        );
        assert_eq!(
            out.channel_events,
            vec![ChannelEvent {
                channel: 400,
                event: transitions::CHANNEL_OUT,
                participant: 88
            }]
        );
    }
    let mut absent = Session::new(p).unwrap();
    absent.start(&inputs()).unwrap();
    assert_eq!(absent.event(0, 1, &inputs()), Err(Error::MissingInput));
    assert!(absent.tick(0.1, &inputs()).unwrap().transitions.is_empty());
}

#[test]
fn triggered_transitions_keep_single_participant_and_all_members_distinct() {
    use crate::scoring::{
        participant_properties::Property,
        transitions::{self, Selection},
    };
    let source = Property {
        channel: "state".into(),
        port: 1,
    };
    let p = Arc::new(Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(
                1,
                Behavior::TriggeredTransition {
                    sources: vec![source.clone(), source.clone()],
                    target: None,
                },
            ),
        ],
        cells: vec![],
        connections: [transitions::TRIGGER, transitions::TRIGGER_ALL]
            .into_iter()
            .map(|event| Connection {
                source: Endpoint { node: 0, event },
                target: Endpoint { node: 1, event },
                delivery: Delivery::Local,
            })
            .collect(),
        startup: vec![],
        unmodeled: vec![],
    });
    let mut s = Session::new(p).unwrap();
    s.start(&inputs()).unwrap();
    assert!(
        s.event(0, transitions::TRIGGER, &inputs())
            .unwrap()
            .transitions
            .is_empty()
    );
    let out = s
        .transaction(&inputs(), |c, d| {
            c.emit_with(
                Endpoint {
                    node: 0,
                    event: transitions::TRIGGER,
                },
                Some(Context::Participant(88)),
                d,
            )
        })
        .unwrap();
    assert_eq!(
        out.transitions,
        vec![transitions::Request {
            node: 1,
            selection: Selection::FirstCompleted(88),
            sources: vec![source.clone(), source.clone()],
            target: None
        }]
    );
    let out = s.event(0, transitions::TRIGGER_ALL, &inputs()).unwrap();
    assert_eq!(
        out.transitions,
        vec![transitions::Request {
            node: 1,
            selection: Selection::AllMembers,
            sources: vec![source.clone(), source],
            target: None
        }]
    );
}

#[test]
fn teleport_location_requests_use_own_participant_and_keep_provider_links() {
    use crate::scoring::{
        participant_properties::Property,
        teleport_location::{self, Request},
    };
    let location = Property {
        channel: "start".into(),
        port: 1,
    };
    let manager = Property {
        channel: "teleport".into(),
        port: 2,
    };
    let p = Arc::new(Program {
        nodes: vec![
            node(0, Behavior::ParticipantProvider),
            node(
                1,
                Behavior::TeleportLocation {
                    provider: 0,
                    location: location.clone(),
                    manager: manager.clone(),
                },
            ),
        ],
        cells: vec![],
        connections: vec![Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint {
                node: 1,
                event: teleport_location::SET,
            },
            delivery: Delivery::Local,
        }],
        startup: vec![],
        unmodeled: vec![],
    });
    let mut s = Session::new(p).unwrap();
    assert!(s.start(&inputs()).unwrap().teleport_locations.is_empty());
    for _ in 0..2 {
        let out = s
            .transaction(&inputs(), |c, d| {
                c.emit_with(
                    Endpoint { node: 0, event: 1 },
                    Some(Context::Participant(88)),
                    d,
                )
            })
            .unwrap();
        assert_eq!(
            out.teleport_locations,
            vec![Request {
                node: 1,
                participant: 77,
                location: location.clone(),
                manager: manager.clone()
            }]
        );
    }
    assert!(
        s.event(0, 2, &inputs())
            .unwrap()
            .teleport_locations
            .is_empty()
    );
}

#[test]
fn linked_curve_uses_its_own_input_and_waits_for_a_callback() {
    use crate::scoring::{
        curve::{Curve, Interpolation, Knot},
        participant_properties::Property,
    };
    let source = Property {
        channel: "curve".into(),
        port: 8,
    };
    let curve = Curve::new(
        [0.0, 1.0],
        [0.0, 10.0],
        1.0,
        1.0,
        Interpolation::Linear,
        vec![
            Knot {
                x: 0.0,
                y: 0.0,
                coefficient: 0.0,
            },
            Knot {
                x: 1.0,
                y: 1.0,
                coefficient: 0.0,
            },
        ],
    )
    .unwrap();
    let mut wrapper = node(
        0,
        Behavior::LinkedCurve {
            source: source.clone(),
        },
    );
    wrapper.defaults.push((INPUT, Value::Float(0.5)));
    let p = Arc::new(Program {
        nodes: vec![wrapper],
        cells: vec![cell(Kind::Float, &[port(0, OUTPUT)])],
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    for (apply, expected) in [(true, 5.0), (false, 0.0)] {
        let mut s = Session::new(p.clone()).unwrap();
        s.bind_curve(source.clone(), curve.clone(), apply).unwrap();
        assert!(s.bind_curve(source.clone(), curve.clone(), apply).is_err());
        s.start(&inputs()).unwrap();
        assert_eq!(s.read(port(0, OUTPUT)).unwrap(), None);
        s.transaction(&inputs(), |c, d| c.curve(0, d)).unwrap();
        assert_eq!(
            s.read(port(0, OUTPUT)).unwrap(),
            Some(Value::Float(expected))
        );
    }
    let mut absent = Session::new(p).unwrap();
    absent.start(&inputs()).unwrap();
    assert_eq!(
        absent.transaction(&inputs(), |c, d| c.curve(0, d)),
        Err(Error::MissingInput)
    );
    assert_eq!(absent.read(port(0, OUTPUT)).unwrap(), None);
}

#[test]
fn event_details_binding_preserves_unsigned_id_and_absence() {
    use crate::scoring::participant_properties::Property;
    let source = Property {
        channel: "details".into(),
        port: 5,
    };
    let p = Arc::new(Program {
        nodes: vec![node(
            0,
            Behavior::EventDetails {
                source: Some(source.clone()),
            },
        )],
        cells: vec![
            cell(Kind::Unsigned32, &[port(0, EVENT_ID)]),
            cell(Kind::Text, &[port(0, EVENT_ID_STRING)]),
        ],
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    let mut absent = Session::new(p.clone()).unwrap();
    absent.start(&inputs()).unwrap();
    assert_eq!(absent.read(port(0, EVENT_ID)).unwrap(), None);
    assert_eq!(absent.read(port(0, EVENT_ID_STRING)).unwrap(), None);
    for value in [0, u32::MAX] {
        let mut s = Session::new(p.clone()).unwrap();
        s.bind_event_id(source.clone(), value).unwrap();
        let name = crate::scoring::value::Text::new("event/sprint").unwrap();
        s.bind_event_string(source.clone(), name.clone()).unwrap();
        assert!(s.bind_event_string(source.clone(), name.clone()).is_err());
        assert!(s.bind_event_id(source.clone(), value).is_err());
        s.start(&inputs()).unwrap();
        assert_eq!(
            s.read(port(0, EVENT_ID_STRING)).unwrap(),
            Some(Value::Text(name.clone()))
        );
        assert!(s.bind_event_string(source.clone(), name).is_err());
        assert_eq!(
            s.read(port(0, EVENT_ID)).unwrap(),
            Some(Value::Unsigned32(value))
        );
        assert!(s.bind_event_id(source.clone(), value).is_err());
    }
}

#[test]
fn participant_name_rpc_requires_participant_event_and_retains_repeats() {
    let p = Program {
        nodes: vec![node(0, Behavior::Inert), node(1, Behavior::ParticipantName)],
        cells: vec![],
        connections: vec![Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint {
                node: 1,
                event: nodes::IN,
            },
            delivery: Delivery::Local,
        }],
        startup: vec![],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    assert!(
        s.event(0, 1, &inputs())
            .unwrap()
            .participant_names
            .is_empty()
    );
    for _ in 0..2 {
        let out = s
            .transaction(&inputs(), |c, d| {
                c.emit_with(
                    Endpoint { node: 0, event: 1 },
                    Some(Context::Participant(88)),
                    d,
                )
            })
            .unwrap();
        assert_eq!(out.participant_names, vec![(1, 88)]);
    }
    let out = s
        .transaction(&inputs(), |c, d| {
            c.emit_with(
                Endpoint { node: 0, event: 1 },
                Some(Context::Entity(EntityContext { object: Some(88) })),
                d,
            )
        })
        .unwrap();
    assert!(out.participant_names.is_empty());
}

#[test]
fn scoped_set_adapters_predicates_and_counters_follow_owned_membership() {
    use crate::scoring::participant_sets::{self as sets, Key, Operation, Source};
    let scoped = Key {
        channel: "event".into(),
        port: 4,
    };
    let global = Key {
        channel: "race".into(),
        port: 5,
    };
    let p = Program {
        nodes: vec![
            node(0, Behavior::ParticipantProvider),
            node(
                1,
                Behavior::ParticipantSetAdapter {
                    source: Some(scoped.clone()),
                    provider: Some(0),
                },
            ),
            node(
                2,
                Behavior::ParticipantSetPredicate {
                    operation: Operation::Intersection,
                    operands: vec![Source::Node(1), Source::Channel(global.clone())],
                },
            ),
            node(
                3,
                Behavior::ParticipantSetCounter {
                    source: Some(Source::Node(2)),
                },
            ),
            node(4, Behavior::ParticipantSetCounter { source: None }),
        ],
        cells: vec![
            cell(Kind::Integer, &[port(3, sets::COUNT)]),
            cell(Kind::Integer, &[port(4, sets::COUNT)]),
        ],
        connections: vec![],
        // A consumer may activate before its source; subsequent source activation
        // fills it through the same registered observer path.
        startup: vec![
            Step::Bind(3),
            Step::Bind(2),
            Step::Bind(1),
            Step::Bind(0),
            Step::Bind(4),
            Step::Activate(3),
            Step::Activate(2),
            Step::Activate(1),
            Step::Activate(0),
            Step::Activate(4),
        ],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(p.clone())).unwrap();
    s.import_scoped_set(&scoped, 77, &[77, 88], &inputs())
        .unwrap();
    s.import_scoped_set(&scoped, 99, &[99], &inputs()).unwrap();
    s.import_participant_sets(
        &[sets::Change {
            set: global.clone(),
            participant: 88,
            member: true,
        }],
        &inputs(),
    )
    .unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(
        s.read(port(3, sets::COUNT)).unwrap(),
        Some(Value::Integer(1))
    );
    assert_eq!(
        s.read(port(4, sets::COUNT)).unwrap(),
        Some(Value::Integer(0))
    );
    s.import_scoped_set(&scoped, 99, &[], &inputs()).unwrap();
    assert_eq!(
        s.read(port(3, sets::COUNT)).unwrap(),
        Some(Value::Integer(1))
    );
    s.import_scoped_set(&scoped, 77, &[77], &inputs()).unwrap();
    assert_eq!(
        s.read(port(3, sets::COUNT)).unwrap(),
        Some(Value::Integer(0))
    );
    s.import_participant_sets(
        &[sets::Change {
            set: global,
            participant: 77,
            member: true,
        }],
        &inputs(),
    )
    .unwrap();
    assert_eq!(
        s.read(port(3, sets::COUNT)).unwrap(),
        Some(Value::Integer(1))
    );
    let before = s.state.clone();
    assert_eq!(
        s.import_scoped_set(&scoped, 77, &[77, 77], &inputs()),
        Err(Error::InvalidGraph)
    );
    assert_eq!(s.state, before);
    let mut cyclic = p;
    cyclic.nodes[2].behavior = Behavior::ParticipantSetPredicate {
        operation: Operation::Union,
        operands: vec![Source::Node(2)],
    };
    assert_eq!(cyclic.validate(), Err(Error::InvalidGraph));
}

#[test]
fn rank_query_tracks_selected_identity_and_score_without_bind_or_repeat_updates() {
    use crate::scoring::{
        participant_properties::Property,
        rank_query::{INDEX, Row, Update},
    };
    let sorter = Property {
        channel: "event/scores".into(),
        port: 8,
    };
    let query = node(
        0,
        Behavior::ParticipantRankQuery {
            sorter: sorter.clone(),
        },
    );
    let p = Arc::new(Program {
        nodes: vec![query],
        cells: vec![cell(Kind::Integer, &[port(0, INDEX)])],
        connections: vec![],
        startup: vec![
            Step::Bind(0),
            Step::Register(Registration::Notify(port(0, INDEX))),
            Step::Default {
                port: port(0, INDEX),
                value: Value::Integer(0),
                mode: DefaultMode::Override,
            },
            Step::Activate(0),
        ],
        unmodeled: vec![],
    });
    let mut s = Session::new(p.clone()).unwrap();
    let a = Row {
        participant: 77,
        value: -10,
    };
    let b = Row {
        participant: 88,
        value: -10,
    };
    assert_eq!(
        s.import_rank_table(&sorter, &[a, b], &inputs()).unwrap(),
        Output::default()
    );
    assert_eq!(s.start(&inputs()).unwrap(), Output::default());
    assert_eq!(
        s.import_rank_table(&sorter, &[a, b], &inputs())
            .unwrap()
            .rank_queries,
        vec![Update {
            node: 0,
            row: Some(a)
        }]
    );
    assert_eq!(
        s.import_rank_table(&sorter, &[a, b], &inputs()).unwrap(),
        Output::default()
    );
    // Identity changes are observable even at an equal score.
    assert_eq!(
        s.import_rank_table(&sorter, &[b, a], &inputs())
            .unwrap()
            .rank_queries,
        vec![Update {
            node: 0,
            row: Some(b)
        }]
    );
    let b = Row {
        value: i32::MIN,
        ..b
    };
    assert_eq!(
        s.import_rank_table(&sorter, &[b, a], &inputs())
            .unwrap()
            .rank_queries[0]
            .row,
        Some(b)
    );
    let change_index = |s: &mut Session, value| {
        s.transaction(&inputs(), |c, d| {
            d.publish(c, port(0, INDEX), Value::Integer(value))
        })
    };
    assert_eq!(
        change_index(&mut s, 1).unwrap().rank_queries[0].row,
        Some(a)
    );
    assert_eq!(
        change_index(&mut s, -1).unwrap().rank_queries,
        vec![Update { node: 0, row: None }]
    );
    assert_eq!(change_index(&mut s, i32::MAX).unwrap(), Output::default());
    assert_eq!(
        change_index(&mut s, 0).unwrap().rank_queries[0].row,
        Some(b)
    );
    assert_eq!(
        s.import_rank_table(&sorter, &[], &inputs())
            .unwrap()
            .rank_queries[0]
            .row,
        None
    );
    let before = s.state.clone();
    assert_eq!(
        s.import_rank_table(&sorter, &[a, a], &inputs()),
        Err(Error::InvalidGraph)
    );
    assert_eq!(s.state, before);
    let many = (0..=super::super::rank_query::MAX_ROWS)
        .map(|id| Row {
            participant: id as u64,
            value: 0,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        s.import_rank_table(&sorter, &many, &inputs()),
        Err(Error::Bound)
    );
    assert_eq!(s.state, before);
    let mut other = Session::new(p).unwrap();
    other.start(&inputs()).unwrap();
    assert!(other.state.rank_tables.is_empty());
}

#[test]
fn game_id_provider_requires_owned_context_and_preserves_all_bits() {
    let p = Arc::new(Program {
        nodes: vec![node(0, Behavior::GameId)],
        cells: vec![cell(Kind::Unsigned64, &[port(0, GAME_ID)])],
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    let mut absent = Session::new(p.clone()).unwrap();
    absent.start(&inputs()).unwrap();
    assert_eq!(absent.read(port(0, GAME_ID)).unwrap(), None);
    assert!(absent.bind_game_id(1).is_err());
    for id in [0, (1 << 63) + 1, u64::MAX] {
        let mut present = Session::new(p.clone()).unwrap();
        present.bind_game_id(id).unwrap();
        assert!(present.bind_game_id(id).is_err());
        present.start(&inputs()).unwrap();
        assert_eq!(
            present.read(port(0, GAME_ID)).unwrap(),
            Some(Value::Unsigned64(id))
        );
    }
}

#[test]
fn bank_requests_wait_for_owned_completion_preserve_identity_and_bound_pending_work() {
    use crate::scoring::{bank, events::Remote, participant_properties::Property};
    let mut transaction = node(
        2,
        Behavior::Bank {
            bank: Property {
                channel: "economy".into(),
                port: 7,
            },
            provider: 1,
            kind: bank::Kind::Deposit,
        },
    );
    transaction.defaults = vec![(bank::AMOUNT, Value::Integer(1234))];
    let p = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(1, Behavior::ParticipantProvider),
            transaction,
        ],
        cells: vec![],
        connections: vec![
            Connection {
                source: Endpoint { node: 0, event: 1 },
                target: Endpoint {
                    node: 2,
                    event: bank::TRANSACT,
                },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint {
                    node: 2,
                    event: bank::SUCCESS,
                },
                target: Endpoint { node: 0, event: 2 },
                delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 2 }),
            },
            Connection {
                source: Endpoint {
                    node: 2,
                    event: bank::FAILURE,
                },
                target: Endpoint { node: 0, event: 3 },
                delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 3 }),
            },
        ],
        startup: vec![Step::Bind(1)],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    let mut changed_owner = inputs();
    changed_owner.participant = 99;
    for expected in 1..=2 {
        let output = s.event(0, 1, &changed_owner).unwrap();
        assert!(output.remote.is_empty());
        assert_eq!(output.bank_requests.len(), 1);
        let r = &output.bank_requests[0];
        assert_eq!((r.id, r.participant, r.amount), (expected, 77, 1234));
    }
    assert_eq!(
        s.complete_bank(3, true, &inputs()),
        Err(Error::InvalidGraph)
    );
    let failed = s.complete_bank(2, false, &inputs()).unwrap();
    assert_eq!(failed.remote.len(), 1);
    assert_eq!(failed.remote[0].context, None);
    assert_eq!(
        s.complete_bank(2, true, &inputs()).unwrap(),
        Output::default()
    );
    assert_eq!(s.complete_bank(1, true, &inputs()).unwrap().remote.len(), 1);
    assert_eq!(
        s.complete_bank(1, true, &inputs()).unwrap(),
        Output::default()
    );
    for _ in 0..bank::MAX_PENDING {
        s.event(0, 1, &inputs()).unwrap();
    }
    let before = s.state.clone();
    assert_eq!(s.event(0, 1, &inputs()), Err(Error::Bound));
    assert_eq!(s.state, before);
    s.complete_bank(3, true, &inputs()).unwrap();
    assert_eq!(
        s.event(0, 1, &inputs()).unwrap().bank_requests[0].id,
        3 + bank::MAX_PENDING as u64
    );
}

#[test]
fn float_hub_select_events_preserve_selector_and_bound_readiness() {
    use crate::scoring::{integer_hub::SELECT, integers};
    let p = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(1, Behavior::FloatHub { count: 3 }),
        ],
        cells: vec![
            Definition {
                initial: Some(Value::Integer(0)),
                ..cell(Kind::Integer, &[port(1, SELECT)])
            },
            Definition {
                initial: Some(Value::Float(4.25)),
                ..cell(Kind::Float, &[port(1, integers::port("In", 0))])
            },
            cell(Kind::Float, &[port(1, integers::port("In", 1))]),
            cell(Kind::Float, &[port(1, OUT)]),
        ],
        connections: (0..3)
            .map(|i| Connection {
                source: Endpoint {
                    node: 0,
                    event: u32::from(i),
                },
                target: Endpoint {
                    node: 1,
                    event: integers::port("SelectIn", i),
                },
                delivery: Delivery::Local,
            })
            .collect(),
        startup: vec![Step::Bind(1)],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(s.read(port(1, OUT)).unwrap(), Some(Value::Float(4.25)));
    s.event(0, 1, &inputs()).unwrap(); // bound, unready: no write
    assert_eq!(s.read(port(1, OUT)).unwrap(), Some(Value::Float(4.25)));
    s.event(0, 2, &inputs()).unwrap(); // unbound: zero fallback
    assert_eq!(s.read(port(1, OUT)).unwrap(), Some(Value::Float(0.0)));
    assert_eq!(s.read(port(1, SELECT)).unwrap(), Some(Value::Integer(0)));
    s.event(0, 0, &inputs()).unwrap();
    assert_eq!(s.read(port(1, OUT)).unwrap(), Some(Value::Float(4.25)));
}

#[test]
fn uint64_explicit_comparison_forwards_the_received_context() {
    use crate::scoring::{
        event_sync::Routed,
        events::Remote,
        nodes::{A, B, Comparison, GREATER},
    };
    let mut comparison = node(
        1,
        Behavior::Scalar(Scalar::Compare {
            kind: Comparison::Unsigned64,
            on_change: false,
            on_start: false,
        }),
    );
    comparison.defaults = vec![(A, Value::Unsigned64(u64::MAX)), (B, Value::Unsigned64(1))];
    let p = Program {
        nodes: vec![
            node(0, Behavior::EventSync),
            comparison,
            node(2, Behavior::Inert),
        ],
        cells: vec![],
        connections: vec![
            Connection {
                source: Endpoint {
                    node: 0,
                    event: OUT,
                },
                target: Endpoint { node: 1, event: 1 },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint {
                    node: 1,
                    event: GREATER,
                },
                target: Endpoint { node: 2, event: 9 },
                delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 4 }),
            },
        ],
        startup: vec![Step::Activate(1)],
        unmodeled: vec![],
    };
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    let context = EntityContext { object: Some(123) };
    for _ in 0..2 {
        let out = s
            .event_sync(
                Routed {
                    node: 0,
                    context: Some(context),
                },
                &inputs(),
            )
            .unwrap();
        assert_eq!(out.remote.len(), 1);
        assert_eq!(out.remote[0].context, Some(Context::Entity(context)));
    }
}

#[test]
fn participant_set_accessors_share_membership_without_crossing_participant_identity() {
    use crate::scoring::participant_sets::{self, CONTAINS, Change, Key};
    let key = Key {
        channel: "event".into(),
        port: 7,
    };
    let p = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(1, Behavior::ParticipantProvider),
            node(
                2,
                Behavior::ParticipantSetAccessor {
                    set: Some(key.clone()),
                    provider: Some(1),
                },
            ),
            node(
                3,
                Behavior::ParticipantSetAccessor {
                    set: Some(key.clone()),
                    provider: Some(1),
                },
            ),
            node(
                4,
                Behavior::ParticipantSetAccessor {
                    set: None,
                    provider: Some(1),
                },
            ),
        ],
        cells: (2..5)
            .map(|id| cell(Kind::Boolean, &[port(id, CONTAINS)]))
            .collect(),
        connections: [participant_sets::ADD, participant_sets::REMOVE]
            .into_iter()
            .map(|event| Connection {
                source: Endpoint { node: 0, event },
                target: Endpoint { node: 2, event },
                delivery: Delivery::Local,
            })
            .collect(),
        // Providers exist at construction, even when their bind occurs later.
        startup: vec![
            Step::Bind(2),
            Step::Bind(3),
            Step::Bind(4),
            Step::Bind(1),
            Step::Activate(2),
            Step::Activate(3),
            Step::Activate(4),
            Step::Activate(1),
        ],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(p)).unwrap();
    let change = |participant, member| Change {
        set: key.clone(),
        participant,
        member,
    };
    session
        .import_participant_sets(&[change(77, true)], &inputs())
        .unwrap();
    assert!(
        session
            .start(&inputs())
            .unwrap()
            .participant_sets
            .is_empty()
    );
    for id in [2, 3] {
        assert_eq!(
            session.read(port(id, CONTAINS)).unwrap(),
            Some(Value::Boolean(true))
        );
    }
    assert_eq!(
        session.read(port(4, CONTAINS)).unwrap(),
        Some(Value::Boolean(false))
    );
    assert!(
        session
            .event(0, participant_sets::ADD, &inputs())
            .unwrap()
            .participant_sets
            .is_empty()
    );
    let out = session
        .event(0, participant_sets::REMOVE, &inputs())
        .unwrap();
    assert_eq!(out.participant_sets, vec![change(77, false)]);
    session
        .import_participant_sets(&[change(88, true)], &inputs())
        .unwrap();
    for id in [2, 3] {
        assert_eq!(
            session.read(port(id, CONTAINS)).unwrap(),
            Some(Value::Boolean(false))
        );
    }
    let mut foreign = inputs();
    foreign.participant = 99;
    assert_eq!(
        session
            .event(0, participant_sets::ADD, &foreign)
            .unwrap()
            .participant_sets,
        vec![change(77, true)]
    );
    let changes = (100..228).map(|p| change(p, true)).collect::<Vec<_>>();
    assert!(
        session
            .import_participant_sets(&changes, &inputs())
            .is_err()
    );
    assert_eq!(session.state.participant_sets[&key], [77, 88].into());
}

#[test]
fn participant_events_use_bound_provider_and_filter_out_player_context() {
    use crate::scoring::{
        event_sync::Routed,
        events::{EntityContext, Remote},
    };
    let p = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(1, Behavior::ParticipantProvider),
            node(
                2,
                Behavior::ParticipantEvent {
                    provider: Some(1),
                    filter: false,
                },
            ),
            node(
                3,
                Behavior::ParticipantEvent {
                    provider: Some(1),
                    filter: true,
                },
            ),
            node(4, Behavior::Inert),
            node(5, Behavior::EventSync),
        ],
        cells: vec![],
        connections: vec![
            Connection {
                source: Endpoint { node: 0, event: 1 },
                target: Endpoint {
                    node: 2,
                    event: crate::scoring::nodes::IN,
                },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint {
                    node: 2,
                    event: OUT,
                },
                target: Endpoint { node: 3, event: 55 },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint {
                    node: 5,
                    event: OUT,
                },
                target: Endpoint { node: 3, event: 55 },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint {
                    node: 3,
                    event: OUT,
                },
                target: Endpoint { node: 4, event: 9 },
                delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 3 }),
            },
        ],
        startup: vec![Step::Bind(1)],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(p)).unwrap();
    session.start(&inputs()).unwrap();
    let mut later = inputs();
    later.participant = 99;
    let output = session.event(0, 1, &later).unwrap();
    assert_eq!(output.remote.len(), 1);
    assert_eq!(output.remote[0].context, Some(Context::Participant(77)));
    let output = session
        .event_sync(
            Routed {
                node: 5,
                context: Some(EntityContext { object: Some(77) }),
            },
            &inputs(),
        )
        .unwrap();
    assert!(output.remote.is_empty());
    // An ordinary event on the filter does not become a ParticipantEvent.
    assert!(session.event(5, OUT, &inputs()).unwrap().remote.is_empty());
}

#[test]
fn participant_rank_uses_only_stored_values_and_keeps_derived_writes_owned() {
    use crate::scoring::{
        integers::{COUNT, RANK},
        participant_properties::Property,
    };
    let score = Property {
        channel: "race".into(),
        port: 1,
    };
    let rank = Property {
        channel: "race".into(),
        port: 2,
    };
    let program = Program {
        nodes: vec![
            node(
                0,
                Behavior::ParticipantRank {
                    property: score.clone(),
                    descending: true,
                },
            ),
            node(
                1,
                Behavior::ParticipantProperty {
                    property: rank.clone(),
                    kind: Kind::Integer,
                    write: true,
                },
            ),
        ],
        cells: vec![
            cell(Kind::Integer, &[port(0, RANK), port(1, FLOAT_VALUE)]),
            cell(Kind::Integer, &[port(0, COUNT)]),
        ],
        connections: vec![],
        startup: vec![
            Step::Bind(0),
            Step::Register(Registration::Notify(port(1, FLOAT_VALUE))),
            Step::Bind(1),
            Step::Activate(0),
            Step::Activate(1),
        ],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program.clone())).unwrap();
    // An unready rank neither writes a fabricated zero nor prevents activation.
    session.start(&inputs()).unwrap();
    assert_eq!(session.participant_property(77, &rank), None);
    assert_eq!(session.read(port(0, RANK)).unwrap(), None);
    let mut session = Session::new(Arc::new(program.clone())).unwrap();
    session
        .set_participant_properties(&[(score.clone(), 100)], &inputs())
        .unwrap();
    session.start(&inputs()).unwrap();
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(1))
    );
    assert_eq!(session.participant_property(77, &rank), Some(1));
    // A rival updates the shared property; the local rank writer remains 77.
    let output = session
        .import_participant_values(99, &[(score.clone(), Value::Integer(200))], &inputs())
        .unwrap();
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(2))
    );
    assert_eq!(session.participant_property(77, &rank), Some(2));
    assert_eq!(session.participant_property(99, &rank), None);
    assert_eq!(
        output.participant_properties,
        vec![participant_properties::Write {
            participant: 77,
            property: rank.clone(),
            value: Value::Integer(2)
        }]
    );
    // Equal scores preserve key traversal, not most-recent update order.
    session
        .import_participant_values(99, &[(score.clone(), Value::Integer(100))], &inputs())
        .unwrap();
    assert_eq!(session.participant_property(77, &rank), Some(1));
    // Different scalar maps do not enter integer rankings.
    session
        .import_participant_values(88, &[(score.clone(), Value::Float(900.0))], &inputs())
        .unwrap();
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(2))
    );
    for id in 100..126 {
        session
            .import_participant_values(id, &[(score.clone(), Value::Integer(0))], &inputs())
            .unwrap();
    }
    assert!(
        session
            .import_participant_values(126, &[(score.clone(), Value::Integer(0))], &inputs())
            .is_err()
    );
    assert_eq!(session.participant_property(126, &score), None);
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(28))
    );
}

#[test]
fn event_sync_preserves_player_context_and_cannot_drive_other_node_classes() {
    use crate::scoring::{
        event_sync::Routed,
        events::{EntityContext, Remote},
    };
    let program = Program {
        nodes: vec![node(0, Behavior::EventSync), node(1, Behavior::Inert)],
        connections: vec![Connection {
            source: Endpoint {
                node: 0,
                event: OUT,
            },
            target: Endpoint { node: 1, event: 9 },
            delivery: Delivery::NetworkedClient(Remote { bus: 0, index: 3 }),
        }],
        cells: vec![],
        startup: vec![],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    let route = Routed {
        node: 0,
        context: Some(EntityContext { object: Some(5) }),
    };
    assert!(session.event_sync(route, &inputs()).is_err());
    session.start(&inputs()).unwrap();
    let out = session.event_sync(route, &inputs()).unwrap();
    assert_eq!(out.remote.len(), 1);
    assert_eq!(out.remote[0].context, route.context.map(Context::Entity));
    let out = session
        .event_sync(
            Routed {
                context: None,
                ..route
            },
            &inputs(),
        )
        .unwrap();
    assert_eq!(out.remote[0].context, None);
    assert!(
        session
            .event_sync(Routed { node: 1, ..route }, &inputs())
            .is_err()
    );
}

#[test]
fn float_switch_publishes_before_selected_and_repeats_selection_events() {
    use crate::scoring::float_switch::{self, Entry};
    let program = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(
                1,
                Behavior::FloatSwitch(float_switch::Definition {
                    entries: vec![Entry {
                        property: 10,
                        event: 20,
                        fallback: 3.0,
                    }],
                    on_change: true,
                }),
            ),
            node(2, Behavior::Scalar(Scalar::Accumulator)),
        ],
        cells: vec![
            cell(
                Kind::Float,
                &[port(1, float_switch::OUTPUT), port(2, INPUT)],
            ),
            cell(Kind::Float, &[port(2, OUTPUT)]),
        ],
        connections: vec![
            Connection {
                source: Endpoint { node: 0, event: 1 },
                target: Endpoint { node: 1, event: 20 },
                delivery: Delivery::Local,
            },
            Connection {
                source: Endpoint {
                    node: 1,
                    event: float_switch::SELECTED,
                },
                target: Endpoint {
                    node: 2,
                    event: ACCUMULATE,
                },
                delivery: Delivery::Local,
            },
        ],
        startup: vec![Step::Bind(1), Step::Bind(2)],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    session.start(&inputs()).unwrap();
    assert_eq!(
        session.read(port(1, float_switch::OUTPUT)).unwrap(),
        Some(Value::Float(0.0))
    );
    for expected in [3.0, 6.0] {
        session.event(0, 1, &inputs()).unwrap();
        assert_eq!(
            session.read(port(2, OUTPUT)).unwrap(),
            Some(Value::Float(expected))
        );
    }
}

#[test]
fn participant_scalar_maps_are_typed_seedable_and_atomic() {
    use crate::scoring::participant_properties::Property;
    let key = Property {
        channel: "race".into(),
        port: 7,
    };
    let kinds = [Kind::Boolean, Kind::Float, Kind::Integer];
    let p = Program {
        nodes: kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| {
                node(
                    i as u32,
                    Behavior::ParticipantProperty {
                        property: key.clone(),
                        kind,
                        write: false,
                    },
                )
            })
            .collect(),
        cells: kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| cell(kind, &[port(i as u16, nodes::OUTPUT_VALUE)]))
            .collect(),
        startup: vec![
            Step::Bind(0),
            Step::Bind(1),
            Step::Bind(2),
            Step::Activate(0),
            Step::Activate(1),
            Step::Activate(2),
        ],
        connections: vec![],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(p)).unwrap();
    let values = [Value::Boolean(true), Value::Float(1.25), Value::Integer(17)];
    let writes = values.clone().map(|v| (key.clone(), v));
    session.set_participant_values(&writes, &inputs()).unwrap();
    assert!(
        session
            .start(&inputs())
            .unwrap()
            .participant_properties
            .is_empty()
    );
    for (i, value) in values.into_iter().enumerate() {
        assert_eq!(
            session.read(port(i as u16, nodes::OUTPUT_VALUE)).unwrap(),
            Some(value)
        );
    }
    for invalid in [Value::Float(f32::INFINITY), Value::Vec3([0.0; 3])] {
        assert!(
            session
                .set_participant_values(
                    &[(key.clone(), Value::Boolean(false)), (key.clone(), invalid)],
                    &inputs()
                )
                .is_err()
        );
        assert_eq!(
            session.participant_value(77, &key, Kind::Boolean),
            Some(Value::Boolean(true))
        );
    }
    let mut stranger = inputs();
    stranger.participant = 78;
    session
        .set_participant_values(&[(key.clone(), Value::Float(2.5))], &stranger)
        .unwrap();
    assert_eq!(
        session.participant_value(77, &key, Kind::Float),
        Some(Value::Float(1.25))
    );
    assert_eq!(
        session.participant_value(78, &key, Kind::Float),
        Some(Value::Float(2.5))
    );
    assert_eq!(
        session.read(port(1, nodes::OUTPUT_VALUE)).unwrap(),
        Some(Value::Float(1.25))
    );
}

#[test]
fn race_clock_uses_host_origin_rounding_and_ignores_graph_age_and_scale() {
    let p = Program {
        nodes: vec![node(0, Behavior::RaceTime)],
        cells: vec![cell(Kind::Float, &[port(0, stopwatch::TIME)])],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        connections: vec![],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(p)).unwrap();
    let mut input = inputs();
    input.now = 123.25;
    input.time_scale = 0.0;
    session.start(&input).unwrap();
    assert_eq!(
        session.read(port(0, stopwatch::TIME)).unwrap(),
        Some(Value::Float(123.25))
    );
    input.now = 126.75;
    session.tick(0.01, &input).unwrap();
    assert_eq!(
        session.read(port(0, stopwatch::TIME)).unwrap(),
        Some(Value::Float(126.75))
    );
    input.now = f64::MAX;
    assert!(session.tick(0.01, &input).is_err());
    assert_eq!(
        session.read(port(0, stopwatch::TIME)).unwrap(),
        Some(Value::Float(126.75))
    );
}

#[test]
fn transform_splitter_waits_for_a_ready_input_and_updates_the_four_vectors() {
    let fields = [0x7c83_f1be, 0x0059_7380, 0x7667_213e, 0x0d8a_a67f];
    let query = Query::Physics;
    let transform_field = super::super::vehicle_query::TRANSFORM;
    let mut cells = vec![cell(
        Kind::Transform,
        &[port(0, transform_field), port(1, 0x8752_5469)],
    )];
    cells.extend(fields.map(|field| cell(Kind::Vec3, &[port(1, field)])));
    let p = Program {
        nodes: vec![
            node(
                0,
                Behavior::VehicleQuery(
                    vehicle_query::Bindings::new(query, &[(transform_field, true)]).unwrap(),
                ),
            ),
            node(1, Behavior::TransformSplitter),
        ],
        cells,
        connections: vec![],
        unmodeled: vec![],
        startup: vec![
            Step::Bind(0),
            Step::Bind(1),
            Step::Register(Registration::Notify(port(1, 0x8752_5469))),
            Step::Activate(0),
        ],
    };
    let mut unready = p.clone();
    unready.nodes[0].behavior = Behavior::Inert;
    let mut unready = Session::new(Arc::new(unready)).unwrap();
    unready.start(&inputs()).unwrap();
    assert_eq!(unready.read(port(1, fields[0])).unwrap(), None);
    let mut session = Session::new(Arc::new(p)).unwrap();
    session.start(&inputs()).unwrap();
    assert_eq!(
        session.read(port(1, fields[0])).unwrap(),
        Some(Value::Vec3([1.0, 0.0, 0.0]))
    );
    let mut input = inputs();
    let vehicle = Vehicle {
        identity: 1,
        velocity: [0.0; 3],
        angular_velocity: [0.0; 3],
        airborne: None,
        state_3c230e35: None,
        state_cd5ae2f0: None,
        max_rpm: None,
        rpm: None,
        nos: None,
        burnout: None,
        transform: [
            [2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0],
            [8.0, 9.0, 10.0],
            [11.0, 12.0, 13.0],
        ],
    };
    input.vehicle = Some(vehicle);
    session.tick(0.1, &input).unwrap();
    for (i, field) in fields.into_iter().enumerate() {
        assert_eq!(
            session.read(port(1, field)).unwrap(),
            Some(Value::Vec3(vehicle.transform[i]))
        );
    }
}

#[test]
fn participant_store_connects_readers_writers_and_suppresses_equal_imports() {
    use crate::scoring::participant_properties::Property;
    let property = Property {
        channel: "test/race".into(),
        port: 77,
    };
    let mut producer = node(
        1,
        Behavior::Scalar(Scalar::Integer {
            default: 10,
            step: 5,
        }),
    );
    producer.defaults = vec![(FLOAT_VALUE, Value::Integer(10))];
    let program = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            producer,
            node(
                2,
                Behavior::ParticipantProperty {
                    kind: Kind::Integer,
                    property: property.clone(),
                    write: true,
                },
            ),
            node(
                3,
                Behavior::ParticipantProperty {
                    kind: Kind::Integer,
                    property: property.clone(),
                    write: false,
                },
            ),
        ],
        cells: vec![
            cell(Kind::Integer, &[port(1, FLOAT_VALUE), port(2, FLOAT_VALUE)]),
            cell(Kind::Integer, &[port(3, nodes::OUTPUT_VALUE)]),
        ],
        connections: vec![Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint {
                node: 1,
                event: nodes::INCREMENT,
            },
            delivery: Delivery::Local,
        }],
        startup: vec![
            Step::Bind(3),
            Step::Bind(1),
            Step::Bind(2),
            Step::Register(Registration::Notify(port(2, FLOAT_VALUE))),
            Step::Activate(3),
            Step::Activate(1),
            Step::Activate(2),
        ],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    let output = session.start(&inputs()).unwrap();
    assert_eq!(output.participant_properties.len(), 1);
    assert_eq!(output.participant_properties[0].value, Value::Integer(10));
    assert_eq!(
        session.read(port(3, nodes::OUTPUT_VALUE)).unwrap(),
        Some(Value::Integer(10))
    );
    let independent = session.clone();
    let output = session.event(0, 1, &inputs()).unwrap();
    assert_eq!(output.participant_properties[0].value, Value::Integer(15));
    assert_eq!(
        session.read(port(3, nodes::OUTPUT_VALUE)).unwrap(),
        Some(Value::Integer(15))
    );
    let values = [(property.clone(), 23)];
    assert!(
        session
            .set_participant_properties(&values, &inputs())
            .unwrap()
            .participant_properties
            .is_empty()
    );
    assert_eq!(
        session.read(port(3, nodes::OUTPUT_VALUE)).unwrap(),
        Some(Value::Integer(23))
    );
    assert!(
        session
            .set_participant_properties(&values, &inputs())
            .unwrap()
            .participant_properties
            .is_empty()
    );
    assert_eq!(session.participant_property(77, &property), Some(23));
    assert_eq!(session.participant_property(78, &property), None);
    assert_eq!(independent.participant_property(77, &property), Some(10));
    assert!(
        session
            .set_participant_properties(&[(property.clone(), 1), (property.clone(), 2)], &inputs())
            .is_err()
    );
    assert_eq!(session.participant_property(77, &property), Some(23));
}

#[test]
fn participant_reader_keeps_missing_values_unready_and_import_failure_is_atomic() {
    use crate::scoring::participant_properties::Property;
    let property = Property {
        channel: "test/race".into(),
        port: 77,
    };
    for output_kind in [Kind::Integer, Kind::Float] {
        let mut reader = node(
            0,
            Behavior::ParticipantProperty {
                kind: Kind::Integer,
                property: property.clone(),
                write: false,
            },
        );
        reader.defaults = vec![(FLOAT_VALUE, Value::Integer(999))];
        let p = Program {
            nodes: vec![reader],
            cells: vec![cell(output_kind, &[port(0, nodes::OUTPUT_VALUE)])],
            connections: vec![],
            startup: vec![Step::Bind(0), Step::Activate(0)],
            unmodeled: vec![],
        };
        let mut session = Session::new(Arc::new(p)).unwrap();
        session.start(&inputs()).unwrap();
        assert_eq!(session.read(port(0, nodes::OUTPUT_VALUE)).unwrap(), None);
        let result = session.set_participant_properties(&[(property.clone(), 0)], &inputs());
        if output_kind == Kind::Integer {
            result.unwrap();
            assert_eq!(
                session.read(port(0, nodes::OUTPUT_VALUE)).unwrap(),
                Some(Value::Integer(0))
            );
        } else {
            assert!(result.is_err());
            assert_eq!(session.participant_property(77, &property), None);
            assert_eq!(session.read(port(0, nodes::OUTPUT_VALUE)).unwrap(), None);
        }
    }
}

#[test]
fn integer_subtraction_and_division_propagate_changes_and_isolate_sessions() {
    use crate::scoring::integers::{self, Operation};
    let mut producer = node(
        1,
        Behavior::Scalar(Scalar::Integer {
            default: 20,
            step: 4,
        }),
    );
    producer.defaults = vec![(FLOAT_VALUE, Value::Integer(20))];
    let program = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            producer,
            node(
                2,
                Behavior::Snapshot(vec![(FLOAT_VALUE, Value::Integer(2))]),
            ),
            node(
                3,
                Behavior::Snapshot(vec![(FLOAT_VALUE, Value::Integer(3))]),
            ),
            node(
                4,
                Behavior::IntegerArithmetic {
                    operations: vec![Operation::Subtract, Operation::Divide],
                },
            ),
        ],
        cells: vec![
            cell(
                Kind::Integer,
                &[port(1, FLOAT_VALUE), port(4, integers::port("In", 0))],
            ),
            cell(
                Kind::Integer,
                &[port(2, FLOAT_VALUE), port(4, integers::port("In", 1))],
            ),
            cell(
                Kind::Integer,
                &[port(3, FLOAT_VALUE), port(4, integers::port("In", 2))],
            ),
            cell(Kind::Integer, &[port(4, OUT)]),
        ],
        connections: vec![Connection {
            source: Endpoint { node: 0, event: 1 },
            target: Endpoint {
                node: 1,
                event: nodes::INCREMENT,
            },
            delivery: Delivery::Local,
        }],
        startup: vec![
            Step::Bind(1),
            Step::Bind(2),
            Step::Bind(3),
            Step::Bind(4),
            Step::Register(Registration::Notify(port(4, integers::port("In", 0)))),
        ],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    session.start(&inputs()).unwrap();
    assert_eq!(session.read(port(4, OUT)).unwrap(), Some(Value::Integer(6)));
    let independent = session.clone();
    session.event(0, 1, &inputs()).unwrap();
    assert_eq!(session.read(port(4, OUT)).unwrap(), Some(Value::Integer(7)));
    assert_eq!(
        independent.read(port(4, OUT)).unwrap(),
        Some(Value::Integer(6))
    );
}

#[test]
fn vector_dot_recomputes_from_owned_velocity_and_rolls_back_invalid_samples() {
    use crate::scoring::{
        vector_dot,
        vehicle_query::{Bindings, IDENTITY, Query, VELOCITY},
    };
    let program = Program {
        nodes: vec![
            node(
                0,
                Behavior::VehicleQuery(Bindings::new(Query::Physics, &[(VELOCITY, true)]).unwrap()),
            ),
            node(
                1,
                Behavior::Snapshot(vec![(FLOAT_VALUE, Value::Vec3([2., 3., 4.]))]),
            ),
            node(2, Behavior::VectorDot),
        ],
        cells: vec![
            cell(Kind::Vec3, &[port(0, VELOCITY), port(2, vector_dot::LEFT)]),
            cell(
                Kind::Vec3,
                &[port(1, FLOAT_VALUE), port(2, vector_dot::RIGHT)],
            ),
            cell(Kind::Float, &[port(2, vector_dot::OUT)]),
        ],
        connections: vec![],
        startup: vec![
            Step::Bind(0),
            Step::Bind(1),
            Step::Bind(2),
            Step::Register(Registration::Notify(port(2, vector_dot::LEFT))),
            Step::Activate(0),
            Step::Activate(2),
        ],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    let mut input = inputs();
    session.start(&input).unwrap();
    assert_eq!(
        session.read(port(2, vector_dot::OUT)).unwrap(),
        Some(Value::Float(0.))
    );
    input.vehicle = Some(Vehicle {
        identity: 1,
        transform: IDENTITY,
        velocity: [5., 6., 7.],
        angular_velocity: [0.; 3],
        airborne: Some(false),
        state_3c230e35: Some(false),
        state_cd5ae2f0: Some(false),
        max_rpm: None,
        rpm: None,
        nos: None,
        burnout: None,
    });
    session.tick(0.1, &input).unwrap();
    assert_eq!(
        session.read(port(2, vector_dot::OUT)).unwrap(),
        Some(Value::Float(56.))
    );
    let independent = session.clone();
    input.vehicle.as_mut().unwrap().velocity = [1., 0., 0.];
    session.tick(0.1, &input).unwrap();
    assert_eq!(
        session.read(port(2, vector_dot::OUT)).unwrap(),
        Some(Value::Float(2.))
    );
    assert_eq!(
        independent.read(port(2, vector_dot::OUT)).unwrap(),
        Some(Value::Float(56.))
    );
    input.vehicle.as_mut().unwrap().velocity = [f32::NAN, 0., 0.];
    assert!(session.tick(0.1, &input).is_err());
    assert_eq!(
        session.read(port(2, vector_dot::OUT)).unwrap(),
        Some(Value::Float(2.))
    );
}

#[test]
fn integer_rank_trigger_sum_and_snapshot_are_owned_and_ordered() {
    use crate::scoring::integers;
    for on_change in [false, true] {
        let mut producer = node(
            1,
            Behavior::Scalar(Scalar::Integer {
                default: 7,
                step: 3,
            }),
        );
        producer.defaults = vec![(FLOAT_VALUE, Value::Integer(7))];
        let nodes = vec![
            node(0, Behavior::Inert),
            producer,
            node(
                2,
                Behavior::Snapshot(vec![(FLOAT_VALUE, Value::Integer(9))]),
            ),
            node(
                3,
                Behavior::RankIntegers {
                    count: 2,
                    on_change,
                },
            ),
            node(4, Behavior::IntegerSum { count: 2 }),
        ];
        let mut startup = vec![Step::Bind(1), Step::Bind(2), Step::Bind(3), Step::Bind(4)];
        for id in [3, 4] {
            for i in 0..2 {
                startup.push(Step::Register(Registration::Notify(port(
                    id,
                    integers::port("In", i),
                ))));
            }
        }
        let p = Program {
            nodes,
            cells: vec![
                cell(
                    Kind::Integer,
                    &[
                        port(1, FLOAT_VALUE),
                        port(3, integers::port("In", 0)),
                        port(4, integers::port("In", 0)),
                    ],
                ),
                cell(
                    Kind::Integer,
                    &[
                        port(2, FLOAT_VALUE),
                        port(3, integers::port("In", 1)),
                        port(4, integers::port("In", 1)),
                    ],
                ),
                cell(Kind::Integer, &[port(3, integers::port("Out", 0))]),
                cell(Kind::Integer, &[port(3, integers::port("Out", 1))]),
                cell(Kind::Integer, &[port(4, OUT)]),
            ],
            connections: vec![
                Connection {
                    source: Endpoint { node: 0, event: 1 },
                    target: Endpoint {
                        node: 1,
                        event: nodes::INCREMENT,
                    },
                    delivery: Delivery::Local,
                },
                Connection {
                    source: Endpoint { node: 0, event: 2 },
                    target: Endpoint {
                        node: 3,
                        event: integers::SORT,
                    },
                    delivery: Delivery::Local,
                },
            ],
            startup,
            unmodeled: vec![],
        };
        let mut s = Session::new(Arc::new(p)).unwrap();
        s.start(&inputs()).unwrap();
        assert_eq!(
            s.read(port(3, integers::port("Out", 0))).unwrap(),
            Some(Value::Integer(2))
        );
        assert_eq!(s.read(port(4, OUT)).unwrap(), Some(Value::Integer(16)));
        let independent = s.clone();
        s.event(0, 1, &inputs()).unwrap();
        assert_eq!(s.read(port(4, OUT)).unwrap(), Some(Value::Integer(19)));
        assert_eq!(
            s.read(port(3, integers::port("Out", 0))).unwrap(),
            Some(Value::Integer(if on_change { 1 } else { 2 }))
        );
        s.event(0, 2, &inputs()).unwrap();
        assert_eq!(
            s.read(port(3, integers::port("Out", 0))).unwrap(),
            Some(Value::Integer(1))
        );
        assert_eq!(
            s.read(port(3, integers::port("Out", 1))).unwrap(),
            Some(Value::Integer(2))
        );
        assert_eq!(
            independent.read(port(4, OUT)).unwrap(),
            Some(Value::Integer(16))
        );
    }
}

#[test]
fn buildup_uses_ready_inputs_merges_observers_and_consumes_event_counts_once() {
    use crate::scoring::buildup::*;
    let mut nodes = vec![node(0, Behavior::NosBuildup), node(1, Behavior::NosBuildup)];
    for node in &mut nodes {
        node.defaults = vec![
            (DRIFTING, Value::Boolean(false)),
            (DRAFTING, Value::Boolean(false)),
            (ONCOMING, Value::Boolean(true)),
            (AIRBORNE, Value::Boolean(false)),
            (SPEED, Value::Float(10.0)),
            (PACING, Value::Float(0.25)),
        ];
    }
    nodes.push(node(2, Behavior::Inert));
    let program = Program {
        nodes,
        cells: vec![cell(Kind::Float, &[port(0, SPEED)])],
        connections: [(100, 0, NEAR_MISS), (101, 1, NEAR_MISS), (102, 1, COP_SLAM)]
            .into_iter()
            .map(|(event, node, target)| Connection {
                source: Endpoint { node: 2, event },
                target: Endpoint {
                    node,
                    event: target,
                },
                delivery: Delivery::Local,
            })
            .collect(),
        startup: vec![
            Step::Default {
                port: port(0, SPEED),
                value: Value::Float(20.0),
                mode: super::super::properties::DefaultMode::Override,
            },
            Step::Activate(0),
            Step::Activate(1),
        ],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    let mut owned = inputs();
    owned.vehicle = Some(Vehicle {
        identity: 1,
        transform: [[0.0; 3]; 4],
        velocity: [0.0; 3],
        angular_velocity: [0.0; 3],
        airborne: None,
        state_3c230e35: None,
        state_cd5ae2f0: None,
        max_rpm: None,
        rpm: None,
        burnout: None,
        nos: Some(nos::Sample {
            engaged: false,
            level: 1.0,
            capacity: 8.0,
        }),
    });
    let startup = session.start(&owned).unwrap();
    assert_eq!(startup.buildup.speed, 20.0);
    assert!(startup.buildup.oncoming);
    session.event(2, 100, &owned).unwrap();
    session.event(2, 100, &owned).unwrap();
    session.event(2, 101, &owned).unwrap();
    session.event(2, 102, &owned).unwrap();
    let mut independent = session.clone();
    let out = session.tick(0.1, &owned).unwrap();
    assert_eq!(out.buildup.near_misses, 2);
    assert_eq!(out.buildup.cop_slams, 1);
    assert_eq!(session.tick(0.1, &owned).unwrap().buildup.near_misses, 0);
    assert_eq!(
        independent.tick(0.1, &owned).unwrap().buildup.near_misses,
        2
    );
    let mut absent = owned.clone();
    absent.vehicle = None;
    session.event(2, 100, &absent).unwrap();
    assert_eq!(session.tick(0.1, &owned).unwrap().buildup.near_misses, 0);
}

/// Float(1) -> Accumulator(2) input; Accumulator output -> SyncedFloat(3);
/// event 0x100 at root(0) -> Accumulate; Accumulator OnOutputChanged -> Score(4)
/// Award through context(5) with one keeper.
fn program() -> Program {
    let nodes = vec![
        node(0, Behavior::Inert),
        node(
            1,
            Behavior::Scalar(Scalar::Float {
                default: 2.5,
                step: 1.0,
            }),
        ),
        node(2, Behavior::Scalar(Scalar::Accumulator)),
        node(3, Behavior::Synced(synced::Kind::Float)),
        node(
            4,
            Behavior::Score {
                default: 100,
                context: 5,
            },
        ),
        node(
            5,
            Behavior::ScoreContext {
                participant_provider: 0,
                keepers: vec![Keeper {
                    node: 0,
                    field: 0x5151,
                }],
            },
        ),
    ];
    let cells = vec![
        cell(Kind::Float, &[port(1, FLOAT_VALUE), port(2, INPUT)]),
        cell(Kind::Float, &[port(2, OUTPUT), port(3, synced::IN)]),
        cell(Kind::Float, &[port(3, synced::OUT)]),
    ];
    let connections = vec![
        Connection {
            source: Endpoint {
                node: 0,
                event: 0x100,
            },
            target: Endpoint {
                node: 2,
                event: ACCUMULATE,
            },
            delivery: Delivery::Local,
        },
        Connection {
            source: Endpoint {
                node: 2,
                event: OUTPUT_CHANGED,
            },
            target: Endpoint {
                node: 4,
                event: score::AWARD,
            },
            delivery: Delivery::Local,
        },
    ];
    let startup = vec![
        Step::Bind(1),
        Step::Bind(2),
        Step::Register(Registration::Notify(port(2, INPUT))),
        Step::Bind(3),
        Step::Register(Registration::Notify(port(3, synced::IN))),
        Step::Bind(4),
        Step::Bind(5),
        Step::Activate(1),
        Step::Activate(2),
        Step::Activate(3),
        Step::Activate(4),
        Step::Activate(5),
    ];
    Program {
        nodes,
        cells,
        connections,
        startup,
        unmodeled: vec![],
    }
}

#[test]
fn startup_seeds_outputs_and_events_flow_into_synced_values_and_awards() {
    let mut session = Session::new(Arc::new(program())).unwrap();
    assert!(session.tick(0.1, &inputs()).is_err());
    let output = session.start(&inputs()).unwrap();
    // Float default and accumulator zero are seeded at bind; the synced node
    // copied the ready accumulator output at bind, which marks it dirty.
    assert_eq!(
        session.read(port(1, FLOAT_VALUE)).unwrap(),
        Some(Value::Float(2.5))
    );
    assert_eq!(
        session.read(port(2, OUTPUT)).unwrap(),
        Some(Value::Float(0.0))
    );
    assert_eq!(output.synced, vec![(3, Value::Float(0.0))]);
    assert!(output.awards.is_empty());
    assert!(session.start(&inputs()).is_err());
    let output = session.event(0, 0x100, &inputs()).unwrap();
    assert_eq!(
        session.read(port(2, OUTPUT)).unwrap(),
        Some(Value::Float(2.5))
    );
    assert_eq!(output.synced, vec![(3, Value::Float(2.5))]);
    // Score input is unbound, so the authored default applies; the keeper ID
    // comes from the activated context, and the participant from inputs.
    assert_eq!(
        output.awards,
        vec![Award {
            keeper: 0x5151,
            participant: 77,
            amount: 100
        }]
    );
    let again = session.event(0, 0x100, &inputs()).unwrap();
    assert_eq!(
        session.read(port(2, OUTPUT)).unwrap(),
        Some(Value::Float(5.0))
    );
    assert_eq!(again.awards.len(), 1);
    assert_eq!(again.synced, vec![(3, Value::Float(5.0))]);
    let idle = session.tick(0.1, &inputs()).unwrap();
    assert_eq!(idle, Output::default());
}

#[test]
fn failed_transactions_leave_the_session_unchanged() {
    let mut program = program();
    // A Math node whose output is bound but whose program reads an unbound
    // integer: evaluation is suppressed, not failed.
    let mut session = Session::new(Arc::new(program.clone())).unwrap();
    session.start(&inputs()).unwrap();
    let before = session.read(port(2, OUTPUT)).unwrap();
    // Unknown node event fails and changes nothing.
    assert_eq!(session.event(9, 0x100, &inputs()), Err(Error::UnknownNode));
    assert_eq!(session.read(port(2, OUTPUT)).unwrap(), before);
    // A score context that is not a context is rejected by validation.
    program.nodes[4].behavior = Behavior::Score {
        default: 1,
        context: 1,
    };
    assert_eq!(
        Session::new(Arc::new(program)).err(),
        Some(Error::InvalidGraph)
    );
}

#[test]
fn prefab_forwarding_and_event_realm_gate_delivery() {
    let mut program = program();
    // Prefab reference (6) and its interface (7) are not entities; events
    // crossing the boundary are re-emitted at the other side.
    let mut reference = node(6, Behavior::Inert);
    reference.created = false;
    reference.events = false;
    reference.properties = false;
    reference.forward = Some(7);
    let mut interface = reference.clone();
    interface.identity = identity(7);
    interface.forward = Some(6);
    program.nodes.extend([reference, interface]);
    program.connections[0].target = Endpoint {
        node: 6,
        event: 0x100,
    };
    program.connections.push(Connection {
        source: Endpoint {
            node: 7,
            event: 0x100,
        },
        target: Endpoint {
            node: 2,
            event: ACCUMULATE,
        },
        delivery: Delivery::Local,
    });
    // Accumulator input bound but never ready: Accumulate fails and the
    // whole transaction rolls back.
    program.cells[0].ports = vec![port(2, INPUT)];
    program
        .startup
        .retain(|s| !matches!(s, Step::Bind(1) | Step::Activate(1)));
    program.nodes[1].created = false;
    program.nodes[1].events = false;
    program.nodes[1].properties = false;
    program.nodes[1].behavior = Behavior::Inert;
    let mut session = Session::new(Arc::new(program.clone())).unwrap();
    session.start(&inputs()).unwrap();
    assert_eq!(session.event(0, 0x100, &inputs()), Err(Error::MissingInput));
    assert_eq!(
        session.read(port(2, OUTPUT)).unwrap(),
        Some(Value::Float(0.0))
    );
    // A node outside the server event realm ignores delivered events.
    program.nodes[2].events = false;
    let mut session = Session::new(Arc::new(program)).unwrap();
    session.start(&inputs()).unwrap();
    let output = session.event(0, 0x100, &inputs()).unwrap();
    assert!(output.awards.is_empty());
    assert_eq!(
        session.read(port(2, OUTPUT)).unwrap(),
        Some(Value::Float(0.0))
    );
    let _ = OUT;
}

/// Regression (E1130): the drift train counted every replicated car, so a
/// traffic car beside a solo drift earned Crew REP. The filter reads only the
/// members of its participant set.
#[test]
fn proximity_counts_members_of_its_set_and_ignores_other_cars() {
    use crate::scoring::proximity::{
        COUNT, DRIFT_PARTICIPANTS, HIGH_SPEED_PARTICIPANTS, Member, RADIUS,
    };
    let mut filter = node(
        0,
        Behavior::Proximity {
            set: DRIFT_PARTICIPANTS,
        },
    );
    filter.defaults = vec![(RADIUS, Value::Float(30.0))];
    let program = Program {
        nodes: vec![filter],
        cells: vec![cell(Kind::Integer, &[port(0, COUNT)])],
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    };
    let car = |identity: u32, x: f32| draft::Vehicle {
        identity,
        position: [x, 0.0, 0.0],
        velocity: [20.0, 0.0, 0.0],
    };
    let mut owned = inputs();
    owned.vehicle = Some(Vehicle {
        identity: 1,
        transform: [[0.0; 3]; 4],
        velocity: [20.0, 0.0, 0.0],
        angular_velocity: [0.0; 3],
        airborne: None,
        state_3c230e35: None,
        state_cd5ae2f0: None,
        max_rpm: None,
        rpm: None,
        nos: None,
    });
    // Traffic and rival cars alongside: replicated, but in no set.
    owned.others = vec![car(2, 5.0), car(3, 12.0)];
    let mut session = Session::new(Arc::new(program)).unwrap();
    session.start(&owned).unwrap();
    session.tick(0.1, &owned).unwrap();
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(0))
    );
    // A high-speed member is in the other set; the owner never counts.
    owned.participants = vec![
        Member {
            set: HIGH_SPEED_PARTICIPANTS,
            vehicle: car(4, 8.0),
        },
        Member {
            set: DRIFT_PARTICIPANTS,
            vehicle: car(1, 0.0),
        },
    ];
    session.tick(0.1, &owned).unwrap();
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(0))
    );
    // A drifting participant 10 m away joins the train.
    owned.participants.push(Member {
        set: DRIFT_PARTICIPANTS,
        vehicle: car(5, 10.0),
    });
    session.tick(0.1, &owned).unwrap();
    assert_eq!(
        session.read(port(0, COUNT)).unwrap(),
        Some(Value::Integer(1))
    );
}
