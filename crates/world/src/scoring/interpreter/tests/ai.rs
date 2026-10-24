// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::{
    ai_commands::{
        self as ai, Change, Control, Definition as Ai, Enable, Goto, Override, Scripted,
    },
    ai_control,
};

fn key(port: u32) -> participant_properties::Property {
    participant_properties::Property {
        channel: "test/ai".into(),
        port,
    }
}
fn fixture() -> Program {
    let transform = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [1., 2., 3.]];
    let mut goto = node(
        6,
        Behavior::Ai(Ai::Goto(Goto {
            location: None,
            transform,
            drift: false,
            stop: true,
            radius: 150.,
        })),
    );
    goto.defaults
        .push((ai::LOCATION_TRANSFORM, Value::Transform(transform)));
    let mut p = Program {
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
                Behavior::Ai(Ai::Enable(Enable {
                    inclusion: ai_control::Inclusion::All,
                    add_on_enable: true,
                    remove_on_disable: true,
                    state: Some(4),
                    actor_manager: key(1),
                    ai_manager: key(2),
                })),
            ),
            node(
                4,
                Behavior::Ai(Ai::State {
                    handles_marked_for_delete: false,
                    send_rpc: true,
                }),
            ),
            node(
                5,
                Behavior::Ai(Ai::Control(Control {
                    context_hash: 42,
                    one_by_one: false,
                    apply_existing: false,
                    states: vec![4],
                    commands: vec![6],
                    command_manager: key(3),
                    actor_manager: key(1),
                    context_manager: key(4),
                })),
            ),
            goto,
            node(
                7,
                Behavior::ParticipantEvent {
                    provider: Some(1),
                    filter: false,
                },
            ),
            node(
                8,
                Behavior::ParticipantEvent {
                    provider: Some(1),
                    filter: false,
                },
            ),
        ],
        cells: vec![],
        connections: vec![],
        startup: vec![
            Step::Bind(1),
            Step::Activate(3),
            Step::Activate(4),
            Step::Activate(5),
        ],
        unmodeled: vec![],
    };
    for (event, converter, command) in [
        (100, 2, ai_control::ENABLE),
        (101, 7, ai_control::DISABLE),
        (102, 8, ai_control::RELEASE),
    ] {
        p.connections.push(Connection {
            source: Endpoint { node: 0, event },
            target: Endpoint {
                node: converter,
                event: nodes::IN,
            },
            delivery: Delivery::Local,
        });
        p.connections.push(Connection {
            source: Endpoint {
                node: converter,
                event: nodes::OUT,
            },
            target: Endpoint {
                node: 3,
                event: command,
            },
            delivery: Delivery::Local,
        });
    }
    p.connections.push(Connection {
        source: Endpoint {
            node: 0,
            event: 103,
        },
        target: Endpoint {
            node: 5,
            event: ai::NEXT,
        },
        delivery: Delivery::Local,
    });
    for (index, (node, event)) in [
        (3, ai_control::ENABLED),
        (3, ai_control::DISABLED),
        (5, ai::ENTERED),
        (6, ai::COMPLETED),
    ]
    .into_iter()
    .enumerate()
    {
        p.connections.push(Connection {
            source: Endpoint { node, event },
            target: Endpoint { node: 0, event },
            delivery: Delivery::NetworkedClient(crate::scoring::events::Remote {
                bus: 0,
                index: index as u16,
            }),
        });
    }
    p
}

