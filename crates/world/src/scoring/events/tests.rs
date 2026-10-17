// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn endpoint(node: u16, event: u32) -> Endpoint {
    Endpoint { node, event }
}
fn edge(source: Endpoint, target: Endpoint, delivery: Delivery) -> Connection {
    Connection {
        source,
        target,
        delivery,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    score: u32,
    awarded: u32,
    trace: Vec<Endpoint>,
}
impl Handler for State {
    fn handle(&mut self, target: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        self.trace.push(target);
        match target.event {
            1 => {
                dispatch.emit(self, endpoint(target.node, 100))?;
                // This change must occur after the nested award, not before a
                // deferred queue eventually reaches that award.
                self.score = 0;
            }
            2 => self.awarded += self.score,
            3 => self.score += 1,
            4 => dispatch.emit(self, endpoint(0, 200))?,
            _ => return Err(Error::UnsupportedEvent),
        }
        Ok(())
    }
}

#[test]
fn nested_award_precedes_reset_and_authored_siblings_keep_their_order() {
    let root = endpoint(0, 200);
    let graph = Graph::new(
        4,
        vec![
            edge(root, endpoint(1, 1), Delivery::Local),
            edge(endpoint(1, 100), endpoint(2, 2), Delivery::Local),
            edge(root, endpoint(3, 3), Delivery::Local),
            edge(root, endpoint(3, 3), Delivery::Local),
        ],
    )
    .unwrap();
    let first = State {
        score: 40,
        ..State::default()
    };
    let second = State {
        score: 7,
        ..State::default()
    };
    for (state, credit) in [(&first, 40), (&second, 7)] {
        let (next, remote) = graph.apply(state, root).unwrap();
        assert_eq!((next.score, next.awarded), (2, credit));
        assert_eq!(
            next.trace,
            vec![
                endpoint(1, 1),
                endpoint(2, 2),
                endpoint(3, 3),
                endpoint(3, 3)
            ]
        );
        assert!(remote.is_empty());
        assert_eq!(state.awarded, 0);
        assert!(state.trace.is_empty());
    }
}

#[test]
fn remote_only_skips_local_handler_and_nested_remote_precedes_outer_remote() {
    let root = endpoint(0, 200);
    let nested = endpoint(1, 100);
    let outer = Remote { bus: 0, index: 8 };
    let inner = Remote { bus: 1, index: 2 };
    let both = Remote { bus: 0, index: 12 };
    let graph = Graph::new(
        3,
        vec![
            edge(root, endpoint(2, 999), Delivery::NetworkedClient(outer)),
            edge(root, endpoint(1, 1), Delivery::Local),
            edge(nested, endpoint(2, 999), Delivery::NetworkedClient(inner)),
            edge(
                root,
                endpoint(2, 3),
                Delivery::NetworkedClientAndServer(both),
            ),
        ],
    )
    .unwrap();
    let (state, output) = graph.apply(&State::default(), root).unwrap();
    assert_eq!(state.trace, vec![endpoint(1, 1), endpoint(2, 3)]);
    assert_eq!(
        output,
        vec![
            RemoteEmission {
                source: nested,
                connections: vec![inner],
                context: None,
            },
            RemoteEmission {
                source: root,
                connections: vec![outer, both],
                context: None,
            },
        ]
    );
}

#[test]
fn cycles_bounds_unknown_targets_and_handler_failure_discard_the_candidate() {
    let root = endpoint(0, 200);
    let state = State {
        score: 5,
        ..State::default()
    };
    let cycle = Graph::new(1, vec![edge(root, endpoint(0, 4), Delivery::Local)]).unwrap();
    assert_eq!(cycle.apply(&state, root), Err(Error::Bound));
    assert_eq!(state.score, 5);
    assert!(state.trace.is_empty());
    let invalid = Graph::new(
        1,
        vec![
            edge(root, endpoint(0, 3), Delivery::Local),
            edge(root, endpoint(0, 999), Delivery::Local),
        ],
    )
    .unwrap();
    assert_eq!(invalid.apply(&state, root), Err(Error::UnsupportedEvent));
    assert_eq!(
        invalid.apply(&state, endpoint(1, 0)),
        Err(Error::UnknownNode)
    );
    assert!(Graph::new(0, vec![]).is_err());
    assert!(Graph::new(MAX_NODES + 1, vec![]).is_err());
    assert_eq!(
        Graph::new(1, vec![edge(root, endpoint(1, 0), Delivery::Local)]),
        Err(Error::UnknownNode)
    );
    let repeated = edge(
        root,
        root,
        Delivery::NetworkedClient(Remote { bus: 0, index: 1 }),
    );
    assert_eq!(Graph::new(1, vec![repeated; 2]), Err(Error::InvalidGraph));
    assert_eq!(
        Graph::new(1, vec![repeated; MAX_CONNECTIONS + 1]),
        Err(Error::Bound)
    );
    // Unbounded recursion (event 4 re-emits the root) exhausts the shared
    // depth/work budget and discards the candidate.
    let recursive = Graph::new(2, vec![edge(root, endpoint(1, 4), Delivery::Local)]).unwrap();
    assert_eq!(recursive.apply(&state, root), Err(Error::Bound));
    let remote = Graph::new(
        1,
        (0..=MAX_REMOTE)
            .map(|index| {
                edge(
                    root,
                    root,
                    Delivery::NetworkedClient(Remote {
                        bus: 0,
                        index: index as u16,
                    }),
                )
            })
            .collect(),
    )
    .unwrap();
    assert_eq!(remote.apply(&state, root), Err(Error::Bound));
    assert_eq!(
        state,
        State {
            score: 5,
            ..State::default()
        }
    );
}

#[test]
fn a_nested_error_cannot_be_hidden_by_a_handler() {
    #[derive(Clone)]
    struct Ignoring;
    impl Handler for Ignoring {
        fn handle(&mut self, _: Endpoint, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
            let _ = dispatch.emit(self, endpoint(4, 0));
            Ok(())
        }
    }
    let root = endpoint(0, 200);
    let graph = Graph::new(1, vec![edge(root, endpoint(0, 1), Delivery::Local)]).unwrap();
    assert!(matches!(
        graph.apply(&Ignoring, root),
        Err(Error::UnknownNode)
    ));
}
