// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Continuations returned by a spawned graph or shared-mode observer. These run
//! before the next native listener, within the enclosing host transaction.
use super::*;
use nfs_world::scoring::events::Context;
use std::cell::RefCell;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Timing(timed::Command),
    Float {
        source: Property,
        participant: Participant,
        value: f32,
    },
    FloatObserver {
        source: Property,
        observer: u64,
        subscribe: bool,
    },
    /// Called at the native node's bind or activation step, never globally
    /// before the ordinary graph has reached that node.
    Activate {
        observer: Observer,
        binding: bool,
    },
    Ai {
        identity: Identity,
        event: u32,
    },
    State(Action),
    Integer {
        source: IntegerSource,
        participant: Participant,
        value: i32,
    },
    IntegerObserver {
        source: IntegerSource,
        observer: u64,
        subscribe: bool,
    },
    Boolean {
        key: Property,
        participant: Participant,
        value: bool,
    },
    BooleanObserver {
        source: BooleanSource,
        observer: u64,
        subscribe: bool,
    },
    Set {
        source: SetSource,
        participant: Participant,
        member: bool,
    },
    CopySet {
        identity: Identity,
        event: u32,
    },
    SetObserver {
        source: SetSource,
        subscription: sets::Subscription,
        observer: u64,
        subscribe: bool,
    },
    Predicate {
        identity: Identity,
        event: u32,
        participant: Option<Participant>,
    },
    Results {
        identity: Identity,
        event: u32,
        context: Option<Context>,
    },
}

pub type Callback<'a> = dyn FnMut(&dyn nfs_world::participant_states::Membership, &Effect) -> Result<Vec<Command>, Error>
    + 'a;

