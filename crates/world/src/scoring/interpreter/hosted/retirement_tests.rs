// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{cell, inputs, node, port};
use super::*;
use crate::scoring::{participant_properties::Property, participant_sets::CONTAINS};
use events::native::{Call, Callbacks, Host, Request};

fn fixture() -> Session {
    Session::new(Arc::new(Program {
        nodes: vec![
            node(0, Behavior::ParticipantProvider),
            node(
                1,
                Behavior::ParticipantProperty {
                    property: Property {
                        channel: "synthetic/driving".into(),
                        port: 7,
                    },
                    kind: Kind::Boolean,
                    write: false,
                },
            ),
            node(
                2,
                Behavior::ParticipantSetAccessor {
                    set: Some(Property {
                        channel: "synthetic/driving".into(),
                        port: 8,
                    }),
                    provider: Some(0),
                },
            ),
        ],
        cells: vec![cell(Kind::Boolean, &[port(2, CONTAINS)])],
        connections: vec![],
        startup: vec![
            Step::Bind(0),
            Step::Bind(1),
            Step::Bind(2),
            Step::Activate(1),
            Step::Activate(2),
        ],
        unmodeled: vec![],
    }))
    .unwrap()
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Subscriptions {
    properties: Vec<u16>,
    sets: Vec<u16>,
    removed: Vec<u16>,
    reject: Option<u16>,
}
impl Host for Subscriptions {
    fn invoke(&mut self, request: Request, _: &mut dyn Callbacks) -> Result<(), Error> {
        assert_eq!(request.instance, 11);
        match request.call {
            Call::ParticipantObserve { participant, .. } => {
                assert_eq!(participant, inputs().participant);
                self.properties.push(request.node);
            }
            Call::SetObserve { participant, .. } => {
                assert_eq!(participant, inputs().participant);
                self.sets.push(request.node);
            }
            Call::ParticipantUnobserve | Call::SetUnobserve => {
                if self.reject == Some(request.node) {
                    return Err(Error::MissingInput);
                }
                let listeners = if request.call == Call::SetUnobserve {
                    &mut self.sets
                } else {
                    &mut self.properties
                };
                assert!(listeners.contains(&request.node));
                listeners.retain(|&node| node != request.node);
                self.removed.push(request.node);
            }
            _ => return Err(Error::InvalidGraph),
        }
        Ok(())
    }
}

#[test]
fn standalone_set_and_property_observers_retire_without_native_subscriptions() {
    let mut session = fixture();
    session.start(&inputs()).unwrap();
    assert_eq!(
        session.read(port(2, CONTAINS)).unwrap(),
        Some(Value::Boolean(false))
    );
    assert!(
        session
            .retire(&inputs())
            .unwrap()
            .participant_properties
            .is_empty()
    );
}

#[test]
fn hosted_retirement_keeps_unsubscriptions_and_failure_is_retryable() {
    let mut session = fixture();
    let mut host = Subscriptions::default();
    session
        .hosted(
            Action::Start,
            &inputs(),
            Scope {
                instance: 11,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert_eq!(host.properties, [1]);
    assert_eq!(host.sets, [2]);
    let before = session.state.clone();
    // The caller owns the host transaction. Failure after removing the set
    // must allow discarding that candidate and retrying both subscriptions.
    let mut candidate = Subscriptions {
        reject: Some(1),
        ..host.clone()
    };
    assert_eq!(
        session.hosted(
            Action::Retire,
            &inputs(),
            Scope {
                instance: 11,
                host: &mut candidate,
                upstream: None
            }
        ),
        Err(Error::MissingInput)
    );
    assert_eq!(candidate.removed, [2]);
    assert_eq!(session.state, before);
    assert_eq!(host.properties, [1]);
    assert_eq!(host.sets, [2]);
    session
        .hosted(
            Action::Retire,
            &inputs(),
            Scope {
                instance: 11,
                host: &mut host,
                upstream: None,
            },
        )
        .unwrap();
    assert_eq!(host.removed, [2, 1]);
    assert!(host.properties.is_empty() && host.sets.is_empty());
    assert!(session.state.retired);
}
