// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Validated mirror controllers over the shared lifecycle set owner. The input
//! projection, copied membership, and scalar comparison are separate state.
use super::channels;
use crate::event_logic::lifecycle::{Host, Plan, SceneInputs, commands::Command};
use nfs_content::{
    events::{
        discovery::{
            channels::{Publisher, Signal},
            membership::copies::Control,
        },
        lifecycle::{
            Scope,
            bindings::{Observer, ParticipantSet, SetKind, SetSource},
        },
    },
    scoring::events::Identity,
};
use nfs_world::{
    participant_states,
    scoring::{
        Error,
        math::Value,
        nodes::{self, Action, Effect, Node},
        participant_properties::Property,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn state_error(error: participant_states::Error) -> Error {
    match error {
        participant_states::Error::Bound => Error::Bound,
        _ => Error::InvalidGraph,
    }
}

#[derive(Clone, Debug)]
pub struct Definition {
    pub input: Identity,
    pub channel: Property,
    pub comparison: nodes::Definition,
}
impl From<&Control> for Definition {
    fn from(control: &Control) -> Self {
        Self {
            input: control.input_owner.clone(),
            channel: control.channel.clone(),
            comparison: control.comparison_config.definition,
        }
    }
}
#[derive(Clone, Debug)]
struct Mirror {
    input: Identity,
    channel: Property,
    comparison: Node,
    enabled: bool,
}
#[derive(Clone, Debug)]
pub struct State {
    host: Host,
    inputs: BTreeSet<Identity>,
    mirrors: BTreeMap<Identity, Mirror>,
    channels: channels::State,
}
impl State {
    pub fn new(
        controls: &BTreeMap<Identity, Definition>,
        channels: &BTreeMap<Property, Vec<Publisher>>,
    ) -> Result<Self, Error> {
        if controls.len() > nfs_content::events::discovery::MAX_EVENTS {
            return Err(Error::Bound);
        }
        let channels = channels::State::new(channels)?;
        let mut inputs = BTreeSet::new();
        let mut mirrors = BTreeMap::new();
        for (id, control) in controls {
            // Explicit start-trigger dispatch is owned by a full graph. These
            // map controllers are driven by their ready channel property only.
            if !matches!(
                control.comparison,
                nodes::Definition::CompareBoolean {
                    on_start: false,
                    ..
                }
            ) || !channels.values().contains_key(&control.channel)
                || controls.contains_key(&control.input)
            {
                return Err(Error::InvalidGraph);
            }
            inputs.insert(control.input.clone());
            mirrors.insert(
                id.clone(),
                Mirror {
                    input: control.input.clone(),
                    channel: control.channel.clone(),
                    comparison: Node::new(control.comparison)?,
                    enabled: false,
                },
            );
        }
        let mut scope = Scope::default();
        scope.bindings.sets = inputs
            .iter()
            .map(|id| ParticipantSet {
                identity: id.clone(),
                kind: SetKind::Owned,
            })
            .chain(controls.iter().map(|(id, control)| ParticipantSet {
                identity: id.clone(),
                kind: SetKind::Copy {
                    source: Some(SetSource::Local(control.input.clone())),
                },
            }))
            .collect();
        scope.bindings.order = (0..scope.bindings.sets.len()).map(Observer::Set).collect();
        let plan = Arc::new(Plan::scene(&scope).map_err(state_error)?);
        let (host, _) = Host::from_scene(plan, SceneInputs::default(), &mut |_, _| Ok(vec![]))
            .map_err(state_error)?;
        let mut result = Self {
            host,
            inputs,
            mirrors,
            channels,
        };
        let initial = result.channels.values().clone();
        for (property, value) in initial {
            result.publish(&property, value, Action::PropertyChanged(nodes::BOOL))?;
            result.publish(&property, value, Action::Activate)?;
        }
        Ok(result)
    }
    fn run(&mut self, commands: Vec<Command>) -> Result<(), Error> {
        self.host
            .dispatch(commands, &mut |_, _| Ok(vec![]))
            .map_err(state_error)?;
        Ok(())
    }
    pub fn contains(&self, copy: &Identity, participant: u64) -> Result<bool, Error> {
        if !self.mirrors.contains_key(copy) {
            return Err(Error::MissingInput);
        }
        Ok(self
            .host
            .source_members(&SetSource::Local(copy.clone()))
            .map_err(state_error)?
            .contains(&participant))
    }
    /// Supply every exact input owner after evaluating its authored membership.
    /// Removed players leave both input and listener-maintained copy when enabled.
    pub fn sync(&mut self, projected: &BTreeMap<Identity, BTreeSet<u64>>) -> Result<(), Error> {
        if projected.keys().cloned().collect::<BTreeSet<_>>() != self.inputs {
            return Err(Error::MissingInput);
        }
        if projected
            .values()
            .any(|p| p.len() > participant_states::MAX_PARTICIPANTS)
        {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        for (id, members) in projected {
            let source = SetSource::Local(id.clone());
            let old = next
                .host
                .source_members(&source)
                .map_err(state_error)?
                .clone();
            for (participant, member) in old
                .difference(members)
                .map(|&p| (p, false))
                .chain(members.difference(&old).map(|&p| (p, true)))
            {
                let mut commands = vec![Command::Set {
                    source: source.clone(),
                    participant,
                    member,
                }];
                for (copy, mirror) in &next.mirrors {
                    if &mirror.input == id && mirror.enabled {
                        commands.push(Command::Set {
                            source: SetSource::Local(copy.clone()),
                            participant,
                            member,
                        });
                    }
                }
                next.run(commands)?;
            }
        }
        *self = next;
        Ok(())
    }
    pub fn handles(&self, signal: &Signal) -> bool {
        self.channels.handles(signal)
    }
    pub fn signal(&mut self, signal: &Signal) -> Result<(), Error> {
        let mut next = self.clone();
        for (property, value) in next.channels.signal(signal)? {
            next.publish(&property, value, Action::PropertyChanged(nodes::BOOL))?;
        }
        *self = next;
        Ok(())
    }
    fn publish(&mut self, property: &Property, value: bool, action: Action) -> Result<(), Error> {
        let ids: Vec<_> = self
            .mirrors
            .iter()
            .filter(|(_, m)| &m.channel == property)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let mirror = self.mirrors.get_mut(&id).ok_or(Error::MissingInput)?;
            let effects = mirror.comparison.apply(action, |port| {
                (port == nodes::BOOL).then_some(Value::Boolean(value))
            })?;
            for effect in effects {
                match effect {
                    Effect::Emit(nodes::ON_TRUE) => {
                        // Copy completes before the two listener gates open.
                        self.run(vec![Command::CopySet {
                            identity: id.clone(),
                            event: 0x7c81_3ec0,
                        }])?;
                        self.mirrors
                            .get_mut(&id)
                            .ok_or(Error::MissingInput)?
                            .enabled = true;
                    }
                    Effect::Emit(nodes::ON_FALSE) => {
                        // Both gates close before Clear notifies set consumers.
                        self.mirrors
                            .get_mut(&id)
                            .ok_or(Error::MissingInput)?
                            .enabled = false;
                        self.run(vec![Command::CopySet {
                            identity: id.clone(),
                            event: 0x0cac_9f9c,
                        }])?;
                    }
                    _ => return Err(Error::InvalidGraph),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
