// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Session-owned state membership with per-instance authored aliases. Imported
//! scene states are bindings to their existing owner, never copied snapshots.
use super::*;
use commands::Command;
use states::Membership;

mod execution;
pub use execution::{Execution, Yield};
mod mapping;

const MAX_INSTANCES: usize = 256;

/// Non-reused lifetime identity. A restarted mode receives a new instance even
/// when it uses the same content and wire serializers as the previous mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Instance(u64);
impl Instance {
    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Address {
    pub instance: Instance,
    pub state: StateId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scoped<T> {
    pub instance: Instance,
    pub value: T,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Publication {
    pub instance: Instance,
    pub value: Effect,
    /// Actual destination for a state transition, including destinations owned
    /// by another scope. Local observer projections may legitimately omit it.
    pub transition_target: Option<Address>,
}

pub type Callback<'a> =
    dyn FnMut(&Execution, &Publication) -> Result<Vec<Scoped<Command>>, Error> + 'a;

#[derive(Clone, Debug)]
struct Scope {
    plan: Arc<Plan>,
    observers: Observers,
    local: BTreeMap<StateId, StateId>,
    reverse: BTreeMap<StateId, StateId>,
    seed_slots: Vec<usize>,
}

/// A borrowed alias table over the current transaction. Reads during nested
/// callbacks see the same candidate as the caller, including deferred moves.
pub struct View<'a> {
    machine: &'a Machine,
    local: &'a BTreeMap<StateId, StateId>,
}
impl Membership for View<'_> {
    fn members(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        self.machine
            .members(*self.local.get(&state).ok_or(Error::UnknownState)?)
    }
    fn completed(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        self.machine
            .completed(*self.local.get(&state).ok_or(Error::UnknownState)?)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Listener {
    instance: Instance,
    observer: Observer,
}

#[derive(Clone, Debug)]
pub struct WorldHost {
    machine: Machine,
    scopes: BTreeMap<Instance, Scope>,
    scenes: BTreeMap<u32, Instance>,
    owners: BTreeMap<StateId, Address>,
    listeners: BTreeMap<StateId, listeners::List<Listener>>,
    seeds: Vec<Option<Listener>>,
    next_instance: u64,
}
impl Default for WorldHost {
    fn default() -> Self {
        Self {
            machine: Machine::empty(),
            scopes: BTreeMap::new(),
            scenes: BTreeMap::new(),
            owners: BTreeMap::new(),
            listeners: BTreeMap::new(),
            seeds: vec![None],
            next_instance: 0,
        }
    }
}

impl WorldHost {
    /// Install an inactive scope. Binding imports requires their scene to be
    /// installed already. Activation remains an ordered graph callback.
    pub fn install(&mut self, plan: Arc<Plan>, scene_key: Option<u32>) -> Result<Instance, Error> {
        let mut next = self.clone();
        let id = next.install_inner(plan, scene_key)?;
        *self = next;
        Ok(id)
    }

    fn install_inner(
        &mut self,
        plan: Arc<Plan>,
        scene_key: Option<u32>,
    ) -> Result<Instance, Error> {
        if scene_key.is_some() && plan.root().is_some() {
            return Err(Error::Invalid);
        }
        if self.scopes.len() >= MAX_INSTANCES {
            return Err(Error::Bound);
        }
        if scene_key.is_some_and(|key| self.scenes.contains_key(&key)) {
            return Err(Error::Invalid);
        }
        let id = Instance(self.next_instance.checked_add(1).ok_or(Error::Bound)?);
        let mut local = BTreeMap::new();
        let mut definitions = Vec::new();
        let mut available = (0..=StateId::MAX).filter(|id| !self.owners.contains_key(id));
        for definition in &plan.data.states {
            let alias = StateId::try_from(definition.serializer).map_err(|_| Error::Bound)?;
            let state = if let Some(imported) = plan.external.get(&alias) {
                let owner = self
                    .scenes
                    .get(&imported.scene_content_key)
                    .ok_or(Error::UnknownState)?;
                let scope = &self.scopes[owner];
                let source =
                    StateId::try_from(imported.state.serializer).map_err(|_| Error::Bound)?;
                if scope.plan.external.contains_key(&source) {
                    return Err(Error::Invalid);
                }
                let original = scope
                    .plan
                    .data
                    .states
                    .iter()
                    .find(|s| s.serializer == imported.state.serializer)
                    .ok_or(Error::UnknownState)?;
                if original.handles_marked_for_delete != imported.state.handles_marked_for_delete
                    || original.send_rpc_to_client != imported.state.send_rpc_to_client
                {
                    return Err(Error::Invalid);
                }
                *scope.local.get(&source).ok_or(Error::UnknownState)?
            } else {
                let state = available.next().ok_or(Error::Bound)?;
                definitions.push(states::Definition {
                    id: state,
                    handles_marked_for_delete: definition.handles_marked_for_delete,
                    send_rpc_to_client: definition.send_rpc_to_client,
                });
                state
            };
            if local.insert(alias, state).is_some() {
                return Err(Error::Invalid);
            }
        }
        let reverse: BTreeMap<_, _> = local
            .iter()
            .map(|(&local, &global)| (global, local))
            .collect();
        if reverse.len() != local.len() {
            return Err(Error::Invalid);
        }
        self.machine.install(&definitions)?;
        for definition in definitions {
            self.owners.insert(
                definition.id,
                Address {
                    instance: id,
                    state: reverse[&definition.id],
                },
            );
            self.listeners
                .insert(definition.id, listeners::List::default());
        }
        // Reuse the observer constructors and validation, discarding their
        // empty standalone storage. No imported membership is accepted here.
        let Host { observers, .. } = Host::prepare(plan.clone(), SceneInputs::default())?;
        let mut seed_slots = Vec::new();
        for &observer in &plan.data.bindings.order {
            let slot = if let Some(i) = self
                .seeds
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(i, value)| value.is_none().then_some(i))
            {
                i
            } else {
                if self.seeds.len() >= states::MAX_PENDING {
                    return Err(Error::Bound);
                }
                self.seeds.push(None);
                self.seeds.len() - 1
            };
            self.seeds[slot] = Some(Listener {
                instance: id,
                observer,
            });
            seed_slots.push(slot);
        }
        self.scopes.insert(
            id,
            Scope {
                plan,
                observers,
                local,
                reverse,
                seed_slots,
            },
        );
        if let Some(key) = scene_key {
            self.scenes.insert(key, id);
        }
        self.next_instance = id.0;
        Ok(id)
    }

    pub fn view(&self, instance: Instance) -> Result<View<'_>, Error> {
        let scope = self.scopes.get(&instance).ok_or(Error::UnknownState)?;
        Ok(View {
            machine: &self.machine,
            local: &scope.local,
        })
    }
    pub fn values(&self, instance: u64) -> Result<values::Values<'_>, Error> {
        let scope = self
            .scopes
            .get(&Instance(instance))
            .ok_or(Error::UnknownState)?;
        Ok(values::Values {
            plan: &scope.plan,
            observers: &scope.observers,
        })
    }
    pub fn plan(&self, instance: Instance) -> Result<&Plan, Error> {
        Ok(&self.scopes.get(&instance).ok_or(Error::UnknownState)?.plan)
    }
    pub fn boolean(
        &self,
        instance: Instance,
        source: &BooleanSource,
        participant: Participant,
    ) -> Result<Option<bool>, Error> {
        self.values(instance.value())?.boolean(source, participant)
    }

