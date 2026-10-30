// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

impl Timers {
    pub(super) fn set_clocks(&mut self, clocks: Clocks) -> Result<(), Error> {
        time(clocks.receipt_seconds)?;
        if clocks.channels.len() > 256 {
            return Err(Error::Bound);
        }
        if self
            .clocks
            .as_ref()
            .is_some_and(|old| clocks.receipt_seconds < old.receipt_seconds)
        {
            return Err(Error::Invalid);
        }
        for (key, &value) in &clocks.channels {
            key.validate().map_err(|_| Error::Invalid)?;
            time(value)?;
        }
        self.clocks = Some(clocks);
        Ok(())
    }
}

impl Host {
    pub(super) fn dispatch_timing(
        &mut self,
        command: Command,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<HostEffect>, Error> {
        self.dispatch(
            vec![commands::Command::Timing(command)],
            &mut |machine, effect| {
                callback(machine, effect)
                    .map(|actions| actions.into_iter().map(commands::Command::State).collect())
            },
        )
    }
}

impl Observers {
    pub(super) fn timing_command(
        &mut self,
        plan: &Plan,
        command: Command,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<HostEffect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        match command {
            Command::Clocks(clocks) => {
                self.timed.set_clocks(clocks)?;
                Ok(vec![])
            }
            Command::Countdown(state) => {
                let definition = plan
                    .data
                    .states
                    .iter()
                    .find(|s| s.serializer == usize::from(state))
                    .ok_or(Error::UnknownState)?;
                if definition.kind != data::StateKind::Countdown
                    || plan.external.contains_key(&state)
                {
                    return Err(Error::Invalid);
                }
                let Some(deadline) = self.timed.countdowns.get(&state).copied() else {
                    return Ok(vec![]);
                };
                if self.timed.owned_time(plan, usize::from(state))? <= deadline {
                    return Ok(vec![]);
                }
                self.timed.countdowns.remove(&state);
                let mut actions = emit(
                    machine,
                    HostEffect::Timing(Effect::CountdownStop { state }),
                    effects,
                    callback,
                )?;
                actions.push(Action::CompleteAll {
                    state,
                    only_incomplete: false,
                });
                actions.extend(self.enqueue([HostWork::Timing(Work::CountdownEvent(state))])?);
                Ok(actions)
            }
            Command::Transition(id) => self.timing_transition(plan, id, 0),
            Command::Waypoint {
                source,
                participant,
                lap,
                seconds,
            } => {
                if !seconds.is_finite() || seconds < 0. {
                    return Err(Error::Invalid);
                }
                if !machine.members(source)?.contains(&participant) {
                    return Ok(vec![]);
                }
                self.enqueue([HostWork::Timing(Work::Waypoint {
                    source,
                    participant,
                    lap,
                    seconds,
                    observer: 0,
                })])
            }
            Command::DnfEvent {
                transition: id,
                event,
                participant,
            } => {
                if transition(plan, id)?.kind != data::TransitionKind::DnfTriggered {
                    return Err(Error::Invalid);
                }
                let Some(participant) = participant.filter(|_| event == 0x44b3_58f4) else {
                    return Ok(vec![]);
                };
                let work: Vec<_> = self
                    .timed
                    .dnf
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .filter(|e| e.participant == participant)
                    .map(|e| {
                        HostWork::Timing(Work::Dnf {
                            transition: id,
                            token: e.token,
                        })
                    })
                    .collect();
                self.enqueue(work)
            }
        }
    }

    pub(super) fn timing_transition(
        &mut self,
        plan: &Plan,
        id: usize,
        processed: usize,
    ) -> Result<Vec<Action>, Error> {
        let t = transition(plan, id)?;
        let work = match t.kind {
            data::TransitionKind::Dnf => {
                let now = self.timed.owned_time(plan, id)?;
                self.timed
                    .dnf
                    .get(&id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|e| e.deadline.is_some_and(|d| now >= d))
                    })
                    .map(|e| Work::Dnf {
                        transition: id,
                        token: e.token,
                    })
            }
            data::TransitionKind::FinishLine => {
                let now = self.timed.receipt_time()?;
                let queue = self.timed.finishes.entry(id).or_default();
                if let Some(index) = queue.entries.iter().position(|e| e.received + 0.5 <= now) {
                    let entry = queue.entries.remove(index);
                    queue.seconds = queue.seconds.max(entry.seconds);
                    queue.position = queue
                        .position
                        .checked_add(1)
                        .filter(|p| *p <= i32::MAX as u32)
                        .ok_or(Error::Bound)?;
                    Some(Work::FinishPublish {
                        transition: id,
                        entry,
                        seconds: queue.seconds,
                        position: queue.position,
                    })
                } else {
                    None
                }
            }
            _ => return Err(Error::Invalid),
        };
        let Some(work) = work else {
            return Ok(vec![]);
        };
        if processed >= states::MAX_PENDING {
            return Err(Error::Bound);
        }
        let mut actions = Vec::new();
        if let Work::FinishPublish { entry, .. } = &work {
            actions.push(Action::Move {
                source: entry.source,
                participant: entry.participant,
                target: t.target.map(|t| t.serializer as StateId),
            });
        }
        actions.extend(self.enqueue([
            HostWork::Timing(work),
            HostWork::Timing(Work::UpdateTransition {
                id,
                processed: processed + 1,
            }),
        ])?);
        Ok(actions)
    }

    pub(super) fn timing_waypoint(
        &mut self,
        plan: &Plan,
        work: Work,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<HostEffect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        let Work::Waypoint {
            source,
            participant,
            lap,
            seconds,
            mut observer,
        } = work
        else {
            return Err(Error::Invalid);
        };
        while let Some(&item) = plan.data.bindings.order.get(observer) {
            observer += 1;
            let Observer::Transition(id) = item else {
                continue;
            };
            let t = transition(plan, id)?;
            if !t
                .sources
                .iter()
                .any(|s| s.serializer == usize::from(source))
            {
                continue;
            }
            match t.configuration {
                data::TransitionConfiguration::Dnf { timeout_seconds } => {
                    let deadline =
                        time(self.timed.owned_time(plan, id)? + f64::from(timeout_seconds))?;
                    if let Some(entry) = self.timed.dnf.get_mut(&id).and_then(|entries| {
                        entries
                            .iter_mut()
                            .find(|e| e.source == source && e.participant == participant)
                    }) {
                        entry.deadline = Some(deadline);
                        let mut actions = emit(
                            machine,
                            HostEffect::Timing(Effect::DnfDeadline {
                                transition: id,
                                source,
                                participant,
                                deadline,
                            }),
                            effects,
                            callback,
                        )?;
                        actions.extend(self.enqueue([HostWork::Timing(Work::Waypoint {
                            source,
                            participant,
                            lap,
                            seconds,
                            observer,
                        })])?);
                        return Ok(actions);
                    }
                }
                data::TransitionConfiguration::FinishLine { laps } if lap > laps => {
                    let received = self.timed.receipt_time()?;
                    if self
                        .timed
                        .finishes
                        .values()
                        .map(|q| q.entries.len())
                        .sum::<usize>()
                        >= states::MAX_PENDING
                    {
                        return Err(Error::Bound);
                    }
                    let queue = self.timed.finishes.entry(id).or_default();
                    queue.entries.push(FinishEntry {
                        source,
                        participant,
                        seconds,
                        received,
                    });
                    queue
                        .entries
                        .sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
                }
                _ => {}
            }
        }
        Ok(vec![])
    }
}
