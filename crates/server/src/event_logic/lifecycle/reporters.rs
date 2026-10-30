// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native state reporters: 046692b0/04669880/04669c00 and
//! 046aaea0/046abf10/046ac0a0. Observer slot +18 runs after source removal
//! and before target entry (0402c090), not on the earlier removal callback.
use super::*;

pub const COUNT: u32 = 0xe6d6_7e59;
pub const ENTER: u32 = 0x864a_8f04;
pub const EXIT: u32 = 0x10bb_9c36;
pub const COMPOUND_ENTER: u32 = 0x4f9c_6ff7;
pub const COMPOUND_EXIT: u32 = 0x282f_2285;
pub const COMPLETED: u32 = 0x65fb_ff69;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Count {
        identity: Identity,
        value: i32,
    },
    Event {
        identity: Identity,
        event: u32,
        participant: Participant,
    },
}

#[derive(Clone, Debug)]
pub(super) enum Work {
    Count(usize),
    Event(usize, u32, Participant),
}

pub(super) fn observe(plan: &Plan, index: usize, event: Notification) -> Vec<super::Work> {
    let definition = &plan.data.bindings.reporters[index];
    let has = |s| {
        definition
            .states
            .iter()
            .any(|e| e.serializer == usize::from(s))
    };
    let mut work = Vec::new();
    let event = match event {
        Notification::Entered {
            state, participant, ..
        } if has(state) => {
            work.push(super::Work::Reporter(Work::Count(index)));
            Some((
                if definition.compound {
                    COMPOUND_ENTER
                } else {
                    ENTER
                },
                participant,
            ))
        }
        Notification::Transitioned {
            source,
            participant,
            target,
        } if has(source) => {
            work.push(super::Work::Reporter(Work::Count(index)));
            if definition.compound && target.is_some_and(has) {
                None
            } else {
                Some((
                    if definition.compound {
                        COMPOUND_EXIT
                    } else {
                        EXIT
                    },
                    participant,
                ))
            }
        }
        Notification::Completed { state, participant } if has(state) => {
            Some((COMPLETED, participant))
        }
        _ => None,
    };
    if let Some((event, participant)) = event {
        work.push(super::Work::Reporter(Work::Event(
            index,
            event,
            participant,
        )));
    }
    work
}

impl Observers {
    pub(super) fn reporter_work(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<super::Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        let effect = match work {
            Work::Count(index) => {
                let definition = &plan.data.bindings.reporters[index];
                let mut count = 0usize;
                for state in &definition.states {
                    count = count
                        .checked_add(machine.members(state.serializer as StateId)?.len())
                        .ok_or(Error::Bound)?;
                }
                Effect::Count {
                    identity: definition.identity.clone(),
                    value: i32::try_from(count).map_err(|_| Error::Bound)?,
                }
            }
            Work::Event(index, event, participant) => Effect::Event {
                identity: plan.data.bindings.reporters[index].identity.clone(),
                event,
                participant,
            },
        };
        emit(machine, super::Effect::Reporter(effect), effects, callback)
    }
}