    /// Retire storage after graph deactivation and source-destruction callbacks.
    /// Imported users and occupied states prevent premature scene destruction.
    pub fn retire(&mut self, instance: Instance) -> Result<(), Error> {
        let scope = self.scopes.get(&instance).ok_or(Error::UnknownState)?;
        let owned: Vec<_> = self
            .owners
            .iter()
            .filter_map(|(&id, owner)| (owner.instance == instance).then_some(id))
            .collect();
        if self.scopes.iter().any(|(&id, scope)| {
            id != instance && owned.iter().any(|state| scope.reverse.contains_key(state))
        }) {
            return Err(Error::Invalid);
        }
        if !scope.observers.pending.is_empty() || !scope.observers.registering.is_empty() {
            return Err(Error::Invalid);
        }
        self.machine.retire(&owned)?;
        for list in self.listeners.values_mut() {
            let remove: Vec<_> = list
                .items
                .iter()
                .flatten()
                .copied()
                .filter(|l| l.instance == instance)
                .collect();
            for listener in remove {
                list.remove(listener);
            }
        }
        for id in owned {
            self.owners.remove(&id);
            self.listeners.remove(&id);
        }
        for &slot in &scope.seed_slots {
            self.seeds[slot] = None;
        }
        self.scopes.remove(&instance);
        self.scenes.retain(|_, id| *id != instance);
        Ok(())
    }

    pub fn dispatch(
        &mut self,
        commands: Vec<Scoped<Command>>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Publication>, Error> {
        let mut execution = self.begin(commands)?;
        while let Some(yielded) = execution.advance()? {
            match yielded {
                Yield::Effect(effect) => {
                    let commands = callback(&execution, &effect)?;
                    execution.resume(commands)?;
                }
                Yield::Returned(_) => return Err(Error::Invalid),
            }
        }
        let (next, effects) = execution.finish()?;
        *self = next;
        Ok(effects)
    }
}

fn watched(plan: &Plan, observer: Observer) -> Vec<StateId> {
    let endpoints: Vec<data::Endpoint> = match observer {
        Observer::Property(i) => match &plan.data.bindings.properties[i].definition {
            properties::Definition::List(states) => states.clone(),
            properties::Definition::Range { begin, end } => vec![*begin, *end],
        },
        Observer::Context(i) => plan.data.bindings.contexts[i].states.clone(),
        Observer::Ai(i) => plan.data.bindings.ai_controls[i].states.clone(),
        Observer::Reporter(i) => plan.data.bindings.reporters[i].states.clone(),
        Observer::Set(i) => match &plan.data.bindings.sets[i].kind {
            SetKind::States(states) => states.clone(),
            _ => vec![],
        },
        Observer::Transition(id) => plan
            .data
            .transitions
            .iter()
            .find(|t| t.serializer == id)
            .map(|t| t.sources.clone())
            .unwrap_or_default(),
        _ => vec![],
    };
    endpoints
        .into_iter()
        .map(|e| e.serializer as StateId)
        .collect()
}

#[cfg(test)]
mod tests;
