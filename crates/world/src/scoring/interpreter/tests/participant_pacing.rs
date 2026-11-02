// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    participant_pacing::{NOS_REBUILD, Sources},
    participant_properties::Property,
};

fn sources(channel: &str) -> Sources {
    let key = |port| Property {
        channel: channel.into(),
        port,
    };
    Sources {
        pacing: key(1),
        actors: key(2),
        personalities: key(3),
        curves: key(4),
    }
}
fn program() -> Program {
    Program {
        nodes: vec![
            node(0, Behavior::ParticipantProvider),
            node(
                1,
                Behavior::ParticipantPacing {
                    sources: sources("services"),
                    provider: 0,
                },
            ),
        ],
        cells: vec![cell(Kind::Float, &[port(1, NOS_REBUILD)])],
        connections: vec![],
        startup: vec![
            Step::Bind(0),
            Step::Bind(1),
            Step::Activate(0),
            Step::Activate(1),
        ],
        unmodeled: vec![],
    }
}

#[test]
fn standalone_participant_property_retirement_does_not_require_a_native_host() {
    let property = Property {
        channel: "driving".into(),
        port: 7,
    };
    let program = Program {
        nodes: vec![
            node(
                0,
                Behavior::Constant {
                    field: 10,
                    value: Value::Boolean(true),
                },
            ),
            node(
                1,
                Behavior::ParticipantProperty {
                    property,
                    kind: Kind::Boolean,
                    write: true,
                },
            ),
        ],
        cells: vec![cell(Kind::Boolean, &[port(0, 10), port(1, FLOAT_VALUE)])],
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Bind(1), Step::Activate(1)],
        unmodeled: vec![],
    };
    let mut session = Session::new(Arc::new(program)).unwrap();
    let output = session.start(&inputs()).unwrap();
    assert_eq!(output.participant_properties.len(), 1);
    assert_eq!(output.participant_properties[0].value, Value::Boolean(true));
    assert!(
        session
            .retire(&inputs())
            .unwrap()
            .participant_properties
            .is_empty()
    );
}
#[test]
fn pacing_is_sampled_after_activation_and_missing_samples_retain_output() {
    let mut session = Session::new(Arc::new(program())).unwrap();
    let input = inputs();
    let source = sources("services");
    session
        .set_pacing_sample(&source, input.participant, Some(0.75))
        .unwrap();
    session.start(&input).unwrap();
    let output = port(1, NOS_REBUILD);
    assert_eq!(session.read(output).unwrap(), Some(Value::Float(0.)));
    session.tick(0.1, &input).unwrap();
    assert_eq!(session.read(output).unwrap(), Some(Value::Float(0.75)));
    session
        .set_pacing_sample(&source, input.participant, None)
        .unwrap();
    session
        .set_pacing_sample(&source, input.participant + 1, Some(99.))
        .unwrap();
    session.tick(0.1, &input).unwrap();
    assert_eq!(session.read(output).unwrap(), Some(Value::Float(0.75)));
    assert!(
        session
            .set_pacing_sample(&sources("other"), input.participant, Some(3.))
            .is_err()
    );
    let before = session.state.clone();
    assert!(
        session
            .set_pacing_sample(&source, input.participant, Some(f32::NAN))
            .is_err()
    );
    assert_eq!(session.state, before);
    session
        .set_pacing_sample(&source, input.participant, Some(0.25))
        .unwrap();
    let before = session.state.clone();
    assert!(session.tick(f32::NAN, &input).is_err());
    assert_eq!(session.state, before);
    session.tick(0.1, &input).unwrap();
    assert_eq!(session.read(output).unwrap(), Some(Value::Float(0.25)));
    let mut other = Session::new(Arc::new(program())).unwrap();
    other
        .start(&Inputs {
            participant: 88,
            ..input.clone()
        })
        .unwrap();
    other
        .tick(
            0.1,
            &Inputs {
                participant: 88,
                ..input
            },
        )
        .unwrap();
    assert_eq!(other.read(output).unwrap(), Some(Value::Float(0.)));
    let mut invalid = program();
    invalid.nodes[0].behavior = Behavior::Inert;
    assert!(Session::new(Arc::new(invalid)).is_err());
}
