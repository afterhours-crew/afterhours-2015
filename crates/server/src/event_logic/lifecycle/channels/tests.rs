// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use std::collections::VecDeque;

#[derive(Clone)]
struct Driver {
    router: Router,
    fail: bool,
}
impl super::super::graphs::Driver for Driver {
    fn invoke(&mut self, request: Request, reader: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if self.fail && request.instance == 3 && matches!(request.call, Call::Property { .. }) {
            return Err(Error::UnsupportedEvent);
        }
        self.router
            .invoke(&request, reader)?
            .ok_or(Error::UnknownNode)
    }
    fn resume(
        &mut self,
        owner: u64,
        token: u64,
        reader: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        self.router
            .resume(owner, token, reader)?
            .ok_or(Error::UnknownNode)
    }
    fn effect(
        &mut self,
        _: &super::super::Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        Ok(vec![])
    }
}
fn instance(initial: Option<Value>) -> super::super::graphs::Instance {
    use nfs_world::scoring::{
        interpreter::{Inputs, Session},
        program::{Behavior, Identity, Node, Program, Step as Startup},
        properties::{Definition, Kind, Registration},
    };
    let port = Port { node: 0, field: 11 };
    super::super::graphs::Instance {
        session: Session::new(std::sync::Arc::new(Program {
            nodes: vec![Node {
                identity: Identity {
                    asset: "fixture".into(),
                    index: 0,
                    instances: vec![],
                },
                class: "LinkedChannel".into(),
                created: true,
                events: true,
                properties: true,
                forward: None,
                defaults: vec![],
                behavior: Behavior::Native,
            }],
            cells: vec![Definition {
                kind: Kind::Integer,
                initial,
                ports: vec![port],
                listeners: vec![],
            }],
            connections: vec![],
            startup: vec![
                Startup::Register(Registration::Notify(port)),
                Startup::Bind(0),
                Startup::Activate(0),
            ],
            unmodeled: vec![],
        }))
        .unwrap(),
        inputs: Inputs {
            participant: 7,
            vehicle: None,
            others: vec![],
            road: None,
            stats: None,
            now: 0.,
            time_scale: 1.,
        },
    }
}

#[test]
fn real_graph_channel_callbacks_and_continuations_commit_together_or_roll_back() {
    use super::super::{Host, SceneInputs, graphs::Runtime, tests::ranked};
    for fail in [false, true] {
        let mut router = Router::new(91);
        router.definition(location(1), vec![11]).unwrap();
        for id in [2, 3] {
            router
                .reference(location(id), location(1), BTreeSet::from([11]))
                .unwrap();
        }
        let mut runtime = Runtime::new(
            Host::prepare(ranked(), SceneInputs::default()).unwrap(),
            Driver { router, fail },
        );
        let mut setup = Vec::new();
        for id in 1..=3 {
            setup.push(Step::Install {
                id,
                instance: Box::new(instance(if id == 1 {
                    Some(Value::Integer(5))
                } else {
                    None
                })),
            });
            setup.push(Step::Graph {
                id,
                action: Action::Start,
            });
        }
        // With fail enabled the last reference rejects its seed. The first
        // reference, both activations and all installations must roll back.
        let result = runtime.execute(setup);
        if fail {
            assert_eq!(result.unwrap_err(), Error::UnsupportedEvent);
            for id in 1..=3 {
                assert!(runtime.instance(id).is_none());
            }
            assert!(runtime.driver().router.pending.is_empty());
            assert!(
                runtime
                    .driver()
                    .router
                    .references
                    .values()
                    .all(|r| !r.active)
            );
            continue;
        }
        result.unwrap();
        for id in [2, 3] {
            assert_eq!(
                runtime
                    .instance(id)
                    .unwrap()
                    .session
                    .read(Port { node: 0, field: 11 })
                    .unwrap(),
                Some(Value::Integer(5))
            );
        }
        runtime
            .execute(vec![location(1).publish(11, Value::Integer(9))])
            .unwrap();
        for id in 1..=3 {
            assert_eq!(
                runtime
                    .instance(id)
                    .unwrap()
                    .session
                    .read(Port { node: 0, field: 11 })
                    .unwrap(),
                Some(Value::Integer(9))
            );
        }
        runtime
            .execute(vec![
                Step::Remove { id: 2 },
                location(1).publish(11, Value::Integer(12)),
            ])
            .unwrap();
        assert!(runtime.instance(2).is_none());
        assert_eq!(
            runtime
                .instance(3)
                .unwrap()
                .session
                .read(Port { node: 0, field: 11 })
                .unwrap(),
            Some(Value::Integer(12))
        );
        assert_eq!(runtime.driver().router.definitions[&location(1)].busy, 0);
        assert!(runtime.driver().router.pending.is_empty());
    }
}

