// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::scoring::events::Composition;
use nfs_world::scoring::{
    events::{Connection, Delivery, Remote},
    nodes,
    program::{Behavior, Identity, Node, Program, Step},
    properties::{Definition as Cell, Kind, Port, Registration},
};

pub(crate) fn fixture(fail: bool) -> Arc<Definition> {
    let behavior = [
        Behavior::Native,
        Behavior::EventSync,
        Behavior::Scalar(nodes::Definition::CompareBoolean {
            on_change: false,
            on_start: false,
            always_send: false,
        }),
        Behavior::Scalar(nodes::Definition::Boolean { default: false }),
        Behavior::Scalar(nodes::Definition::CompareBoolean {
            on_change: true,
            on_start: false,
            always_send: false,
        }),
        Behavior::Native,
        Behavior::EventSync,
        Behavior::Native,
        Behavior::Native,
        Behavior::EventSync,
    ];
    let list = behavior
        .into_iter()
        .enumerate()
        .map(|(i, behavior)| Node {
            identity: Identity {
                asset: "constructed".into(),
                index: i as u32,
                instances: vec![],
            },
            class: "constructed".into(),
            created: true,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior,
        })
        .collect::<Vec<_>>();
    let port = |node, field| Port { node, field };
    let mut connections = vec![];
    let mut edge = |node, event, target, input, delivery| {
        connections.push(Connection {
            source: Endpoint { node, event },
            target: Endpoint {
                node: target,
                event: input,
            },
            delivery,
        })
    };
    edge(0, STARTED, 2, nodes::IN, Delivery::Local);
    edge(
        2,
        nodes::ON_FALSE,
        0,
        0,
        Delivery::NetworkedClient(Remote { bus: 0, index: 7 }),
    );
    edge(1, OUT, 5, AWARD, Delivery::Local);
    edge(5, AWARDED, 7, 0xc6f6_466d, Delivery::Local);
    edge(
        5,
        AWARDED,
        0,
        0,
        Delivery::NetworkedClient(Remote { bus: 0, index: 11 }),
    );
    edge(6, OUT, 3, nodes::SET_TRUE, Delivery::Local);
    edge(4, nodes::ON_TRUE, 0, STOP, Delivery::Local);
    edge(9, OUT, 5, AWARD, Delivery::Local);
    if fail {
        edge(9, OUT, 8, 123, Delivery::Local);
    }
    let mut startup = vec![];
    for i in 0..10 {
        startup.push(Step::Bind(i));
        if i == 2 || i == 4 {
            startup.push(Step::Register(Registration::Notify(port(i, nodes::BOOL))));
        }
    }
    startup.extend((0..10).map(Step::Activate));
    let program = Program {
        nodes: list,
        cells: vec![Cell {
            kind: Kind::Boolean,
            initial: None,
            ports: vec![
                port(3, nodes::FLOAT_VALUE),
                port(2, nodes::BOOL),
                port(4, nodes::BOOL),
            ],
            listeners: vec![],
        }],
        connections: connections.clone(),
        startup,
        unmodeled: vec![],
    };
    Arc::new(Definition {
        program,
        composition: Composition {
            nodes: vec![],
            buses: vec![],
            bus_entities: vec![Some(1)],
            connections,
            properties: vec![],
            links: vec![],
        },
        signals: vec![(1, 41), (6, 42), (9, 43)]
            .into_iter()
            .map(|(node, event)| scoring::event_sync::Binding {
                entity: 1,
                event,
                node,
            })
            .collect(),
        services: BTreeMap::from([
            (0, Service::Root),
            (
                5,
                Service::AwardItem {
                    definition: [7; 16],
                },
            ),
            (7, Service::ItemSelector),
            (8, Service::Console),
        ]),
        root: 0,
        links: BTreeMap::new(),
    })
}
fn reached(event: u32, player: u8) -> Message {
    Message::Reached {
        event,
        player,
        target: EntityRef {
            ghost: 51,
            entity: 1,
        },
    }
}
#[test]
fn selection_runs_authored_branches_with_owned_signals_and_acknowledged_awards() {
    let (mut run, initial) = Execution::start(fixture(false), 51, 77, 0, 0.0).unwrap();
    assert_eq!(
        initial.remote,
        vec![Message::Fire {
            events: vec![7],
            player: None,
            target: EntityRef {
                ghost: 51,
                entity: 1
            }
        }]
    );
    assert!(run.awarded(5, 0.0).is_err());
    assert!(run.signal(&reached(41, 1), 0.0).unwrap().is_none());
    let choice = run.signal(&reached(41, 0), 0.0).unwrap().unwrap();
    assert_eq!(
        choice.commands,
        vec![Command::Award {
            node: 5,
            definition: [7; 16]
        }]
    );
    assert!(choice.remote.is_empty());
    assert!(
        run.signal(&reached(41, 0), 0.0)
            .unwrap()
            .unwrap()
            .commands
            .is_empty()
    );
    let committed = run.awarded(5, 0.0).unwrap();
    assert_eq!(committed.commands, vec![Command::Select { node: 7 }]);
    assert!(matches!(&committed.remote[..],[Message::Fire{events,..}] if events==&[11]));
    assert!(run.awarded(5, 0.0).unwrap().commands.is_empty());
    let finished = run.signal(&reached(42, 0), 0.0).unwrap().unwrap();
    assert_eq!(finished.commands, vec![Command::Stop]);
    assert!(run.signal(&reached(42, 0), 0.0).unwrap().is_none());
}
#[test]
fn failed_callback_discards_pending_award_and_other_sessions_stay_independent() {
    let (mut a, _) = Execution::start(fixture(true), 51, 77, 0, 0.0).unwrap();
    let (mut b, _) = Execution::start(fixture(false), 52, 78, 1, 0.0).unwrap();
    assert!(a.signal(&reached(43, 0), 0.0).is_err());
    assert!(a.services.awards.is_empty());
    assert!(a.awarded(5, 0.0).is_err());
    assert!(b.signal(&reached(41, 0), 0.0).unwrap().is_none());
    assert_eq!(
        a.signal(&reached(41, 0), 0.0)
            .unwrap()
            .unwrap()
            .commands
            .len(),
        1
    );
    assert!(b.services.awards.is_empty());
}
