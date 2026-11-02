// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bind native lifecycle callbacks to the same compiled shared-mode graph.
//! Scene services and replication consume the unhandled calls/effects; there
//! is no implicit success for an unknown native entity.
use super::{
    Effect, Plan,
    commands::Command,
    graphs::Step,
    links::{Object, Registry},
};
use nfs_content::{
    events::lifecycle::bindings::{Observer, PredicateKind},
    scoring::events::{Composition, Identity},
};
use nfs_world::{
    participant_states::{Action as StateAction, StateId},
    scoring::{
        Error,
        events::{
            Context, Endpoint,
            native::{Call, Operation, Request},
        },
        interpreter::hosted::Action,
        properties::Port,
        transitions,
        value::Value,
    },
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct Bindings {
    pub(super) instance: u64,
    pub(super) plan: Arc<Plan>,
    nodes: BTreeMap<Identity, u16>,
    observers: BTreeMap<u16, Observer>,
    states: BTreeMap<Object, StateId>,
}

impl Bindings {
    pub fn new(
        instance: u64,
        plan: Arc<Plan>,
        composition: &Composition,
        serializers: &[Identity],
        links: &Registry,
        scenes: &BTreeMap<u32, u64>,
    ) -> Result<Self, Error> {
        let mut nodes = BTreeMap::new();
        for (index, n) in composition.nodes.iter().enumerate() {
            let object = links.object(instance, &n.identity)?;
            if usize::from(object.node) != index
                || links.node(object)?.class != n.class
                || nodes.insert(n.identity.clone(), object.node).is_some()
            {
                return Err(Error::InvalidGraph);
            }
        }
        let node = |identity: &Identity| nodes.get(identity).copied().ok_or(Error::UnknownNode);
        let mut states = BTreeMap::new();
        for state in &plan.data.states {
            let id = StateId::try_from(state.serializer).map_err(|_| Error::Bound)?;
            let object = if let Some(external) = plan.external_state(id) {
                let scene = *scenes
                    .get(&external.scene_content_key)
                    .ok_or(Error::MissingInput)?;
                links.object(scene, &external.identity)?
            } else {
                let identity = serializers
                    .get(state.serializer)
                    .ok_or(Error::UnknownNode)?;
                Object {
                    instance,
                    node: node(identity)?,
                }
            };
            if states.insert(object, id).is_some() {
                return Err(Error::InvalidGraph);
            }
        }
        let b = &plan.data.bindings;
        let mut observers = BTreeMap::new();
        for &observer in &b.order {
            let identity = match observer {
                Observer::SetCounter(i) => b.set_counters.get(i).map(|n| &n.identity),
                Observer::Ai(i) => b.ai_controls.get(i).map(|n| &n.identity),
                Observer::Distance(i) => b.distances.get(i).map(|n| &n.identity),
                Observer::Mirror(i) => b.mirrors.get(i).map(|n| &n.identity),
                Observer::MirrorProvider(i) => b.mirror_providers.get(i).map(|n| &n.identity),
                Observer::Predicate(i) => b.predicates.get(i).map(|n| &n.identity),
                Observer::Property(i) => b.properties.get(i).map(|n| &n.identity),
                Observer::Context(i) => b.contexts.get(i).map(|n| &n.identity),
                Observer::Reporter(i) => b.reporters.get(i).map(|n| &n.identity),
                Observer::Sorter(i) => b.sorters.get(i).map(|n| &n.identity),
                Observer::Set(i) => b.sets.get(i).map(|n| &n.identity),
                Observer::Spawner(i) => b.spawners.get(i).map(|n| &n.identity),
                Observer::Transition(id) => serializers.get(id),
            }
            .ok_or(Error::InvalidGraph)?;
            if observers.insert(node(identity)?, observer).is_some() {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(Self {
            instance,
            plan,
            nodes,
            observers,
            states,
        })
    }
    pub fn state(&self, object: Object) -> Result<StateId, Error> {
        self.states.get(&object).copied().ok_or(Error::InvalidGraph)
    }
    /// The caller must first authorize the client against this live mode's
    /// primary membership. Method 8 is a state move, not a root leave/ack.
    pub fn request_final(&self, participant: u64) -> Result<Command, Error> {
        let root = self.plan.root().ok_or(Error::InvalidGraph)?;
        let target = StateId::try_from(root.final_state).map_err(|_| Error::Bound)?;
        let sources = root
            .registered_states
            .iter()
            .filter(|id| **id != root.final_state)
            .map(|id| StateId::try_from(*id).map_err(|_| Error::Bound))
            .collect::<Result<_, _>>()?;
        Ok(Command::State(StateAction::MoveFirstMember {
            sources,
            participant,
            target: Some(target),
        }))
    }
    pub fn transition(
        &self,
        instance: u64,
        request: &transitions::Request,
        links: &Registry,
    ) -> Result<Command, Error> {
        let mut sources = Vec::new();
        for key in &request.sources {
            for object in links.property(instance, key)? {
                if sources.len() >= nfs_world::participant_states::MAX_STATES {
                    return Err(Error::Bound);
                }
                sources.push(self.state(object)?);
            }
        }
        let target = if let Some(key) = &request.target {
            match links.property(instance, key)?.as_slice() {
                [] => None,
                [object] => Some(self.state(*object)?),
                _ => return Err(Error::InvalidGraph),
            }
        } else {
            None
        };
        Ok(Command::State(match request.selection {
            transitions::Selection::FirstCompleted(participant) => {
                StateAction::MoveFirstCompleted {
                    sources,
                    participant,
                    target,
                }
            }
            transitions::Selection::AllMembers => StateAction::MoveAll { sources, target },
        }))
    }
    pub fn invoke(&self, request: &Request, links: &Registry) -> Result<Option<Vec<Step>>, Error> {
        if let Call::Transition(transition) = &request.call {
            return Ok(Some(vec![Step::Lifecycle(vec![self.transition(
                request.instance,
                transition,
                links,
            )?])]));
        }
        if request.instance != self.instance {
            return Ok(None);
        }
        let observer = self.observers.get(&request.node).copied();
        match &request.call {
            Call::Update => {
                let object = Object {
                    instance: request.instance,
                    node: request.node,
                };
                let command = if let Some(state) = self.states.get(&object) {
                    self.plan
                        .data
                        .states
                        .iter()
                        .find(|s| s.serializer == usize::from(*state))
                        .filter(|s| s.kind == nfs_content::events::lifecycle::StateKind::Countdown)
                        .map(|_| super::timed::Command::Countdown(*state))
                } else if let Some(Observer::Transition(id)) = observer {
                    self.plan
                        .data
                        .transitions
                        .iter()
                        .find(|t| t.serializer == id)
                        .filter(|t| {
                            matches!(
                                t.kind,
                                nfs_content::events::lifecycle::TransitionKind::Dnf
                                    | nfs_content::events::lifecycle::TransitionKind::FinishLine
                            )
                        })
                        .map(|_| super::timed::Command::Transition(id))
                } else {
                    None
                };
                if let Some(command) = command {
                    return Ok(Some(vec![Step::Lifecycle(vec![Command::Timing(command)])]));
                }
            }
            Call::Bind | Call::Activate if observer.is_some() => {
                return Ok(Some(vec![Step::Lifecycle(vec![Command::Activate {
                    observer: observer.ok_or(Error::InvalidGraph)?,
                    binding: request.call == Call::Bind,
                }])]));
            }
            Call::Event { event, context } => {
                let command = match observer {
                    Some(Observer::Transition(id))
                        if self.plan.data.transitions.iter().any(|t| {
                            t.serializer == id
                                && t.kind
                                    == nfs_content::events::lifecycle::TransitionKind::DnfTriggered
                        }) =>
                    {
                        Some(Command::Timing(super::timed::Command::DnfEvent {
                            transition: id,
                            event: *event,
                            participant: match context {
                                Some(Context::Participant(p)) => Some(*p),
                                _ => None,
                            },
                        }))
                    }
                    Some(Observer::Predicate(i))
                        if self.plan.data.bindings.predicates[i].kind
                            == PredicateKind::Triggered =>
                    {
                        Some(Command::Predicate {
                            identity: self.plan.data.bindings.predicates[i].identity.clone(),
                            event: *event,
                            participant: match context {
                                Some(Context::Participant(p)) => Some(*p),
                                _ => None,
                            },
                        })
                    }
                    Some(Observer::Ai(i)) => Some(Command::Ai {
                        identity: self.plan.data.bindings.ai_controls[i].identity.clone(),
                        event: *event,
                    }),
                    Some(Observer::Set(i))
                        if matches!(
                            self.plan.data.bindings.sets[i].kind,
                            nfs_content::events::lifecycle::bindings::SetKind::Copy { .. }
                        ) =>
                    {
                        Some(Command::CopySet {
                            identity: self.plan.data.bindings.sets[i].identity.clone(),
                            event: *event,
                        })
                    }
                    _ => self
                        .plan
                        .data
                        .bindings
                        .results
                        .iter()
                        .find(|r| self.nodes.get(&r.identity) == Some(&request.node))
                        .map(|r| Command::Results {
                            identity: r.identity.clone(),
                            event: *event,
                            context: *context,
                        }),
                };
                if let Some(command) = command {
                    return Ok(Some(vec![Step::Lifecycle(vec![command])]));
                }
            }
            _ => {}
        }
        Ok(None)
    }
    fn node(&self, identity: &Identity) -> Result<u16, Error> {
        self.nodes.get(identity).copied().ok_or(Error::UnknownNode)
    }
    fn graph(&self, operation: Operation) -> Step {
        Step::Graph {
            id: self.instance,
            action: Action::Graph(operation),
        }
    }
    fn publish(&self, identity: &Identity, field: u32, value: Value) -> Result<Step, Error> {
        Ok(self.graph(Operation::Publish {
            port: Port {
                node: self.node(identity)?,
                field,
            },
            value,
        }))
    }
    fn emit(
        &self,
        identity: &Identity,
        event: u32,
        context: Option<Context>,
    ) -> Result<Step, Error> {
        Ok(self.graph(Operation::Emit {
            source: Endpoint {
                node: self.node(identity)?,
                event,
            },
            context,
        }))
    }
    /// These effects reenter the ordinary graph before the lifecycle host
    /// resumes its next observer. Other effects still require their owner.
    pub fn effect(&self, effect: &Effect) -> Result<Option<Vec<Step>>, Error> {
        Ok(Some(match effect {
            Effect::Reporter(super::reporters::Effect::Count { identity, value }) => {
                vec![self.publish(identity, super::reporters::COUNT, Value::Integer(*value))?]
            }
            Effect::Reporter(super::reporters::Effect::Event {
                identity,
                event,
                participant,
            }) => vec![self.emit(identity, *event, Some(Context::Participant(*participant)))?],
            Effect::Distance(e) => vec![self.publish(
                &e.identity,
                super::distance::TOTAL,
                Value::Float(e.total_metres),
            )?],
            Effect::Mirror(e) if e.bound => {
                vec![self.emit(&e.identity, super::mirrors::BOUND, None)?]
            }
            Effect::Mirror(_) => vec![],
            Effect::Timing(super::timed::Effect::CountdownEvent { state, event }) => {
                let object = self
                    .states
                    .iter()
                    .find_map(|(object, id)| (*id == *state).then_some(*object))
                    .ok_or(Error::UnknownNode)?;
                if object.instance != self.instance {
                    return Err(Error::InvalidGraph);
                }
                vec![self.graph(Operation::Emit {
                    source: Endpoint {
                        node: object.node,
                        event: *event,
                    },
                    context: None,
                })]
            }
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests;