pub(super) const COMMAND_BIT: u64 = 1 << 63;
#[derive(Default)]
pub(super) struct Queue {
    next: u64,
    pending: BTreeMap<u64, Command>,
}
impl Queue {
    pub(super) fn expand(&mut self, commands: Vec<Command>) -> Result<Vec<Action>, Error> {
        if commands.len() > states::MAX_PENDING {
            return Err(Error::Bound);
        }
        let mut actions = Vec::new();
        for command in commands {
            if let Command::State(action) = command {
                // Invocation tokens belong only to this dispatch or to the host's
                // own observer queue; callers cannot forge a continuation.
                if matches!(action, Action::Invoke { .. }) {
                    return Err(Error::Invalid);
                }
                actions.push(action);
            } else {
                if self.pending.len() >= states::MAX_PENDING
                    || self.next >= states::MAX_STEPS as u64
                {
                    return Err(Error::Bound);
                }
                self.next += 1;
                let token = COMMAND_BIT | self.next;
                self.pending.insert(token, command);
                actions.push(Action::Invoke { observer: 0, token });
            }
        }
        Ok(actions)
    }
    pub(super) fn take(&mut self, token: u64) -> Result<Command, Error> {
        self.pending.remove(&token).ok_or(Error::Invalid)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

impl Host {
    /// Run graph-to-host calls and their synchronous consequences atomically.
    /// External graph/replication state in the callback must also be a candidate
    /// and must be committed only after this method succeeds.
    pub fn dispatch(
        &mut self,
        commands: Vec<Command>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let queue = RefCell::new(Queue::default());
        let actions = queue.borrow_mut().expand(commands)?;
        let mut next = self.clone();
        let effects = next.execute_queued(
            &actions,
            &mut |machine, effect| queue.borrow_mut().expand(callback(machine, effect)?),
            Some(&queue),
        )?;
        *self = next;
        Ok(effects)
    }
}

impl Observers {
    pub(super) fn command(&mut self, plan: &Plan, command: Command) -> Result<Vec<Action>, Error> {
        match command {
            Command::Timing(command) => self.enqueue([Work::Timing(timed::Work::Command(command))]),
            Command::Float {
                source,
                participant,
                value,
            } => self.write_float(source, participant, value),
            Command::FloatObserver {
                source,
                observer,
                subscribe,
            } => {
                self.floats.observe(source, observer, subscribe)?;
                Ok(vec![])
            }
            Command::Activate { observer, binding } => {
                if !plan.data.bindings.order.contains(&observer) {
                    return Err(Error::Invalid);
                }
                self.enqueue([Work::Activate { observer, binding }])
            }
            Command::Ai { identity, event } => {
                let index = plan
                    .data
                    .bindings
                    .ai_controls
                    .iter()
                    .position(|c| c.identity == identity)
                    .ok_or(Error::Invalid)?;
                self.enqueue([Work::Ai(ai::Work::Event(index, event))])
            }
            Command::State(_) => Err(Error::Invalid),
            Command::IntegerObserver {
                source,
                observer,
                subscribe,
            } => {
                plan.validate_integer(&source)?;
                if observer == 0 {
                    return Err(Error::Invalid);
                }
                if subscribe {
                    self.rankings
                        .subscribe(source, rankings::Listener::External(observer))?;
                } else {
                    self.rankings
                        .unsubscribe(&source, rankings::Listener::External(observer));
                }
                Ok(vec![])
            }
            Command::Integer {
                source,
                participant,
                value,
            } => {
                plan.validate_integer(&source)?;
                self.enqueue([Work::Ranking(rankings::Work::Write {
                    source,
                    participant,
                    value,
                })])
            }
            Command::Boolean {
                key,
                participant,
                value,
            } => self.write_boolean(key, participant, value),
            Command::BooleanObserver {
                source,
                observer,
                subscribe,
            } => {
                plan.validate_boolean(&source)?;
                self.boolean_listeners
                    .external(source, observer, subscribe)?;
                Ok(vec![])
            }
            Command::Set {
                source,
                participant,
                member,
            } => self.write_set(plan, source, participant, member),
            Command::CopySet { identity, event } => self.copy_set(plan, &identity, event),
            Command::SetObserver {
                source,
                subscription,
                observer,
                subscribe,
            } => {
                plan.validate_set(&source)?;
                self.set_listeners
                    .external(source, subscription, observer, subscribe)?;
                Ok(vec![])
            }
            Command::Predicate {
                identity,
                event,
                participant,
            } => {
                let i = *plan.predicates.get(&identity).ok_or(Error::Invalid)?;
                if plan.data.bindings.predicates[i].kind != PredicateKind::Triggered {
                    return Err(Error::Invalid);
                }
                let value = match event {
                    0xa425_3151 => true,
                    0x27d4_023a => false,
                    _ => return Ok(vec![]),
                };
                let Some(participant) = participant else {
                    return Ok(vec![]);
                };
                self.enqueue([Work::AssignPredicate(i, participant, value)])
            }
            Command::Results {
                identity,
                event,
                context,
            } => self.enqueue([Work::Results {
                identity,
                event,
                context,
            }]),
        }
    }

    pub(super) fn write_boolean(
        &mut self,
        key: Property,
        participant: Participant,
        value: bool,
    ) -> Result<Vec<Action>, Error> {
        key.validate().map_err(|_| Error::Invalid)?;
        if self.booleans.get(&(key.clone(), participant)) == Some(&value) {
            return Ok(vec![]);
        }
        if self.booleans.len() >= states::MAX_PENDING
            && !self.booleans.contains_key(&(key.clone(), participant))
        {
            return Err(Error::Bound);
        }
        self.booleans.insert((key.clone(), participant), value);
        self.enqueue([Work::Boolean(
            BooleanSource::Channel(key),
            participant,
            value,
        )])
    }

    pub(super) fn write_set(
        &mut self,
        plan: &Plan,
        source: SetSource,
        participant: Participant,
        member: bool,
    ) -> Result<Vec<Action>, Error> {
        plan.validate_set(&source)?;
        let set = match &source {
            SetSource::Local(id) => {
                let i = plan.sets[id];
                if !matches!(
                    plan.data.bindings.sets[i].kind,
                    SetKind::Owned | SetKind::Copy { .. }
                ) {
                    return Err(Error::Invalid);
                }
                &mut self.sets[i]
            }
            SetSource::Channel(p) => {
                if self.channel_sets.len() >= 256 && !self.channel_sets.contains_key(p) {
                    return Err(Error::Bound);
                }
                self.channel_sets.entry(p.clone()).or_default()
            }
        };
        if !assign(set, participant, member)? {
            return Ok(vec![]);
        }
        self.enqueue([Work::Set(source, participant, member)])
    }
}

#[cfg(test)]
mod tests;
