// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::properties::Kind;
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
fn node(index: u32, behavior: Behavior) -> Node {
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
fn cell(kind: Kind, ports: &[Port]) -> Definition {
    Definition {
        kind,
        initial: None,
        ports: ports.to_vec(),
        listeners: vec![],
    }
}
fn port(node: u16, field: u32) -> Port {
    Port { node, field }
}
fn inputs() -> Inputs {
    Inputs {
        participant: 77,
        vehicle: None,
        others: vec![],
        road: None,
        stats: None,
        now: 0.0,
        time_scale: 1.0,
    }
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