fn location(instance: u64) -> Location {
    Location { instance, node: 0 }
}
fn request(instance: u64, call: Call) -> Request {
    Request {
        instance,
        node: 0,
        call,
    }
}
#[derive(Default)]
struct Values(BTreeMap<(u64, u32), Value>);
impl Reader for Values {
    fn read(&mut self, instance: u64, port: Port) -> Result<Option<Value>, Error> {
        Ok(self.0.get(&(instance, port.field)).cloned())
    }
}
fn drain(
    router: &mut Router,
    values: &mut Values,
    steps: Vec<Step>,
    mut callback: impl FnMut(&mut Router, &mut Values, u64, &Operation) -> Vec<Step>,
) -> Vec<(u64, Operation)> {
    let mut pending = VecDeque::from(steps);
    let mut output = Vec::new();
    while let Some(step) = pending.pop_front() {
        let next = match step {
            Step::Continue { owner, token } => {
                router.resume(owner, token, values).unwrap().unwrap()
            }
            Step::Graph {
                id,
                action: Action::Graph(operation),
            } => {
                let next = callback(router, values, id, &operation);
                output.push((id, operation));
                next
            }
            _ => panic!("unexpected channel step"),
        };
        for step in next.into_iter().rev() {
            pending.push_front(step);
        }
    }
    output
}

