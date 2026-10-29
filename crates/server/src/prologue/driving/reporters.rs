// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Intro reporter paths use prefab forwarding, startup delays, event gates and
//! typed participant transitions. Reuse their core models; any other reached class
//! or remote contextual emission is an error, rather than a dropped callback.
use super::*;
use event_logic::lifecycle::commands::Command;
use nfs_world::scoring::{
    self, ai_control, delay,
    events::{self, Dispatch, Endpoint, Handler},
    nodes, transitions,
};
mod delays;

#[derive(Clone, Debug)]
struct HandlerState {
    forward: BTreeMap<u16, u16>,
    gates: BTreeMap<u16, nodes::Node>,
    transitions: BTreeMap<u16, (Vec<u16>, Option<u16>)>,
    actions: Vec<Command>,
    results: BTreeMap<u16, Identity>,
    ai_only: BTreeSet<u16>,
    ai_controls: BTreeSet<u16>,
    controls: BTreeMap<u64, ai_control::Stack>,
    human: Option<u64>,
    owned_filters: BTreeSet<u16>,
    delay_values: BTreeMap<u16, f32>,
    delay_manager: delay::Manager<Option<u64>>,
    diagnostics: BTreeSet<u16>,
}
impl HandlerState {
    fn receive(
        &mut self,
        target: Endpoint,
        participant: Option<u64>,
        d: &mut Dispatch<'_>,
    ) -> Result<(), scoring::Error> {
        if self.owned_filters.contains(&target.node) {
            if let Some(participant) = participant.filter(|p| Some(*p) == self.human) {
                d.emit_participant(
                    self,
                    Endpoint {
                        node: target.node,
                        event: nodes::OUT,
                    },
                    participant,
                )?;
            }
            return Ok(());
        }
        if self.delay_values.contains_key(&target.node) {
            return d.delay_event(self, target.node, target.event, participant);
        }
        if self.diagnostics.contains(&target.node) && target.event == 231_942_004 {
            return Ok(());
        }
        // The mode's human membership callbacks do not enable/disable AI-only
        // racers. Their population owner supplies their separate callbacks.
        if self.ai_only.contains(&target.node) && participant.is_some() && participant == self.human
        {
            return Ok(());
        }
        if self.ai_controls.contains(&target.node) {
            let participant = participant.ok_or(scoring::Error::MissingInput)?;
            if self.controls.len() >= ai_control::MAX_OVERRIDES
                && !self.controls.contains_key(&participant)
            {
                return Err(scoring::Error::Bound);
            }
            let stack = self.controls.entry(participant).or_default();
            let key = ai_control::Key {
                instance: 1,
                node: target.node,
            };
            let event = match target.event {
                ai_control::ENABLE | ai_control::DISABLE => {
                    stack.acquire(key)?;
                    let enabled = target.event == ai_control::ENABLE;
                    stack.set(key, enabled)?;
                    Some(if enabled {
                        ai_control::ENABLED
                    } else {
                        ai_control::DISABLED
                    })
                }
                ai_control::RELEASE => {
                    stack.release(key);
                    None
                }
                _ => return Err(scoring::Error::UnsupportedEvent),
            };
            if let Some(event) = event {
                d.emit_participant(
                    self,
                    Endpoint {
                        node: target.node,
                        event,
                    },
                    participant,
                )?;
            }
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
            self.actions
                .push(Command::State(match (target.event, participant) {
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
                }));
            return Ok(());
        }
        if let Some(identity) = self.results.get(&target.node) {
            if self.actions.len() >= 256 {
                return Err(scoring::Error::Bound);
            }
            self.actions.push(Command::Results {
                identity: identity.clone(),
                event: target.event,
                context: participant.map(events::Context::Participant),
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
/// Immutable compiler output only; no dispatch queues, clocks, gate state or players.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    composition: nfs_content::scoring::events::Composition,
    serializers: Vec<Identity>,
    delays: Vec<nfs_content::scoring::delay::Node>,
    ai_only: Vec<Identity>,
    gates: Vec<(Identity, nodes::Definition)>,
}
impl Source {
    pub(super) fn prepare(
        index: &mut AssetIndex,
        data: &intro::Mode,
    ) -> Result<Self, nfs_content::Error> {
        let name = &data.game_mode.layout.blueprint;
        Ok(Self {
            composition: index.scoring_events(name)?,
            serializers: index
                .event_serializer_identities(&data.game_mode.layout, "GameModeEntityData")?,
            delays: index.scoring_delays(name)?,
            ai_only: index
                .scoring_ai_inclusions(name)?
                .into_iter()
                .filter_map(|(id, inclusion)| {
                    (inclusion == ai_control::Inclusion::Ai).then_some(id)
                })
                .collect(),
            gates: index
                .scoring_nodes(name)?
                .into_iter()
                .filter_map(|node| {
                    matches!(
                        node.configuration.definition,
                        nodes::Definition::Gate { .. }
                    )
                    .then_some((node.identity, node.configuration.definition))
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Reporters {
    graph: events::Graph,
    identities: BTreeMap<Identity, u16>,
    countdowns: BTreeMap<u16, u16>,
    state: HandlerState,
    root: u16,
    started: bool,
    delay_clock: Option<f64>,
}
impl Reporters {
    pub(super) fn from_source(
        source: &Source,
        data: &intro::Mode,
        plan: &Plan,
    ) -> Result<Self, nfs_content::Error> {
        let composition = &source.composition;
        let serializers = &source.serializers;
        // Validate topology before indexing a cached endpoint, and never narrow
        // unchecked node counts or recover missing identities by guessing.
        let graph = composition.dispatcher()?;
        if serializers.len() != data.game_mode.layout.fields.len()
            || composition.nodes.len() > usize::from(u16::MAX)
            || composition.nodes.iter().any(|n| {
                n.forward
                    .is_some_and(|f| usize::from(f) >= composition.nodes.len())
            })
        {
            return Err(nfs_content::Error::Cache("intro reporter layout"));
        }
        let ids: BTreeMap<_, _> = composition
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.identity.clone(), i as u16))
            .collect();
        if ids.len() != composition.nodes.len() {
            return Err(nfs_content::Error::Cache(
                "duplicate intro reporter identity",
            ));
        }
        let id = |identity: &Identity| {
            ids.get(identity)
                .copied()
                .ok_or(nfs_content::Error::Cache("missing intro reporter identity"))
        };
        let serializer = |s: usize| {
            serializers
                .get(s)
                .ok_or(nfs_content::Error::Cache(
                    "missing intro reporter serializer",
                ))
                .and_then(id)
        };
        for identity in serializers {
            id(identity)?;
        }
        if source
            .gates
            .iter()
            .any(|(_, definition)| !matches!(definition, nodes::Definition::Gate { .. }))
        {
            return Err(nfs_content::Error::Cache("prepared intro gate definition"));
        }
        let (delay_values, delay_manager) = delays::from_source(&source.delays, composition, &ids)?;
        let roots: Vec<_> = composition
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.class == "GameModeEntityData")
            .map(|(i, _)| i as u16)
            .collect();
        let [root] = roots.as_slice() else {
            return Err(nfs_content::Error::Cache("intro reporter root"));
        };
        let ai_only = source
            .ai_only
            .iter()
            .map(id)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let gates = source
            .gates
            .iter()
            .map(|(identity, definition)| {
                Ok((
                    id(identity)?,
                    nodes::Node::new(*definition)
                        .map_err(|e| nfs_content::Error::Content(e.to_string()))?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, nfs_content::Error>>()?;
        let transitions = plan
            .data()
            .transitions
            .iter()
            .filter(|t| t.kind == nfs_content::events::lifecycle::TransitionKind::Triggered)
            .map(|t| {
                Ok((
                    serializer(t.serializer)?,
                    (
                        t.sources.iter().map(|s| s.serializer as u16).collect(),
                        t.target.map(|t| t.serializer as u16),
                    ),
                ))
            })
            .collect::<Result<_, nfs_content::Error>>()?;
        Ok(Self {
            root: *root,
            started: false,
            delay_clock: None,
            graph,
            countdowns: data
                .game_mode
                .lifecycle
                .states
                .iter()
                .filter(|s| s.kind == StateKind::Countdown)
                .map(|s| Ok((s.serializer as u16, serializer(s.serializer)?)))
                .collect::<Result<_, nfs_content::Error>>()?,
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
                results: plan
                    .data()
                    .bindings
                    .results
                    .iter()
                    .map(|r| Ok((id(&r.identity)?, r.identity.clone())))
                    .collect::<Result<_, nfs_content::Error>>()?,
                ai_controls: ai_only.clone(),
                ai_only,
                controls: BTreeMap::new(),
                human: None,
                owned_filters: data
                    .owned_filters
                    .iter()
                    .map(id)
                    .collect::<Result<_, _>>()?,
                delay_values,
                delay_manager,
                diagnostics: composition
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.class == "PrintDebugTextEntityData")
                    .map(|(i, _)| i as u16)
                    .collect(),
            },
            identities: ids,
        })
    }
    pub(super) fn bind_human(&mut self, participant: u16) {
        self.state.human = Some(u64::from(participant));
    }
    pub(super) fn controls(&self) -> impl Iterator<Item = (u64, bool)> + '_ {
        self.state.controls.iter().map(|(&p, s)| (p, s.current()))
    }
    pub(super) fn effect(
        &mut self,
        e: &Effect,
    ) -> Result<Vec<Command>, nfs_world::participant_states::Error> {
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
    pub(super) fn external(
        &mut self,
        source: &Identity,
        event: u32,
        participant: u16,
    ) -> Result<Vec<Command>, nfs_world::participant_states::Error> {
        if self.state.human != Some(participant.into()) {
            return Err(nfs_world::participant_states::Error::Invalid);
        }
        self.effect(&Effect::Reporter(
            event_logic::lifecycle::reporters::Effect::Event {
                identity: source.clone(),
                event,
                participant: participant.into(),
            },
        ))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    #[test]
    fn mode_snapshot_rebuilds_fresh_reporters_and_rejects_dangling_identities() {
        let (_, mut mode) = super::super::tests::content(
            Asset {
                bundle: 4,
                type_id: 8,
                local_index: 0,
            },
            nfs_world::sequences::Component {
                scene_key: 7,
                index: 2,
            },
        );
        let identities: Vec<_> = (0..mode.data.game_mode.layout.fields.len())
            .map(|index| Identity {
                asset: "constructed/mode".into(),
                index,
                instances: vec![],
            })
            .collect();
        let source = Source {
            composition: nfs_content::scoring::events::Composition {
                nodes: identities
                    .iter()
                    .enumerate()
                    .map(|(index, identity)| nfs_content::scoring::events::Node {
                        identity: identity.clone(),
                        class: if index == 0 {
                            "GameModeEntityData"
                        } else {
                            "ConstructedNode"
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
                links: vec![],
            },
            serializers: identities,
            delays: vec![],
            ai_only: vec![],
            gates: vec![],
        };
        let mut reporters = Reporters::from_source(&source, &mode.data, &mode.plan).unwrap();
        reporters.bind_human(88);
        reporters.start(1.).unwrap();
        assert!(reporters.started);
        mode.reporters = Some(reporters);
        mode.reporter_source = Some(Arc::new(source));
        let bytes = serde_json::to_vec(&mode).unwrap();
        let mut restored: Mode = serde_json::from_slice(&bytes).unwrap();
        let rebuilt = restored.reporters.as_mut().unwrap();
        assert!(!rebuilt.started);
        assert_eq!(rebuilt.delay_clock, None);
        assert_eq!(rebuilt.state.human, None);
        assert!(rebuilt.state.actions.is_empty() && rebuilt.state.controls.is_empty());
        rebuilt.bind_human(19);
        assert_eq!(mode.reporters.as_ref().unwrap().state.human, Some(88));
        let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        invalid["reporters"]["composition"]["nodes"][0]["identity"]["index"] =
            serde_json::json!(99999);
        assert!(serde_json::from_value::<Mode>(invalid).is_err());
    }
    pub(super) fn fixture(fail: bool) -> Reporters {
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
            root: 0,
            started: false,
            delay_clock: None,
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
                results: BTreeMap::new(),
                ai_only: BTreeSet::new(),
                ai_controls: BTreeSet::new(),
                controls: BTreeMap::new(),
                human: None,
                owned_filters: BTreeSet::new(),
                delay_values: BTreeMap::new(),
                delay_manager: delay::Manager::new(4, &[]).unwrap(),
                diagnostics: BTreeSet::new(),
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
    pub(in crate::prologue::driving) fn shutdown_fixture(
        participant: u16,
        initial: u16,
        final_state: u16,
    ) -> Reporters {
        let mut r = fixture(false);
        r.bind_human(participant);
        r.graph = events::Graph::new(
            3,
            vec![events::Connection {
                source: Endpoint { node: 0, event: 41 },
                target: Endpoint {
                    node: 2,
                    event: transitions::TRIGGER_ALL,
                },
                delivery: events::Delivery::Local,
            }],
        )
        .unwrap();
        r.state.transitions = BTreeMap::from([(2, (vec![initial], Some(final_state)))]);
        r
    }
    #[test]
    fn reporter_gate_preserves_participant_and_closes_before_the_next_event() {
        let mut r = fixture(false);
        assert_eq!(
            r.effect(&effect()).unwrap(),
            vec![Command::State(Action::MoveFirstCompleted {
                sources: vec![7],
                participant: 19,
                target: Some(8)
            })]
        );
        assert!(r.effect(&effect()).unwrap().is_empty());
    }

    #[test]
    fn bound_provider_filter_forwards_only_its_participant_and_keeps_repeats() {
        let mut r = fixture(false);
        r.state.owned_filters.insert(1);
        r.bind_human(19);
        r.graph = events::Graph::new(
            3,
            vec![
                events::Connection {
                    source: Endpoint { node: 0, event: 41 },
                    target: Endpoint {
                        node: 1,
                        event: nodes::IN,
                    },
                    delivery: events::Delivery::Local,
                },
                events::Connection {
                    source: Endpoint {
                        node: 1,
                        event: nodes::OUT,
                    },
                    target: Endpoint {
                        node: 2,
                        event: transitions::TRIGGER,
                    },
                    delivery: events::Delivery::Local,
                },
            ],
        )
        .unwrap();
        assert_eq!(r.effect(&effect()).unwrap().len(), 1);
        assert_eq!(r.effect(&effect()).unwrap().len(), 1);
        r.bind_human(20);
        assert!(r.effect(&effect()).unwrap().is_empty());
        r.state.human = None;
        assert!(r.effect(&effect()).unwrap().is_empty());
        let (state, _) = r
            .graph
            .transaction(&r.state, |s, d| {
                s.receive(
                    Endpoint {
                        node: 1,
                        event: nodes::IN,
                    },
                    None,
                    d,
                )
            })
            .unwrap();
        assert!(state.actions.is_empty());
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

    #[test]
    fn result_callback_preserves_the_triggering_participant() {
        let mut r = fixture(false);
        let identity = Identity {
            asset: "test/result".into(),
            index: 8,
            instances: vec![],
        };
        r.state.results.insert(3, identity.clone());
        r.graph = events::Graph::new(
            4,
            vec![events::Connection {
                source: Endpoint { node: 0, event: 41 },
                target: Endpoint {
                    node: 3,
                    event: nfs_content::events::lifecycle::bindings::results::TRIGGER,
                },
                delivery: events::Delivery::Local,
            }],
        )
        .unwrap();
        assert_eq!(
            r.effect(&effect()).unwrap(),
            vec![Command::Results {
                identity,
                event: nfs_content::events::lifecycle::bindings::results::TRIGGER,
                context: Some(events::Context::Participant(19)),
            }]
        );
    }

    #[test]
    fn ai_control_override_is_per_participant_and_releases_to_previous_value() {
        let mut r = fixture(false);
        r.state.ai_controls.extend([2, 3]);
        r.state.ai_only.extend([2, 3]);
        r.bind_human(19);
        let graph = events::Graph::new(4, vec![]).unwrap();
        let mut apply = |p, node, event| {
            let (state, remote) = graph
                .transaction(&r.state, |s, d| {
                    s.receive(Endpoint { node, event }, Some(p), d)
                })
                .unwrap();
            assert!(remote.is_empty());
            r.state = state;
            r.state.controls.get(&p).map(ai_control::Stack::current)
        };
        assert_eq!(apply(19, 2, ai_control::ENABLE), None);
        assert_eq!(apply(20, 2, ai_control::ENABLE), Some(true));
        assert_eq!(apply(20, 3, ai_control::DISABLE), Some(false));
        assert_eq!(apply(21, 3, ai_control::DISABLE), Some(false));
        assert_eq!(apply(20, 3, ai_control::RELEASE), Some(true));
        assert_eq!(apply(20, 2, ai_control::RELEASE), Some(false));
    }
}
