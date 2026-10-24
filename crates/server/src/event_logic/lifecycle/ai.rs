// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native controller observer order (04632f10/04633e40/046352b0). The edge
//! resolves the retained command/provider identities into replicated goals.
use super::*;
use nfs_world::scoring::ai_commands::{ENTERED, NEXT};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Event {
        identity: Identity,
        participant: Participant,
        event: u32,
    },
    Start {
        identity: Identity,
        participant: Participant,
        token: u64,
        context_hash: u32,
        command_manager: Property,
        actor_manager: Property,
        commands: Vec<Identity>,
    },
    Release {
        identity: Identity,
        participant: Participant,
        token: u64,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    contexts: BTreeMap<(usize, Participant), u64>,
    selections: BTreeMap<usize, usize>,
    retired: BTreeSet<usize>,
    serial: u64,
}

#[derive(Clone, Debug)]
pub(super) enum Work {
    Enter(usize, Participant),
    Start(usize, Participant),
    Release(usize, Participant),
    Event(usize, u32),
    Retire(usize),
}

pub(super) fn observe(plan: &Plan, i: usize, event: Notification) -> Vec<super::Work> {
    let control = &plan.data.bindings.ai_controls[i];
    let has = |s| {
        control
            .states
            .iter()
            .any(|e| e.serializer == usize::from(s))
    };
    let work = match event {
        Notification::Entered {
            state, participant, ..
        } if has(state) => Work::Enter(i, participant),
        // Activation enumerates existing members by calling creation directly;
        // it does not synthesize the participant-entry output (04631020).
        Notification::Existing { state, participant } if has(state) && control.apply_existing => {
            Work::Start(i, participant)
        }
        Notification::Transitioned {
            source,
            participant,
            target,
        } if has(source) && !target.is_some_and(has) => Work::Release(i, participant),
        // Native direct removal (+10) and completion (+20) are noops.
        _ => return vec![],
    };
    vec![super::Work::Ai(work)]
}

impl Host {
    pub fn ai_context(&self, control: usize, participant: Participant) -> Option<u64> {
        self.observers
            .ai_controls
            .contexts
            .get(&(control, participant))
            .copied()
    }

    /// Retire this host's controllers at graph destruction. Repeated calls are
    /// inert, and a failed external release rolls the entire operation back.
    pub fn retire_ai(&mut self, callback: &mut Callback<'_>) -> Result<Vec<super::Effect>, Error> {
        let mut next = self.clone();
        let work = (0..next.plan.data.bindings.ai_controls.len())
            .map(|i| super::Work::Ai(Work::Retire(i)))
            .collect::<Vec<_>>();
        let actions = next.observers.enqueue(work)?;
        let effects = next.execute(&actions, callback)?;
        *self = next;
        Ok(effects)
    }
}

impl Observers {
    pub(super) fn ai_work(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<super::Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        let index = match work {
            Work::Enter(i, _)
            | Work::Start(i, _)
            | Work::Release(i, _)
            | Work::Event(i, _)
            | Work::Retire(i) => i,
        };
        let control = plan
            .data
            .bindings
            .ai_controls
            .get(index)
            .ok_or(Error::Invalid)?;
        let mut actions = Vec::new();
        match work {
            Work::Enter(i, participant) => {
                if self.ai_controls.retired.contains(&i)
                    || self.ai_controls.contexts.contains_key(&(i, participant))
                {
                    return Ok(actions);
                }
                actions.extend(emit(
                    machine,
                    super::Effect::Ai(Effect::Event {
                        identity: control.identity.clone(),
                        participant,
                        event: ENTERED,
                    }),
                    effects,
                    callback,
                )?);
                actions.extend(self.enqueue([super::Work::Ai(Work::Start(i, participant))])?);
            }
            Work::Start(i, participant) => {
                if control.commands.is_empty()
                    || self.ai_controls.retired.contains(&i)
                    || self.ai_controls.contexts.contains_key(&(i, participant))
                {
                    return Ok(actions);
                }
                if self.ai_controls.contexts.len() >= states::MAX_PENDING {
                    return Err(Error::Bound);
                }
                let commands = if control.one_by_one {
                    vec![
                        control.commands[*self.ai_controls.selections.get(&i).unwrap_or(&0)]
                            .clone(),
                    ]
                } else {
                    control.commands.clone()
                };
                self.ai_controls.serial =
                    self.ai_controls.serial.checked_add(1).ok_or(Error::Bound)?;
                let token = self.ai_controls.serial;
                self.ai_controls.contexts.insert((i, participant), token);
                actions.extend(emit(
                    machine,
                    super::Effect::Ai(Effect::Start {
                        identity: control.identity.clone(),
                        participant,
                        token,
                        context_hash: control.context_hash,
                        command_manager: control.command_manager.clone(),
                        actor_manager: control.actor_manager.clone(),
                        commands,
                    }),
                    effects,
                    callback,
                )?);
            }
            Work::Release(i, participant) => {
                if let Some(token) = self.ai_controls.contexts.remove(&(i, participant)) {
                    actions.extend(emit(
                        machine,
                        super::Effect::Ai(Effect::Release {
                            identity: control.identity.clone(),
                            participant,
                            token,
                        }),
                        effects,
                        callback,
                    )?);
                }
            }
            Work::Event(i, event)
                if event == NEXT
                    && control.one_by_one
                    && !self.ai_controls.retired.contains(&i) =>
            {
                if control.commands.is_empty() {
                    return Err(Error::Invalid);
                }
                let selection = self.ai_controls.selections.entry(i).or_default();
                *selection = (*selection + 1) % control.commands.len();
                let mut work = Vec::new();
                for state in &control.states {
                    for &participant in machine.members(state.serializer as StateId)? {
                        work.push(super::Work::Ai(Work::Release(i, participant)));
                        work.push(super::Work::Ai(Work::Start(i, participant)));
                    }
                }
                actions.extend(self.enqueue(work)?);
            }
            Work::Retire(i) => {
                self.ai_controls.retired.insert(i);
                let work = self
                    .ai_controls
                    .contexts
                    .keys()
                    .filter(|(controller, _)| *controller == i)
                    .map(|&(_, participant)| super::Work::Ai(Work::Release(i, participant)))
                    .collect::<Vec<_>>();
                actions.extend(self.enqueue(work)?);
            }
            Work::Event(_, _) => {}
        }
        Ok(actions)
    }
}

#[cfg(test)]
mod tests;