#[test]
fn scoped_control_commands_repeat_disable_complete_and_release_in_native_order() {
    let mut s = Session::new(Arc::new(fixture())).unwrap();
    assert_eq!(s.start(&inputs()).unwrap(), Output::default());
    let output = s.event(0, 100, &inputs()).unwrap();
    assert!(matches!(
        &output.ai[..],
        [
            Change::Override {
                action: Override::Acquire,
                ..
            },
            Change::Override {
                action: Override::Set(true),
                ..
            },
            Change::State {
                node: 4,
                participant: 77,
                entered: true
            },
            Change::Start {
                context_hash: 42,
                ..
            }
        ]
    ));
    assert_eq!(
        output
            .remote
            .iter()
            .map(|r| r.source.event)
            .collect::<Vec<_>>(),
        [ai_control::ENABLED, ai::ENTERED]
    );
    let Change::Start { goals, .. } = &output.ai[3] else {
        panic!()
    };
    assert!(matches!(
        goals[0].command,
        ai::Command::Goto {
            point: [1., 2., 3.],
            stop: true,
            ..
        }
    ));
    let token = goals[0].token;
    let repeat = s.event(0, 100, &inputs()).unwrap();
    assert!(matches!(
        &repeat.ai[..],
        [
            Change::Override {
                action: Override::Set(true),
                ..
            },
            Change::State { entered: true, .. }
        ]
    ));
    assert_eq!(repeat.remote.len(), 1);
    let disabled = s.event(0, 101, &inputs()).unwrap();
    assert!(matches!(
        &disabled.ai[..],
        [
            Change::Override {
                action: Override::Set(false),
                ..
            },
            Change::State { entered: false, .. }
        ]
    ));
    // Direct removal does not invoke the ControlAI transition observer.
    let done = s.complete_ai_command(token, &inputs()).unwrap();
    assert_eq!(done.remote.len(), 1);
    assert_eq!(
        done.remote[0].source,
        Endpoint {
            node: 6,
            event: ai::COMPLETED
        }
    );
    assert_eq!(done.remote[0].context, Some(Context::Participant(77)));
    assert_eq!(
        s.complete_ai_command(token, &inputs()).unwrap(),
        Output::default()
    );
    assert!(matches!(
        &s.event(0, 102, &inputs()).unwrap().ai[..],
        [Change::Override {
            action: Override::Release,
            ..
        }]
    ));
    assert_eq!(s.event(0, 102, &inputs()).unwrap(), Output::default());
    assert!(matches!(
        &s.retire(&inputs()).unwrap().ai[..],
        [Change::Release {
            node: 5,
            participant: 77,
            ..
        }]
    ));
}

#[test]
fn next_replaces_context_and_rejects_stale_completion_without_changing_other_instances() {
    let mut p = fixture();
    let Behavior::Ai(Ai::Control(control)) = &mut p.nodes[5].behavior else {
        panic!()
    };
    control.one_by_one = true;
    control.commands.push(9);
    p.nodes.push(node(
        9,
        Behavior::Ai(Ai::Scripted(Scripted {
            throttle: 1.,
            steering: 0.,
            brake: 1.,
            handbrake: 0.,
            use_nos: false,
        })),
    ));
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    let mut other = s.clone();
    let start = s.event(0, 100, &inputs()).unwrap();
    let Change::Start { goals, .. } = &start.ai[3] else {
        panic!()
    };
    let old = goals[0].token;
    let next = s.event(0, 103, &inputs()).unwrap();
    assert!(matches!(
        &next.ai[..],
        [Change::Release { .. }, Change::Start { .. }]
    ));
    let Change::Start { goals, .. } = &next.ai[1] else {
        panic!()
    };
    assert!(matches!(goals[0].command, ai::Command::Scripted(_)));
    assert_ne!(old, goals[0].token);
    assert_eq!(
        s.complete_ai_command(old, &inputs()).unwrap(),
        Output::default()
    );
    assert_eq!(
        other.complete_ai_command(old, &inputs()).unwrap(),
        Output::default()
    );
    assert!(matches!(
        &s.retire(&inputs()).unwrap().ai[..],
        [
            Change::Release { .. },
            Change::Override {
                action: Override::Release,
                ..
            }
        ]
    ));
}

#[test]
fn missing_location_rolls_back_control_and_tokens_and_membership_filter_is_explicit() {
    let mut p = fixture();
    let Behavior::Ai(Ai::Goto(goto)) = &mut p.nodes[6].behavior else {
        panic!()
    };
    goto.location = Some(key(5));
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(s.event(0, 100, &inputs()), Err(Error::MissingInput));
    s.bind_ai_location(key(5), Some([[0.; 3], [0.; 3], [0.; 3], [10., 20., 30.]]))
        .unwrap();
    let out = s.event(0, 100, &inputs()).unwrap();
    let Change::Start { token, goals, .. } = &out.ai[3] else {
        panic!()
    };
    assert_eq!(*token, 1);
    assert!(matches!(
        goals[0].command,
        ai::Command::Goto {
            point: [10., 20., 30.],
            ..
        }
    ));
    let mut p = fixture();
    let Behavior::Ai(Ai::Enable(enable)) = &mut p.nodes[3].behavior else {
        panic!()
    };
    enable.inclusion = ai_control::Inclusion::Ai;
    let mut s = Session::new(Arc::new(p)).unwrap();
    s.start(&inputs()).unwrap();
    assert_eq!(s.event(0, 100, &inputs()), Err(Error::MissingInput));
    s.bind_ai_membership(key(2), 77, false).unwrap();
    assert_eq!(s.event(0, 100, &inputs()).unwrap(), Output::default());
    s.bind_ai_membership(key(2), 77, true).unwrap();
    assert!(!s.event(0, 100, &inputs()).unwrap().ai.is_empty());
}
