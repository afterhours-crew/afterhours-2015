// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn at(node: u16) -> Endpoint {
    Endpoint { node, event: 7 }
}
fn edge(source: u16, target: u16, delivery: Delivery) -> Connection {
    Connection {
        source: at(source),
        target: at(target),
        delivery,
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    log: Vec<(u16, Option<EntityContext>)>,
    fail: bool,
    cycle: bool,
    swallow: bool,
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
        self.log.push((target.node, None));
        Ok(())
    }
    fn handle_context(
        &mut self,
        target: Endpoint,
        context: EntityContext,
        d: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        self.log.push((target.node, Some(context)));
        if target.node == 1 {
            // Forwarded events retain context. An independent new scalar event
            // emitted during that traversal does not inherit it.
            d.emit(self, at(3))?;
            let result = d.emit_context(self, if self.cycle { at(0) } else { at(1) }, context);
            if !self.swallow {
                result?;
            }
        }
        if self.fail && target.node == 2 {
            return Err(Error::UnsupportedEvent);
        }
        Ok(())
    }
}
fn graph() -> Graph {
    Graph::new(
        5,
        vec![
            edge(0, 1, Delivery::Local),
            edge(
                0,
                2,
                Delivery::NetworkedClientAndServer(Remote { bus: 0, index: 1 }),
            ),
            edge(1, 2, Delivery::Local),
            edge(1, 2, Delivery::NetworkedClient(Remote { bus: 0, index: 2 })),
            edge(3, 4, Delivery::Local),
            edge(3, 4, Delivery::NetworkedClient(Remote { bus: 0, index: 3 })),
        ],
    )
    .unwrap()
}
#[test]
fn explicit_forwarding_preserves_context_for_each_sibling_and_remote_row() {
    for object in [None, Some(7), Some(8191)] {
        let context = EntityContext { object };
        let original = State::default();
        let (next, remote) = graph()
            .transaction(&original, |s, d| d.emit_context(s, at(0), context))
            .unwrap();
        assert_eq!(
            next.log,
            vec![
                (1, Some(context)),
                (4, None),
                (2, Some(context)),
                (2, Some(context))
            ]
        );
        assert_eq!(
            remote
                .iter()
                .map(|e| (e.source.node, e.context))
                .collect::<Vec<_>>(),
            vec![
                (3, None),
                (1, Some(context.into())),
                (0, Some(context.into()))
            ]
        );
        assert!(original.log.is_empty());
    }
}
#[test]
fn contextual_faults_cycles_and_swallowed_nested_errors_roll_back() {
    for state in [
        State {
            fail: true,
            ..State::default()
        },
        State {
            cycle: true,
            ..State::default()
        },
        State {
            cycle: true,
            swallow: true,
            ..State::default()
        },
        State {
            fail: true,
            swallow: true,
            ..State::default()
        },
    ] {
        let before = state.clone();
        let result = graph().transaction(&state, |s, d| {
            d.emit_context(s, at(0), EntityContext { object: Some(23) })
        });
        assert_eq!(
            result,
            Err(if state.cycle {
                Error::Bound
            } else {
                Error::UnsupportedEvent
            })
        );
        assert_eq!(state, before);
    }
    for object in [0, 8192, u16::MAX] {
        assert_eq!(
            graph().transaction(&State::default(), |s, d| {
                let _ = d.emit_context(
                    s,
                    at(0),
                    EntityContext {
                        object: Some(object),
                    },
                );
                Ok(())
            }),
            Err(Error::InvalidGraph)
        );
    }
}
#[test]
fn legacy_handlers_reject_even_null_entity_context_instead_of_dropping_it() {
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Legacy(usize);
    impl Handler for Legacy {
        fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
            self.0 += 1;
            Ok(())
        }
    }
    let graph = Graph::new(2, vec![edge(0, 1, Delivery::Local)]).unwrap();
    let state = Legacy(0);
    assert_eq!(graph.apply(&state, at(0)).unwrap().0, Legacy(1));
    assert_eq!(
        graph.transaction(&state, |s, d| d.emit_context(
            s,
            at(0),
            EntityContext { object: None }
        )),
        Err(Error::UnsupportedEvent)
    );
    assert_eq!(state, Legacy(0));
    assert_eq!(
        graph.transaction(&state, |s, d| d.emit_integer(s, at(0), 0)),
        Err(Error::UnsupportedEvent)
    );
}

#[test]
fn signed_integer_context_is_preserved_through_forwarding_and_remote_emissions() {
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct Integers(Vec<i32>);
    impl Handler for Integers {
        fn handle(&mut self, _: Endpoint, _: &mut Dispatch<'_>) -> Result<(), Error> {
            Err(Error::UnsupportedEvent)
        }
        fn handle_integer(
            &mut self,
            target: Endpoint,
            value: i32,
            d: &mut Dispatch<'_>,
        ) -> Result<(), Error> {
            self.0.push(value);
            if target.node == 1 {
                d.emit_integer(self, at(1), value)?;
            }
            Ok(())
        }
    }
    let graph = Graph::new(
        3,
        vec![
            edge(0, 1, Delivery::Local),
            edge(
                1,
                2,
                Delivery::NetworkedClientAndServer(Remote { bus: 1, index: 0 }),
            ),
        ],
    )
    .unwrap();
    for value in [i32::MIN, -1, 0, 7, i32::MAX] {
        let original = Integers::default();
        let (next, output) = graph
            .transaction(&original, |s, d| d.emit_integer(s, at(0), value))
            .unwrap();
        assert_eq!(next.0, vec![value, value]);
        assert_eq!(output[0].context, Some(Context::Integer(value)));
        assert!(original.0.is_empty());
    }
}
