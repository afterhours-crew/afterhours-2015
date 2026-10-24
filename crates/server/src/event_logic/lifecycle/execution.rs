// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A lifecycle transaction that yields publications to an enclosing graph
//! executor. Publications retain native guards until their continuation ends.
use super::*;
use commands::{Command, Queue};

const PUBLICATION: u64 = 1 << 62;
const RETURN: u64 = 1 << 61;

#[derive(Clone, Debug, PartialEq)]
pub enum Yield {
    Effect(Effect),
    /// A nested graph-to-host call has finished. The enclosing graph callback
    /// can continue, make another call, or resume its original publication.
    Returned(u64),
}

pub struct Execution {
    host: Host,
    machine: states::execution::Execution,
    commands: Queue,
    publications: BTreeMap<u64, Effect>,
    calls: BTreeSet<u64>,
    serial: u64,
    waiting: bool,
    effects: Vec<Effect>,
    fault: Option<Error>,
}

impl Host {
    pub fn begin(&self, commands: Vec<Command>) -> Result<Execution, Error> {
        let mut queue = Queue::default();
        let actions = queue.expand(commands)?;
        Ok(Execution {
            host: self.clone(),
            machine: self
                .machine
                .begin(&actions, self.plan.data.bindings.order.len() + 2)?,
            commands: queue,
            publications: BTreeMap::new(),
            calls: BTreeSet::new(),
            serial: 0,
            waiting: false,
            effects: vec![],
            fault: None,
        })
    }
}

impl Execution {
    pub fn values(&self) -> values::Values<'_> {
        self.host.values()
    }
    pub fn machine(&self) -> &Machine {
        self.machine.machine()
    }
    pub fn plan(&self) -> &Plan {
        &self.host.plan
    }
    pub fn integer_value(&self, source: &IntegerSource, participant: Participant) -> Option<i32> {
        self.host.integer_value(source, participant)
    }
    pub fn boolean_value(
        &self,
        source: &BooleanSource,
        participant: Participant,
    ) -> Result<Option<bool>, Error> {
        self.host.boolean_value(source, participant)
    }
    pub fn ranking(&self, sorter: usize) -> Result<&[nfs_world::scoring::rank_query::Row], Error> {
        self.host.ranking(sorter)
    }
    pub fn float_value(&self, source: &Property, participant: Participant) -> Option<f32> {
        self.host.float_value(source, participant)
    }
    pub fn source_members(&self, source: &SetSource) -> Result<BTreeSet<Participant>, Error> {
        self.host.source_members(source)
    }

    pub fn advance(&mut self) -> Result<Option<Yield>, Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = self.advance_inner();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    pub fn resume(&mut self, commands: Vec<Command>) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if !self.waiting {
                return Err(Error::Invalid);
            }
            let actions = self.commands.expand(commands)?;
            self.machine.resume(actions)?;
            self.waiting = false;
            Ok(())
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    pub fn call(&mut self, commands: Vec<Command>) -> Result<u64, Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if !self.waiting {
                return Err(Error::Invalid);
            }
            if self.calls.len() >= states::MAX_PENDING || self.serial >= states::MAX_STEPS as u64 {
                return Err(Error::Bound);
            }
            let actions = self.commands.expand(commands)?;
            self.serial += 1;
            let token = RETURN | self.serial;
            self.calls.insert(token);
            self.machine.interject(
                actions,
                states::execution::Call {
                    notification: Notification::Invoked { token },
                    observer: 0,
                },
            )?;
            self.waiting = false;
            Ok(token)
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    pub fn finish(mut self) -> Result<(Host, Vec<Effect>), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        if self.waiting
            || !self.calls.is_empty()
            || !self.publications.is_empty()
            || !self.commands.is_empty()
            || !self.host.observers.pending.is_empty()
            || !self.host.observers.registering.is_empty()
        {
            return Err(Error::Invalid);
        }
        let (machine, _) = self.machine.finish()?;
        self.host.machine = machine;
        Ok((self.host, self.effects))
    }

    fn advance_inner(&mut self) -> Result<Option<Yield>, Error> {
        if self.waiting {
            return Err(Error::Invalid);
        }
        while let Some(call) = self.machine.advance()? {
            let event = call.notification;
            let observer = call.observer;
            if let Notification::Invoked { token } = event {
                if observer != 0 {
                    return Err(Error::Invalid);
                }
                if token & RETURN != 0 && token & (PUBLICATION | commands::COMMAND_BIT) == 0 {
                    if !self.calls.remove(&token) {
                        return Err(Error::Invalid);
                    }
                    self.waiting = true;
                    return Ok(Some(Yield::Returned(token)));
                }
                if token & PUBLICATION != 0 && token & commands::COMMAND_BIT == 0 {
                    let effect = self.publications.remove(&token).ok_or(Error::Invalid)?;
                    if self.effects.len() >= states::MAX_STEPS {
                        return Err(Error::Bound);
                    }
                    self.effects.push(effect.clone());
                    self.waiting = true;
                    return Ok(Some(Yield::Effect(effect)));
                }
            }
            let publications = &mut self.publications;
            let serial = &mut self.serial;
            let mut callback = |_: &dyn nfs_world::participant_states::Membership,
                                effect: &Effect| {
                if publications.len() >= states::MAX_PENDING || *serial >= states::MAX_STEPS as u64
                {
                    return Err(Error::Bound);
                }
                *serial += 1;
                let token = PUBLICATION | *serial;
                publications.insert(token, effect.clone());
                Ok(vec![Action::Invoke { observer: 0, token }])
            };
            let machine = self.machine.machine();
            let plan = &self.host.plan;
            let observers = &mut self.host.observers;
            // `emit`'s local accounting is checked by the publication queue;
            // externally visible effects are appended only when actually yielded.
            let mut staged = Vec::new();
            let actions = if let Notification::Invoked { token } = event {
                if token & commands::COMMAND_BIT != 0 {
                    observers.command(plan, self.commands.take(token)?)?
                } else {
                    let work = observers.pending.remove(&token).ok_or(Error::Invalid)?;
                    observers.work(plan, work, machine, &mut staged, &mut callback)?
                }
            } else if observer == 0 {
                callback(machine, &Effect::State(event))?
            } else if observer == plan.data.bindings.order.len() + 1 {
                let mut actions = callback(machine, &Effect::Hook(event))?;
                actions.extend(observers.enqueue([Work::StateHook(event)])?);
                actions
            } else {
                observers.observe(
                    plan,
                    plan.data.bindings.order[observer - 1],
                    event,
                    machine,
                    &mut staged,
                    &mut callback,
                )?
            };
            self.machine.resume(actions)?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
