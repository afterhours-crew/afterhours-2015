// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::scoring::{
    ai_commands::{self as ai, Change, Command, Definition as Ai, Goal, Override},
    ai_control,
};
use std::collections::BTreeSet;

const MAX_MEMBERS: usize = crate::participant_states::MAX_PARTICIPANTS;

#[derive(Clone, Debug, PartialEq)]
struct ContextState {
    token: u64,
    goals: Vec<Goal>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct State {
    membership: BTreeMap<(participant_properties::Property, u64), bool>,
    locations: BTreeMap<participant_properties::Property, Option<[[f32; 3]; 4]>>,
    members: BTreeMap<u16, BTreeSet<u64>>,
    controllers: Vec<u16>,
    selections: BTreeMap<u16, usize>,
    overrides: BTreeSet<(u16, u64)>,
    contexts: BTreeMap<(u16, u64), ContextState>,
    serial: u64,
    pub(super) output: Vec<Change>,
}

impl Session {
    /// Manager membership is distinct from the actor's current AI-control flag.
    /// Supply owned membership before delivering Enable/Disable to the graph.
    pub fn bind_ai_membership(
        &mut self,
        manager: participant_properties::Property,
        participant: u64,
        is_ai: bool,
    ) -> Result<(), Error> {
        manager.validate()?;
        let key = (manager, participant);
        if !self.state.ai.membership.contains_key(&key)
            && self.state.ai.membership.len() >= MAX_OUTPUT
        {
            return Err(Error::Bound);
        }
        self.state.ai.membership.insert(key, is_ai);
        Ok(())
    }
    /// A live location provider takes precedence over the command's transform
    /// input. A resolved absent provider falls back to that authored input.
    pub fn bind_ai_location(
        &mut self,
        key: participant_properties::Property,
        transform: Option<[[f32; 3]; 4]>,
    ) -> Result<(), Error> {
        key.validate()?;
        if let Some(value) = &transform {
            for &v in value.iter().flatten() {
                finite(v)?;
            }
        }
        if !self.state.ai.locations.contains_key(&key)
            && self.state.ai.locations.len() >= MAX_OUTPUT
        {
            return Err(Error::Bound);
        }
        self.state.ai.locations.insert(key, transform);
        Ok(())
    }
    /// Completion is accepted only for an outstanding command in this instance.
    /// Repeated/retired tokens do nothing; callers must also validate instance ID.
    pub fn complete_ai_command(&mut self, token: u64, inputs: &Inputs) -> Result<Output, Error> {
        if !self.started {
            return Err(Error::InvalidGraph);
        }
        self.transaction(inputs, |c, d| {
            let found =
                c.state
                    .ai
                    .contexts
                    .iter()
                    .find_map(|(&(controller, participant), context)| {
                        context
                            .goals
                            .iter()
                            .position(|g| g.token == token)
                            .map(|index| (controller, participant, index))
                    });
            let Some((controller, participant, index)) = found else {
                return Ok(());
            };
            let goal = c
                .state
                .ai
                .contexts
                .get_mut(&(controller, participant))
                .ok_or(Error::InvalidGraph)?
                .goals
                .remove(index);
            if matches!(goal.command, Command::Goto { .. }) {
                d.emit_participant(
                    c,
                    Endpoint {
                        node: goal.node,
                        event: ai::COMPLETED,
                    },
                    participant,
                )?;
            }
            Ok(())
        })
    }
}
impl Candidate<'_> {
    fn ai_push(&mut self, change: Change) -> Result<(), Error> {
        if self.state.ai.output.len() >= MAX_OUTPUT {
            return Err(Error::Bound);
        }
        self.state.ai.output.push(change);
        Ok(())
    }
    fn ai_serial(&mut self) -> Result<u64, Error> {
        self.state.ai.serial = self.state.ai.serial.checked_add(1).ok_or(Error::Bound)?;
        Ok(self.state.ai.serial)
    }
    pub(super) fn ai_activate(
        &mut self,
        node: u16,
        definition: Ai<u16>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        if let Ai::Control(config) = definition {
            self.state.ai.controllers.push(node);
            self.state.ai.selections.insert(node, 0);
            if config.apply_existing {
                for state in &config.states {
                    for participant in self
                        .state
                        .ai
                        .members
                        .get(state)
                        .cloned()
                        .unwrap_or_default()
                    {
                        self.ai_start(node, participant, &config)?;
                    }
                }
            }
        }
        // Other AI classes bind their state on demand, without startup events.
        let _ = dispatch;
        Ok(())
    }
    pub(super) fn ai_event(
        &mut self,
        node: u16,
        event: u32,
        context: Option<Context>,
        definition: Ai<u16>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        match definition {
            Ai::Enable(config) => {
                let Some(Context::Participant(participant)) = context else {
                    return Ok(());
                };
                let key = (node, participant);
                if event == ai_control::RELEASE {
                    if self.state.ai.overrides.remove(&key) {
                        self.ai_push(Change::Override {
                            manager: config.actor_manager,
                            node,
                            participant,
                            action: Override::Release,
                        })?;
                    }
                    return Ok(());
                }
                let enabled = match event {
                    ai_control::ENABLE => true,
                    ai_control::DISABLE => false,
                    _ => return Ok(()),
                };
                let is_ai = if config.inclusion == ai_control::Inclusion::All {
                    false
                } else {
                    *self
                        .state
                        .ai
                        .membership
                        .get(&(config.ai_manager.clone(), participant))
                        .ok_or(Error::MissingInput)?
                };
                if !config.inclusion.includes(is_ai) {
                    return Ok(());
                }
                if !self.state.ai.overrides.contains(&key) {
                    if self.state.ai.overrides.len() >= MAX_OUTPUT {
                        return Err(Error::Bound);
                    }
                    self.state.ai.overrides.insert(key);
                    self.ai_push(Change::Override {
                        manager: config.actor_manager.clone(),
                        node,
                        participant,
                        action: Override::Acquire,
                    })?;
                }
                self.ai_push(Change::Override {
                    manager: config.actor_manager,
                    node,
                    participant,
                    action: Override::Set(enabled),
                })?;
                if enabled {
                    dispatch.emit_participant(
                        self,
                        Endpoint {
                            node,
                            event: ai_control::ENABLED,
                        },
                        participant,
                    )?;
                    if config.add_on_enable
                        && let Some(state) = config.state
                    {
                        self.ai_enter(state, participant, dispatch)?;
                    }
                } else {
                    if config.remove_on_disable
                        && let Some(state) = config.state
                    {
                        let removed = self
                            .state
                            .ai
                            .members
                            .get_mut(&state)
                            .is_some_and(|members| members.remove(&participant));
                        if removed
                            && matches!(
                                self.program.node(state)?.behavior,
                                Behavior::Ai(Ai::State { send_rpc: true, .. })
                            )
                        {
                            self.ai_push(Change::State {
                                node: state,
                                participant,
                                entered: false,
                            })?;
                        }
                        // Native direct removal calls observer +10 (noop here),
                        // not transition observer +18. Context lives until release.
                    }
                    dispatch.emit_participant(
                        self,
                        Endpoint {
                            node,
                            event: ai_control::DISABLED,
                        },
                        participant,
                    )?;
                }
                Ok(())
            }
            Ai::Control(config) if event == ai::NEXT && config.one_by_one => {
                if config.commands.is_empty() {
                    return Err(Error::InvalidGraph);
                }
                let selection = self
                    .state
                    .ai
                    .selections
                    .get_mut(&node)
                    .ok_or(Error::MissingInput)?;
                *selection = (*selection + 1) % config.commands.len();
                for state in &config.states {
                    for participant in self
                        .state
                        .ai
                        .members
                        .get(state)
                        .cloned()
                        .unwrap_or_default()
                    {
                        self.ai_release(node, participant)?;
                        self.ai_start(node, participant, &config)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn ai_enter(
        &mut self,
        node: u16,
        participant: u64,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let Behavior::Ai(Ai::State { send_rpc, .. }) = self.program.node(node)?.behavior else {
            return Err(Error::InvalidGraph);
        };
        let members = self.state.ai.members.entry(node).or_default();
        if !members.contains(&participant) && members.len() >= MAX_MEMBERS {
            return Err(Error::Bound);
        }
        members.insert(participant);
        if send_rpc {
            self.ai_push(Change::State {
                node,
                participant,
                entered: true,
            })?;
        }
        for controller in self.state.ai.controllers.clone() {
            let Behavior::Ai(Ai::Control(config)) = self.program.node(controller)?.behavior.clone()
            else {
                return Err(Error::InvalidGraph);
            };
            if config.states.contains(&node)
                && !self
                    .state
                    .ai
                    .contexts
                    .contains_key(&(controller, participant))
            {
                dispatch.emit_participant(
                    self,
                    Endpoint {
                        node: controller,
                        event: ai::ENTERED,
                    },
                    participant,
                )?;
                self.ai_start(controller, participant, &config)?;
            }
        }
        // Generic ParticipantState completes after entry observers (0473f680).
        // These AI controllers' completion observers are noops. No transition
        // originates from these private states in the supported utility graphs.
        Ok(())
    }
    fn ai_start(
        &mut self,
        node: u16,
        participant: u64,
        config: &ai::Control<u16>,
    ) -> Result<(), Error> {
        if config.commands.is_empty() {
            return Ok(());
        }
        if self.state.ai.contexts.contains_key(&(node, participant)) {
            return Ok(());
        }
        if self.state.ai.contexts.len() >= MAX_OUTPUT {
            return Err(Error::Bound);
        }
        let commands = if config.one_by_one {
            vec![
                config.commands[*self
                    .state
                    .ai
                    .selections
                    .get(&node)
                    .ok_or(Error::MissingInput)?],
            ]
        } else {
            config.commands.clone()
        };
        let token = self.ai_serial()?;
        let mut goals = vec![];
        for command_node in commands {
            let definition = self.program.node(command_node)?.behavior.clone();
            let command = match definition {
                Behavior::Ai(Ai::Goto(config)) => {
                    let provided = match &config.location {
                        Some(key) => *self
                            .state
                            .ai
                            .locations
                            .get(key)
                            .ok_or(Error::MissingInput)?,
                        None => None,
                    };
                    let transform = if let Some(value) = provided {
                        value
                    } else {
                        let Some(Value::Transform(value)) = self.resolve(Port {
                            node: command_node,
                            field: ai::LOCATION_TRANSFORM,
                        })?
                        else {
                            return Err(Error::MissingInput);
                        };
                        value
                    };
                    Command::Goto {
                        point: transform[3],
                        drift: config.drift,
                        stop: config.stop,
                        radius: config.radius,
                    }
                }
                Behavior::Ai(Ai::Scripted(value)) => Command::Scripted(value),
                _ => return Err(Error::InvalidGraph),
            };
            goals.push(Goal {
                token: self.ai_serial()?,
                node: command_node,
                command,
            });
        }
        self.state.ai.contexts.insert(
            (node, participant),
            ContextState {
                token,
                goals: goals.clone(),
            },
        );
        self.ai_push(Change::Start {
            node,
            token,
            participant,
            context_hash: config.context_hash,
            command_manager: config.command_manager.clone(),
            actor_manager: config.actor_manager.clone(),
            context_manager: config.context_manager.clone(),
            goals,
        })
    }
    fn ai_release(&mut self, node: u16, participant: u64) -> Result<(), Error> {
        if let Some(context) = self.state.ai.contexts.remove(&(node, participant)) {
            self.ai_push(Change::Release {
                node,
                token: context.token,
                participant,
            })?;
        }
        Ok(())
    }
    pub(super) fn ai_retire_node(&mut self, retiring: u16) -> Result<(), Error> {
        // Each native node destroys its contexts in ordered participant order.
        for (node, participant) in self.state.ai.contexts.keys().copied().collect::<Vec<_>>() {
            if node == retiring {
                self.ai_release(node, participant)?;
            }
        }
        for (node, participant) in self.state.ai.overrides.iter().copied().collect::<Vec<_>>() {
            if node != retiring {
                continue;
            }
            self.state.ai.overrides.remove(&(node, participant));
            let Behavior::Ai(Ai::Enable(config)) = self.program.node(node)?.behavior.clone() else {
                return Err(Error::InvalidGraph);
            };
            self.ai_push(Change::Override {
                manager: config.actor_manager,
                node,
                participant,
                action: Override::Release,
            })?;
        }
        Ok(())
    }
}
