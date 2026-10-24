// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Yield {
    Effect(Box<Publication>),
    Returned(u64),
}

enum Pending {
    Command(Scoped<Command>),
    Local(Instance, u64),
    Publication(Publication),
    Dispatch(Notification, usize),
    Hook(Notification),
    Return(u64),
}

pub struct Execution {
    world: WorldHost,
    machine: states::execution::Execution,
    pending: BTreeMap<u64, Pending>,
    serial: u64,
    calls: BTreeSet<u64>,
    waiting: bool,
    fault: Option<Error>,
    effects: Vec<Publication>,
}

impl WorldHost {
    pub fn begin(&self, commands: Vec<Scoped<Command>>) -> Result<Execution, Error> {
        let mut execution = Execution {
            world: self.clone(),
            machine: self.machine.begin_routed(&[], self.seeds.len())?,
            pending: BTreeMap::new(),
            serial: 0,
            calls: BTreeSet::new(),
            waiting: false,
            fault: None,
            effects: vec![],
        };
        let actions = execution.commands(commands)?;
        execution.machine = self.machine.begin_routed(&actions, self.seeds.len())?;
        Ok(execution)
    }
}

impl Execution {
    pub fn values(&self, instance: u64) -> Result<values::Values<'_>, Error> {
        self.world.values(instance)
    }
    pub fn view(&self, instance: Instance) -> Result<View<'_>, Error> {
        let scope = self
            .world
            .scopes
            .get(&instance)
            .ok_or(Error::UnknownState)?;
        Ok(View {
            machine: self.machine.machine(),
            local: &scope.local,
        })
    }
    pub fn boolean(
        &self,
        instance: Instance,
        source: &BooleanSource,
        participant: Participant,
    ) -> Result<Option<bool>, Error> {
        self.world.boolean(instance, source, participant)
    }

    fn enqueue(&mut self, pending: Pending) -> Result<Action, Error> {
        enqueue(&mut self.pending, &mut self.serial, pending)
    }

    fn commands(&mut self, commands: Vec<Scoped<Command>>) -> Result<Vec<Action>, Error> {
        if commands.len() > states::MAX_PENDING {
            return Err(Error::Bound);
        }
        let mut actions = Vec::new();
        for command in commands {
            let scope = self
                .world
                .scopes
                .get(&command.instance)
                .ok_or(Error::UnknownState)?;
            if let Command::State(action) = command.value {
                // Seeding and continuation addresses are internal capabilities.
                if matches!(action, Action::SeedMembers { .. } | Action::Invoke { .. }) {
                    return Err(Error::Invalid);
                }
                actions.push(scope.action(action)?);
            } else {
                actions.push(self.enqueue(Pending::Command(command))?);
            }
        }
        Ok(actions)
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
    pub fn resume(&mut self, commands: Vec<Scoped<Command>>) -> Result<(), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if !self.waiting {
                return Err(Error::Invalid);
            }
            let actions = self.commands(commands)?;
            self.machine.resume(actions)?;
            self.waiting = false;
            Ok(())
        })();
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }
    pub fn call(&mut self, commands: Vec<Scoped<Command>>) -> Result<u64, Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = (|| {
            if !self.waiting {
                return Err(Error::Invalid);
            }
            let actions = self.commands(commands)?;
            let token = self.serial.checked_add(1).ok_or(Error::Bound)?;
            let Action::Invoke { token, .. } = self.enqueue(Pending::Return(token))? else {
                unreachable!()
            };
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
    pub fn finish(mut self) -> Result<(WorldHost, Vec<Publication>), Error> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        if self.waiting
            || !self.pending.is_empty()
            || !self.calls.is_empty()
            || self
                .world
                .scopes
                .values()
                .any(|s| !s.observers.pending.is_empty() || !s.observers.registering.is_empty())
        {
            return Err(Error::Invalid);
        }
        let (machine, _) = self.machine.finish()?;
        self.world.machine = machine;
        Ok((self.world, self.effects))
    }

    fn local_actions(
        &mut self,
        instance: Instance,
        actions: Vec<Action>,
    ) -> Result<Vec<Action>, Error> {
        let mut result = Vec::new();
        for action in actions {
            result.push(if let Action::Invoke { observer: 0, token } = action {
                self.enqueue(Pending::Local(instance, token))?
            } else {
                self.world.scopes[&instance].action(action)?
            });
        }
        Ok(result)
    }

    fn publication(
        &mut self,
        instance: Instance,
        value: Effect,
        event: Notification,
    ) -> Result<Action, Error> {
        let transition_target = match event {
            Notification::Transitioned {
                target: Some(target),
                ..
            } => Some(*self.world.owners.get(&target).ok_or(Error::UnknownState)?),
            _ => None,
        };
        self.enqueue(Pending::Publication(Publication {
            instance,
            value,
            transition_target,
        }))
    }

    fn subscribe(&mut self, listener: Listener, source: StateId) -> Result<(), Error> {
        let id = self.world.scopes[&listener.instance].state(source)?;
        let list = self
            .world
            .listeners
            .get_mut(&id)
            .ok_or(Error::UnknownState)?;
        if list.items.len() >= states::MAX_PENDING {
            return Err(Error::Bound);
        }
        list.items.push(Some(listener));
        Ok(())
    }

    fn observe(&mut self, listener: Listener, event: Notification) -> Result<Vec<Action>, Error> {
        let scope = self
            .world
            .scopes
            .get_mut(&listener.instance)
            .ok_or(Error::UnknownState)?;
        let event = scope.notification(event)?;
        let view = View {
            machine: self.machine.machine(),
            local: &scope.local,
        };
        let pending = &mut self.pending;
        let serial = &mut self.serial;
        // Observer-local invocations are translated below. Publications already
        // belong to this execution and are collected separately by sentinel.
        let actions = scope.observers.observe(
            &scope.plan,
            listener.observer,
            event,
            &view,
            &mut vec![],
            &mut |_, effect| {
                let action = enqueue(
                    pending,
                    serial,
                    Pending::Publication(Publication {
                        instance: listener.instance,
                        value: effect.clone(),
                        transition_target: None,
                    }),
                )?;
                Ok(vec![tag(action)])
            },
        )?;
        self.translate(listener.instance, actions)
    }

    fn translate(
        &mut self,
        instance: Instance,
        actions: Vec<Action>,
    ) -> Result<Vec<Action>, Error> {
        let mut output = Vec::new();
        for action in actions {
            if let Action::Invoke { observer: 0, token } = action
                && token & PUBLICATION != 0
            {
                output.push(Action::Invoke {
                    observer: 0,
                    token: token & !PUBLICATION,
                });
            } else {
                output.extend(self.local_actions(instance, vec![action])?);
            }
        }
        Ok(output)
    }

    fn work(&mut self, instance: Instance, work: Work) -> Result<Vec<Action>, Error> {
        let mut subscribe = Vec::new();
        match &work {
            Work::Activate { observer, binding } => {
                let scope = &self.world.scopes[&instance];
                let at_bind = matches!(observer, Observer::Set(i)
                    if matches!(scope.plan.data.bindings.sets[*i].kind, SetKind::Predicate { .. }));
                if *binding == at_bind && !scope.observers.enabled.contains(observer) {
                    // Seeded sources register individually below; all others
                    // register now, before any resulting synchronous callbacks.
                    subscribe = watched(&scope.plan, *observer)
                        .into_iter()
                        .map(|s| {
                            (
                                Listener {
                                    instance,
                                    observer: *observer,
                                },
                                s,
                            )
                        })
                        .collect();
                }
            }
            Work::RegisterSource {
                observer,
                sources,
                next,
            } => {
                if let Some(&source) = sources.get(*next) {
                    self.subscribe(
                        Listener {
                            instance,
                            observer: *observer,
                        },
                        source,
                    )?;
                } else {
                    for source in watched(&self.world.scopes[&instance].plan, *observer) {
                        if !sources.contains(&source) {
                            self.subscribe(
                                Listener {
                                    instance,
                                    observer: *observer,
                                },
                                source,
                            )?;
                        }
                    }
                }
            }
            _ => {}
        }
        let scope = self
            .world
            .scopes
            .get_mut(&instance)
            .ok_or(Error::UnknownState)?;
        let view = View {
            machine: self.machine.machine(),
            local: &scope.local,
        };
        let pending = &mut self.pending;
        let serial = &mut self.serial;
        let actions =
            scope
                .observers
                .work(&scope.plan, work, &view, &mut vec![], &mut |_, effect| {
                    Ok(vec![tag(enqueue(
                        pending,
                        serial,
                        Pending::Publication(Publication {
                            instance,
                            value: effect.clone(),
                            transition_target: None,
                        }),
                    )?)])
                })?;
        for (listener, source) in subscribe {
            if !self.world.scopes[&instance]
                .observers
                .registering
                .contains_key(&listener.observer)
            {
                self.subscribe(listener, source)?;
            }
        }
        self.translate(instance, actions)
    }

    fn advance_inner(&mut self) -> Result<Option<Yield>, Error> {
        if self.waiting {
            return Err(Error::Invalid);
        }
        while let Some(call) = self.machine.advance()? {
            let actions = if let Notification::Invoked { token } = call.notification {
                if call.observer != 0 {
                    return Err(Error::Invalid);
                }
                match self.pending.remove(&token).ok_or(Error::Invalid)? {
                    Pending::Return(token) => {
                        if !self.calls.remove(&token) {
                            return Err(Error::Invalid);
                        }
                        self.waiting = true;
                        return Ok(Some(Yield::Returned(token)));
                    }
                    Pending::Publication(effect) => {
                        if self.effects.len() >= states::MAX_STEPS {
                            return Err(Error::Bound);
                        }
                        self.effects.push(effect.clone());
                        self.waiting = true;
                        return Ok(Some(Yield::Effect(Box::new(effect))));
                    }
                    Pending::Command(Scoped { instance, value }) => {
                        let scope = self
                            .world
                            .scopes
                            .get_mut(&instance)
                            .ok_or(Error::UnknownState)?;
                        let actions = scope.observers.command(&scope.plan, value)?;
                        self.local_actions(instance, actions)?
                    }
                    Pending::Local(instance, token) => {
                        let work = self
                            .world
                            .scopes
                            .get_mut(&instance)
                            .ok_or(Error::UnknownState)?
                            .observers
                            .pending
                            .remove(&token)
                            .ok_or(Error::Invalid)?;
                        self.work(instance, work)?
                    }
                    Pending::Dispatch(event, index) => {
                        let source = mapping::source(event)?;
                        if let Some(listener) = self
                            .world
                            .listeners
                            .get_mut(&source)
                            .ok_or(Error::UnknownState)?
                            .next(index)
                        {
                            let actions = self.observe(listener, event)?;
                            let Action::Invoke { token, observer } =
                                self.enqueue(Pending::Dispatch(event, index + 1))?
                            else {
                                unreachable!()
                            };
                            // The next vector slot is an iteration continuation,
                            // not another nested native callback. Keep the state
                            // guard while allowing a large bounded subscriber list.
                            self.machine.interject(
                                actions,
                                states::execution::Call {
                                    notification: Notification::Invoked { token },
                                    observer,
                                },
                            )?;
                            continue;
                        } else {
                            vec![self.enqueue(Pending::Hook(event))?]
                        }
                    }
                    Pending::Hook(event) => {
                        let owner = self.world.owners[&mapping::source(event)?];
                        let local = self.world.scopes[&owner.instance].notification(event)?;
                        let mut actions =
                            vec![self.publication(owner.instance, Effect::Hook(local), event)?];
                        let work = self
                            .world
                            .scopes
                            .get_mut(&owner.instance)
                            .ok_or(Error::UnknownState)?
                            .observers
                            .enqueue([Work::StateHook(local)])?;
                        actions.extend(self.local_actions(owner.instance, work)?);
                        actions
                    }
                }
            } else if call.observer != 0 {
                let listener = self
                    .world
                    .seeds
                    .get(call.observer)
                    .copied()
                    .flatten()
                    .ok_or(Error::Invalid)?;
                if !matches!(call.notification, Notification::Existing { .. }) {
                    return Err(Error::Invalid);
                }
                self.observe(listener, call.notification)?
            } else {
                let event = call.notification;
                let source = mapping::source(event)?;
                let owner = self.world.owners[&source];
                let local = self.world.scopes[&owner.instance].notification(event)?;
                self.world
                    .listeners
                    .get_mut(&source)
                    .ok_or(Error::UnknownState)?
                    .notifying = true;
                vec![
                    self.publication(owner.instance, Effect::State(local), event)?,
                    self.enqueue(Pending::Dispatch(event, 0))?,
                ]
            };
            self.machine.resume(actions)?;
        }
        Ok(None)
    }
}

const PUBLICATION: u64 = 1 << 63;
fn tag(action: Action) -> Action {
    match action {
        Action::Invoke { observer, token } => Action::Invoke {
            observer,
            token: token | PUBLICATION,
        },
        _ => unreachable!(),
    }
}
fn enqueue(
    pending: &mut BTreeMap<u64, Pending>,
    serial: &mut u64,
    item: Pending,
) -> Result<Action, Error> {
    if pending.len() >= states::MAX_PENDING || *serial >= states::MAX_STEPS as u64 {
        return Err(Error::Bound);
    }
    *serial += 1;
    pending.insert(*serial, item);
    Ok(Action::Invoke {
        observer: 0,
        token: *serial,
    })
}