#[test]
fn activation_seeds_only_ready_bound_values_in_native_order_and_resamples_after_each_callback() {
    let mut router = Router::new(91);
    let mut values = Values::default();
    router
        .definition(location(1), vec![11, 12, 13, 14])
        .unwrap();
    router
        .reference(location(2), location(1), BTreeSet::from([11, 12, 13]))
        .unwrap();
    values.0.insert((1, 11), Value::Integer(5));
    values.0.insert((1, 13), Value::Integer(9));
    values.0.insert((1, 14), Value::Integer(99));
    let steps = router
        .invoke(&request(2, Call::Activate), &mut values)
        .unwrap()
        .unwrap();
    let out = drain(
        &mut router,
        &mut values,
        steps,
        |_, values, _, operation| {
            if matches!(
                operation,
                Operation::Publish {
                    port: Port { field: 11, .. },
                    ..
                }
            ) {
                values.0.insert((1, 13), Value::Integer(20));
            }
            vec![]
        },
    );
    assert_eq!(
        out,
        [
            (
                2,
                Operation::Publish {
                    port: Port { node: 0, field: 11 },
                    value: Value::Integer(5)
                }
            ),
            (
                2,
                Operation::Publish {
                    port: Port { node: 0, field: 13 },
                    value: Value::Integer(20)
                }
            ),
        ]
    );
    assert!(
        router
            .invoke(&request(2, Call::Activate), &mut values)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    // Notifications are forwarded even when the scalar value repeats.
    for _ in 0..2 {
        let steps = router
            .invoke(
                &request(
                    1,
                    Call::Property {
                        field: 11,
                        first: false,
                    },
                ),
                &mut values,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            drain(&mut router, &mut values, steps, |_, _, _, _| vec![]).len(),
            1
        );
    }
}

#[test]
fn event_callbacks_observe_unsubscription_tail_compaction_and_new_subscriptions() {
    let mut router = Router::new(91);
    let mut values = Values::default();
    router.definition(location(1), vec![]).unwrap();
    for id in 2..=5 {
        router
            .reference(location(id), location(1), BTreeSet::new())
            .unwrap();
        let steps = router
            .invoke(&request(id, Call::Activate), &mut values)
            .unwrap()
            .unwrap();
        drain(&mut router, &mut values, steps, |_, _, _, _| vec![]);
    }
    let steps = router
        .invoke(
            &request(
                1,
                Call::Event {
                    event: 77,
                    context: Some(Context::Participant(8)),
                },
            ),
            &mut values,
        )
        .unwrap()
        .unwrap();
    let out = drain(&mut router, &mut values, steps, |router, values, id, _| {
        if id != 2 {
            return vec![];
        }
        router.invoke(&request(3, Call::Retire), values).unwrap();
        router
            .reference(location(6), location(1), BTreeSet::new())
            .unwrap();
        router
            .invoke(&request(6, Call::Activate), values)
            .unwrap()
            .unwrap()
    });
    assert_eq!(
        out.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [2, 6, 4, 5]
    );
    assert!(out.iter().all(|(_, op)| matches!(
        op,
        Operation::Emit {
            source: Endpoint { event: 77, .. },
            context: Some(Context::Participant(8))
        }
    )));
    assert_eq!(router.definitions[&location(1)].busy, 0);
    assert!(router.pending.is_empty());
}

#[test]
fn channel_scope_and_retirement_do_not_cross_event_instances() {
    let mut router = Router::new(91);
    let mut values = Values::default();
    for source in [1, 10] {
        router.definition(location(source), vec![11]).unwrap();
    }
    router
        .reference(location(2), location(1), BTreeSet::from([11]))
        .unwrap();
    router
        .reference(location(20), location(10), BTreeSet::from([11]))
        .unwrap();
    for id in [2, 20] {
        let steps = router
            .invoke(&request(id, Call::Activate), &mut values)
            .unwrap()
            .unwrap();
        drain(&mut router, &mut values, steps, |_, _, _, _| vec![]);
    }
    assert!(
        router
            .invoke(&request(1, Call::Retire), &mut values)
            .is_err()
    );
    values.0.insert((1, 11), Value::Integer(19));
    let steps = router
        .invoke(
            &request(
                1,
                Call::Property {
                    field: 11,
                    first: true,
                },
            ),
            &mut values,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        drain(&mut router, &mut values, steps, |_, _, _, _| vec![])
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [2]
    );
    router
        .invoke(&request(2, Call::Retire), &mut values)
        .unwrap();
    router
        .invoke(&request(1, Call::Retire), &mut values)
        .unwrap();
    assert!(router.definitions.contains_key(&location(10)));
    assert!(router.references.contains_key(&location(20)));
    assert!(router.resume(92, 1, &mut values).unwrap().is_none());
    assert!(router.resume(91, 1, &mut values).is_err());
}

#[test]
fn nested_channel_dispatch_clears_native_iteration_flag_before_outer_continues() {
    let mut router = Router::new(91);
    let mut values = Values::default();
    router.definition(location(1), vec![]).unwrap();
    for id in 2..=105 {
        router
            .reference(location(id), location(1), BTreeSet::new())
            .unwrap();
        let steps = router
            .invoke(&request(id, Call::Activate), &mut values)
            .unwrap()
            .unwrap();
        drain(&mut router, &mut values, steps, |_, _, _, _| vec![]);
    }
    let steps = router
        .invoke(
            &request(
                1,
                Call::Event {
                    event: 77,
                    context: None,
                },
            ),
            &mut values,
        )
        .unwrap()
        .unwrap();
    let output = drain(&mut router, &mut values, steps, |router, values, id, _| {
        if id == 2 {
            let nested = router
                .invoke(
                    &request(
                        1,
                        Call::Event {
                            event: 78,
                            context: None,
                        },
                    ),
                    values,
                )
                .unwrap()
                .unwrap();
            let nested = drain(router, values, nested, |_, _, _, _| vec![]);
            assert_eq!(nested.len(), 104);
            // Native nested dispatch clears a Boolean, not a depth counter.
            // The 101st removal therefore compacts even while outer is pending.
            // Moving 105 into the already-consumed slot zero skips its outer call.
            for id in 2..=102 {
                router.invoke(&request(id, Call::Retire), values).unwrap();
            }
        }
        vec![]
    });
    assert_eq!(
        output.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [2, 104, 103]
    );
    assert_eq!(router.definitions[&location(1)].busy, 0);
    assert!(router.pending.is_empty());
}
