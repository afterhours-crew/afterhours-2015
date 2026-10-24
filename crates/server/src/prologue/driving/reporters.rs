// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The first-drive reporter paths use prefab forwarding, event gates and typed
//! participant transitions. Reuse their core models; any other reached class
//! or remote contextual emission is an error, rather than a dropped callback.
use super::*;
use nfs_world::scoring::{
    self,
    events::{self, Dispatch, Endpoint, Handler},
    nodes, transitions,
};

#[derive(Clone, Debug)]
struct HandlerState {
    forward: BTreeMap<u16, u16>,
    gates: BTreeMap<u16, nodes::Node>,
    transitions: BTreeMap<u16, (Vec<u16>, Option<u16>)>,
    actions: Vec<Action>,
    ai_only: BTreeSet<u16>,
    human: Option<u64>,
}
impl HandlerState {
    fn receive(
        &mut self,
        target: Endpoint,
        participant: Option<u64>,
        d: &mut Dispatch<'_>,
    ) -> Result<(), scoring::Error> {
        // The mode's human membership callbacks do not enable/disable AI-only
        // racers. Their population owner supplies their separate callbacks.
        if self.ai_only.contains(&target.node) && participant.is_some() && participant == self.human
        {
            return Ok(());
        }
        if let Some(&node) = self.forward.get(&target.node) {
            return match participant {
                Some(p) => d.emit_participant(
                    self,
                    Endpoint {
                        node,
                        event: target.event,
                    },
                    p,
                ),
                None => d.emit(
                    self,
                    Endpoint {
                        node,
                        event: target.event,
                    },
                ),
            };
        }
        if let Some(gate) = self.gates.get_mut(&target.node) {
            let effects = gate.apply(nodes::Action::Event(target.event), |_| None)?;
            for effect in effects {
                let nodes::Effect::Emit(event) = effect else {
                    return Err(scoring::Error::UnsupportedEvent);
                };
                match participant {
                    Some(p) => d.emit_participant(
                        self,
                        Endpoint {
                            node: target.node,
                            event,
                        },
                        p,
                    )?,
                    None => d.emit(
                        self,
                        Endpoint {
                            node: target.node,
                            event,
                        },
                    )?,
                };
            }
            return Ok(());
        }
        if let Some((sources, target_state)) = self.transitions.get(&target.node) {
            if self.actions.len() >= 256 {
                return Err(scoring::Error::Bound);
            };
            self.actions.push(match (target.event, participant) {
                (transitions::TRIGGER, Some(participant)) => Action::MoveFirstCompleted {
                    sources: sources.clone(),
                    participant,
                    target: *target_state,
                },
                (transitions::TRIGGER_ALL, _) => Action::MoveAll {
                    sources: sources.clone(),
                    target: *target_state,
                },
                _ => return Err(scoring::Error::UnsupportedEvent),
            });
            return Ok(());
        }
        Err(scoring::Error::UnsupportedEvent)
    }
}
impl Handler for HandlerState {
    fn handle(&mut self, target: Endpoint, d: &mut Dispatch<'_>) -> Result<(), scoring::Error> {
        self.receive(target, None, d)
    }
    fn handle_participant(
        &mut self,
        target: Endpoint,
        participant: u64,
        d: &mut Dispatch<'_>,
    ) -> Result<(), scoring::Error> {
        self.receive(target, Some(participant), d)
    }
}
#[derive(Clone, Debug)]
pub(super) struct Reporters {
    graph: events::Graph,
    identities: BTreeMap<Identity, u16>,
    countdowns: BTreeMap<u16, u16>,
    state: HandlerState,
}
impl Reporters {
    pub(super) fn prepare(
        index: &mut AssetIndex,
        data: &intro::Mode,
        plan: &Plan,
    ) -> Result<Self, nfs_content::Error> {
        let composition = index.scoring_events(&data.game_mode.layout.blueprint)?;
        let serializers =
            index.event_serializer_identities(&data.game_mode.layout, "GameModeEntityData")?;
        let ids = composition
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.identity.clone(), i as u16))
            .collect::<BTreeMap<_, _>>();
        let mut gates = BTreeMap::new();
        let ai_only = index
            .scoring_ai_inclusions(&data.game_mode.layout.blueprint)?
            .into_iter()
            .filter_map(|(identity, inclusion)| {
                (inclusion == nfs_world::scoring::ai_control::Inclusion::Ai).then(|| ids[&identity])
            })
            .collect();
        for node in index.scoring_nodes(&data.game_mode.layout.blueprint)? {
            if matches!(
                node.configuration.definition,
                nodes::Definition::Gate { .. }
            ) {
                gates.insert(
                    ids[&node.identity],
                    nodes::Node::new(node.configuration.definition)
                        .map_err(|e| nfs_content::Error::Content(e.to_string()))?,
                );
            }
        }
        let transitions = plan
            .data()
            .transitions
            .iter()
            .filter(|t| t.kind == nfs_content::events::lifecycle::TransitionKind::Triggered)
            .map(|t| {
                (
                    ids[&serializers[t.serializer]],
                    (
                        t.sources.iter().map(|s| s.serializer as u16).collect(),
                        t.target.map(|t| t.serializer as u16),
                    ),
                )
            })
            .collect();
        Ok(Self {
            graph: composition.dispatcher()?,
            countdowns: data
                .game_mode
                .lifecycle
                .states
                .iter()
                .filter(|s| s.kind == StateKind::Countdown)
                .map(|s| (s.serializer as u16, ids[&serializers[s.serializer]]))
                .collect(),
            identities: ids,
            state: HandlerState {
                forward: composition
                    .nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| n.forward.map(|f| (i as u16, f)))
                    .collect(),
                gates,
                transitions,
                actions: vec![],
                ai_only,
                human: None,
            },
        })
    }
    pub(super) fn bind_human(&mut self, participant: u16) {
        self.state.human = Some(u64::from(participant));
    }
    pub(super) fn effect(
        &mut self,
        e: &Effect,
    ) -> Result<Vec<Action>, nfs_world::participant_states::Error> {
        let (node, event, participant) = match e {
            Effect::Reporter(event_logic::lifecycle::reporters::Effect::Event {
                identity,
                event,
                participant,
            })
            | Effect::Ai(event_logic::lifecycle::ai::Effect::Event {
                identity,
                event,
                participant,
            }) => (
                *self
                    .identities
                    .get(identity)
                    .ok_or(nfs_world::participant_states::Error::Invalid)?,
                *event,
                Some(*participant),
            ),
            Effect::Timing(event_logic::lifecycle::timed::Effect::CountdownEvent {
                state,
                event,
            }) => (
                *self
                    .countdowns
                    .get(state)
                    .ok_or(nfs_world::participant_states::Error::Invalid)?,
                *event,
                None,
            ),
            _ => return Ok(vec![]),
        };
        let (mut next, remote) = self
            .graph
            .transaction(&self.state, |s, d| {
                let source = Endpoint { node, event };
                match participant {
                    Some(p) => d.emit_participant(s, source, p),
                    None => d.emit(s, source),
                }
            })
            .map_err(|_| nfs_world::participant_states::Error::Invalid)?;
        if !remote.is_empty() {
            return Err(nfs_world::participant_states::Error::Invalid);
        };
        let actions = std::mem::take(&mut next.actions);
        self.state = next;
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(fail: bool) -> Reporters {
        let id = Identity {
            asset: "test".into(),
            index: 0,
            instances: vec![],
        };
        let mut connections = vec![];
        let mut edge = |source, event, target, input| {
            connections.push(events::Connection {
                source: Endpoint {
                    node: source,
                    event,
                },
                target: Endpoint {
                    node: target,
                    event: input,
                },
                delivery: events::Delivery::Local,
            })
        };
        edge(0, 41, 1, nodes::IN);
        edge(1, nodes::OUT, 1, nodes::CLOSE);
        edge(1, nodes::OUT, 2, transitions::TRIGGER);
        if fail {
            edge(1, nodes::OUT, 3, nodes::IN)
        };
        Reporters {
            graph: events::Graph::new(4, connections).unwrap(),
            identities: BTreeMap::from([(id, 0)]),
            countdowns: BTreeMap::new(),
            state: HandlerState {
                forward: BTreeMap::new(),
                gates: BTreeMap::from([(
                    1,
                    nodes::Node::new(nodes::Definition::Gate { open: true }).unwrap(),
                )]),
                transitions: BTreeMap::from([(2, (vec![7], Some(8)))]),
                actions: vec![],
                ai_only: BTreeSet::new(),
                human: None,
            },
        }
    }
    fn effect() -> Effect {
        Effect::Reporter(event_logic::lifecycle::reporters::Effect::Event {
            identity: Identity {
                asset: "test".into(),
                index: 0,
                instances: vec![],
            },
            event: 41,
            participant: 19,
        })
    }
    #[test]
    fn reporter_gate_preserves_participant_and_closes_before_the_next_event() {
        let mut r = fixture(false);
        assert_eq!(
            r.effect(&effect()).unwrap(),
            vec![Action::MoveFirstCompleted {
                sources: vec![7],
                participant: 19,
                target: Some(8)
            }]
        );
        assert!(r.effect(&effect()).unwrap().is_empty());
    }
    #[test]
    fn unsupported_sink_rolls_back_both_gate_and_queued_transition() {
        let mut r = fixture(true);
        assert!(r.effect(&effect()).is_err());
        assert!(r.state.actions.is_empty());
        r.graph = fixture(false).graph;
        assert_eq!(r.effect(&effect()).unwrap().len(), 1);
    }

    #[test]
    fn ai_only_filter_excludes_only_the_bound_human_participant() {
        let mut r = fixture(true);
        r.state.ai_only.insert(3);
        r.bind_human(19);
        assert_eq!(r.effect(&effect()).unwrap().len(), 1);
        let mut foreign = fixture(true);
        foreign.state.ai_only.insert(3);
        foreign.bind_human(20);
        assert!(foreign.effect(&effect()).is_err());
        assert!(foreign.state.actions.is_empty());
    }
}
