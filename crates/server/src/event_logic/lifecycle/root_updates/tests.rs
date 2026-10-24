// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::{
    Effect, Host, Plan,
    graphs::{Driver, Instance, Reader, Runtime},
    tests::{definition, identity},
};
use super::*;
use nfs_content::scoring::events::{Composition, DataConnection, Node};
use nfs_world::scoring::{
    events::Endpoint as EventEndpoint,
    interpreter::{Inputs, Session},
    program::{Identity as ProgramIdentity, Node as ProgramNode, Step as Startup},
    properties::Port,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
struct Edge {
    updates: RootUpdates,
    links: Registry,
    trace: Vec<u16>,
    fail: bool,
}
impl Driver for Edge {
    fn resume(&mut self, owner: u64, token: u64, _: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        match self
            .updates
            .resume(owner, token, &self.links)?
            .ok_or(Error::InvalidGraph)?
        {
            Resume::Steps(steps) => Ok(steps),
            Resume::Root(request) => {
                self.trace.push(request.node);
                Ok(vec![])
            }
        }
    }
    fn invoke(&mut self, request: Request, _: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.updates.invoke(&request, &self.links)? {
            return Ok(steps);
        }
        match request.call {
            Call::Bind | Call::Activate | Call::Retire => Ok(vec![]),
            Call::Update => {
                self.trace.push(request.node);
                if self.fail && request.node == 3 {
                    return Err(Error::UnsupportedEvent);
                }
                Ok(if request.node == 2 {
                    vec![Step::Graph {
                        id: 1,
                        action: Action::Graph(Operation::Deliver {
                            target: EventEndpoint { node: 4, event: 13 },
                            context: None,
                        }),
                    }]
                } else {
                    vec![]
                })
            }
            Call::Event { event: 13, .. } => {
                self.trace.push(100 + request.node);
                Ok(vec![])
            }
            _ => Err(Error::UnsupportedEvent),
        }
    }
    fn effect(
        &mut self,
        _: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        Ok(vec![])
    }
}
fn fixture(fail: bool) -> Runtime<Edge> {
    let composition = Composition {
        nodes: (0..5)
            .map(|i| Node {
                identity: identity(i),
                class: if i == 0 {
                    "GameModeEntityData"
                } else {
                    "FixtureNative"
                }
                .into(),
                listeners: None,
                forward: None,
            })
            .collect(),
        buses: vec![],
        bus_entities: vec![],
        connections: vec![],
        properties: vec![],
        links: [(STATES, 2), (STATES, 1), (STATES, 2), (TRANSITIONS, 3)]
            .into_iter()
            .enumerate()
            .map(|(i, (field, node))| DataConnection {
                source: Port { node: 0, field },
                target: Port { node, field: 0 },
                bus: 0,
                index: i as u16,
                flags: None,
            })
            .collect(),
    };
    let program = Program {
        nodes: composition
            .nodes
            .iter()
            .map(|n| ProgramNode {
                identity: ProgramIdentity {
                    asset: n.identity.asset.clone(),
                    index: n.identity.index as u32,
                    instances: vec![],
                },
                class: n.class.clone(),
                created: true,
                events: true,
                properties: true,
                forward: None,
                defaults: vec![],
                behavior: Behavior::Native,
            })
            .collect(),
        cells: vec![],
        connections: vec![],
        startup: (0..5)
            .flat_map(|n| [Startup::Bind(n), Startup::Activate(n)])
            .collect(),
        unmodeled: vec![],
    };
    let mut links = Registry::default();
    links
        .install(1, Arc::new(composition), &BTreeMap::new())
        .unwrap();
    let mut updates = RootUpdates::new(7).unwrap();
    updates
        .install(
            Object {
                instance: 1,
                node: 0,
            },
            &program,
            &links,
        )
        .unwrap();
    assert!(
        updates
            .install(
                Object {
                    instance: 1,
                    node: 0
                },
                &program,
                &links
            )
            .is_err()
    );
    assert!(
        updates
            .install(
                Object {
                    instance: 1,
                    node: 1
                },
                &program,
                &links
            )
            .is_err()
    );
    let mut lifecycle = definition();
    lifecycle.transitions.clear();
    lifecycle.registered_transitions.clear();
    let plan = Arc::new(Plan::new(&lifecycle, |_| unreachable!()).unwrap());
    let host = Host::new(plan).unwrap();
    let mut runtime = Runtime::new(
        host,
        Edge {
            updates,
            links,
            trace: vec![],
            fail,
        },
    );
    let session = Session::new(Arc::new(program)).unwrap();
    runtime
        .execute(vec![
            Step::Install {
                id: 1,
                instance: Box::new(Instance {
                    session,
                    inputs: Inputs {
                        participant: 7,
                        vehicle: None,
                        others: vec![],
                        road: None,
                        stats: None,
                        now: 0.,
                        time_scale: 1.,
                    },
                }),
            },
            Step::Graph {
                id: 1,
                action: Action::Start,
            },
        ])
        .unwrap();
    runtime
}
fn tick() -> Vec<Step> {
    vec![Step::Graph {
        id: 1,
        action: Action::Graph(Operation::Update { node: 0 }),
    }]
}
#[test]
fn root_updates_keep_registration_order_repeats_and_synchronous_nested_callbacks() {
    let mut runtime = fixture(false);
    runtime.execute(tick()).unwrap();
    assert_eq!(runtime.driver().trace, [2, 104, 1, 2, 104, 3, 0]);
    assert!(runtime.driver().updates.pending.is_empty());
    runtime.execute(tick()).unwrap();
    assert_eq!(runtime.driver().trace.len(), 14);
}
#[test]
fn failed_registered_transition_rolls_back_root_progress_and_all_preceding_callbacks() {
    let mut runtime = fixture(true);
    for _ in 0..2 {
        assert_eq!(
            runtime.execute(tick()).unwrap_err(),
            Error::UnsupportedEvent
        );
        assert!(runtime.driver().trace.is_empty());
        assert!(runtime.driver().updates.pending.is_empty());
        assert_eq!(runtime.driver().updates.next, 0);
    }
}
