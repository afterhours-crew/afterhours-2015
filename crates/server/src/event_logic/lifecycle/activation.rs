// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// Snapshot of already existing scene inputs when an event is constructed.
/// Imported state membership is a snapshot, not another entry or RPC. The
/// enclosing scene remains responsible for applying subsequent state effects.
#[derive(Clone, Debug, Default)]
pub struct SceneInputs {
    pub clocks: Option<timed::Clocks>,
    /// (state alias, participant, already completed).
    pub members: Vec<(StateId, Participant, bool)>,
    pub booleans: BTreeMap<(Property, Participant), bool>,
    pub integers: BTreeMap<(IntegerSource, Participant), i32>,
    pub floats: BTreeMap<(Property, Participant), f32>,
    pub sets: BTreeMap<Property, BTreeSet<Participant>>,
    pub players: BTreeMap<Property, BTreeSet<Participant>>,
    pub ai: BTreeMap<Property, BTreeSet<Participant>>,
}

impl Host {
    /// Bind set predicates, then activate observers in authored order. Native
    /// state lists/ranges enumerate under each source's guard; contexts do not
    /// seed, and Boolean predicates do not evaluate merely upon subscription.
    pub fn from_scene(
        plan: Arc<Plan>,
        scene: SceneInputs,
        callback: &mut Callback<'_>,
    ) -> Result<(Self, Vec<Effect>), Error> {
        Self::from_scene_queued(plan, scene, callback, None)
    }

    pub fn from_scene_with_commands(
        plan: Arc<Plan>,
        scene: SceneInputs,
        callback: &mut commands::Callback<'_>,
    ) -> Result<(Self, Vec<Effect>), Error> {
        let queue = std::cell::RefCell::new(commands::Queue::default());
        Self::from_scene_queued(
            plan,
            scene,
            &mut |machine, effect| queue.borrow_mut().expand(callback(machine, effect)?),
            Some(&queue),
        )
    }

    fn from_scene_queued(
        plan: Arc<Plan>,
        scene: SceneInputs,
        callback: &mut Callback<'_>,
        commands: Option<&std::cell::RefCell<commands::Queue>>,
    ) -> Result<(Self, Vec<Effect>), Error> {
        let mut host = Self::prepare(plan, scene)?;
        let order = host.plan.data.bindings.order.clone();
        let work = order
            .iter()
            .copied()
            .map(|observer| Work::Activate {
                observer,
                binding: true,
            })
            .chain(order.iter().copied().map(|observer| Work::Activate {
                observer,
                binding: false,
            }));
        let actions = host.observers.enqueue(work)?;
        let effects = host.execute_queued(&actions, callback, commands)?;
        Ok((host, effects))
    }

    /// Import existing scene state without activating mode observers. The
    /// enclosing graph sends Bind/Activate commands in its own creation order.
    pub fn prepare(plan: Arc<Plan>, scene: SceneInputs) -> Result<Self, Error> {
        if scene.members.len() > states::MAX_PENDING
            || scene.booleans.len() > states::MAX_PENDING
            || scene.integers.len() > states::MAX_PENDING
            || scene.floats.len() > states::MAX_PENDING
        {
            return Err(Error::Bound);
        }
        let mut host = Self::new(plan)?;
        if let Some(clocks) = scene.clocks {
            host.clocks(clocks)?;
        }
        host.observers.enabled.clear();
        host.observers.rankings.clear_listeners();
        host.observers.boolean_listeners.clear();
        host.observers.set_listeners.clear();
        let mut actions = Vec::new();
        for (state, participant, completed) in scene.members {
            if !host.plan.external.contains_key(&state) {
                return Err(Error::UnknownState);
            }
            actions.push(Action::Enter { state, participant });
            if completed {
                actions.push(Action::Complete { state, participant });
            }
        }
        host.machine.run(&actions, |_, _| Ok(vec![]))?;
        for (key, _) in scene.booleans.keys() {
            key.validate().map_err(|_| Error::Invalid)?;
        }
        validate_sets(&scene.sets, 256)?;
        validate_sets(&scene.players, 128)?;
        validate_sets(&scene.ai, 128)?;
        host.observers.booleans = scene.booleans;
        for (source, _) in scene.integers.keys() {
            host.plan.validate_integer(source)?;
        }
        host.observers.rankings.values = scene.integers;
        for ((key, _), value) in &scene.floats {
            key.validate().map_err(|_| Error::Invalid)?;
            if !value.is_finite() {
                return Err(Error::Invalid);
            }
        }
        host.observers.floats.values = scene.floats;
        host.observers.channel_sets = scene.sets;
        host.observers.players = scene.players;
        host.observers.ai = scene.ai;

        Ok(host)
    }
}

impl Observers {
    pub(super) fn activate_observer(
        &mut self,
        plan: &Plan,
        observer: Observer,
        binding: bool,
    ) -> Result<Vec<Action>, Error> {
        if !plan.data.bindings.order.contains(&observer) {
            return Err(Error::Invalid);
        }
        let at_bind = matches!(observer, Observer::Set(i)
            if matches!(plan.data.bindings.sets[i].kind, SetKind::Predicate { .. }));
        if binding != at_bind || !self.enabled.insert(observer) {
            return Ok(vec![]);
        }
        if let Observer::Sorter(i) = observer {
            self.rankings.subscribe(
                plan.data.bindings.sorters[i].source.clone(),
                rankings::Listener::Sorter(i),
            )?;
        }
        self.boolean_listeners.activate(plan, observer)?;
        self.set_listeners.activate(plan, observer)?;
        let sources = match observer {
            Observer::Ai(i) if plan.data.bindings.ai_controls[i].apply_existing => {
                plan.data.bindings.ai_controls[i]
                    .states
                    .iter()
                    .map(|s| s.serializer as StateId)
                    .collect()
            }
            Observer::Property(i) => self.properties[i].seed_sources(),
            Observer::Set(i) => match &plan.data.bindings.sets[i].kind {
                SetKind::States(sources) => {
                    sources.iter().map(|s| s.serializer as StateId).collect()
                }
                _ => vec![],
            },
            Observer::Transition(id) => {
                let transition = plan
                    .data
                    .transitions
                    .iter()
                    .find(|t| t.serializer == id)
                    .ok_or(Error::Invalid)?;
                if matches!(
                    transition.kind,
                    data::TransitionKind::Dnf | data::TransitionKind::DnfTriggered
                ) {
                    transition
                        .sources
                        .iter()
                        .map(|s| s.serializer as StateId)
                        .collect()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        };
        if sources.is_empty() {
            let work = self.seed_work(plan, observer)?;
            return self.enqueue(work);
        }
        if self.registering.insert(observer, BTreeSet::new()).is_some() {
            return Err(Error::Invalid);
        }
        self.enqueue([Work::RegisterSource {
            observer,
            sources,
            next: 0,
        }])
    }

    pub(super) fn register_source(
        &mut self,
        plan: &Plan,
        observer: Observer,
        sources: Vec<StateId>,
        next: usize,
    ) -> Result<Vec<Action>, Error> {
        let registered = self.registering.get_mut(&observer).ok_or(Error::Invalid)?;
        let Some(&state) = sources.get(next) else {
            if next != sources.len() {
                return Err(Error::Invalid);
            }
            self.registering.remove(&observer);
            let work = self.seed_work(plan, observer)?;
            return self.enqueue(work);
        };
        registered.insert(state);
        let index = plan
            .data
            .bindings
            .order
            .iter()
            .position(|o| *o == observer)
            .ok_or(Error::Invalid)?;
        let mut actions = vec![Action::SeedMembers {
            state,
            observer: index + 1,
        }];
        actions.extend(self.enqueue([Work::RegisterSource {
            observer,
            sources,
            next: next + 1,
        }])?);
        Ok(actions)
    }

    fn seed_work(&self, plan: &Plan, observer: Observer) -> Result<Vec<Work>, Error> {
        let mut work = Vec::new();
        match observer {
            Observer::Distance(i) => work.push(Work::Distance(distance::Effect {
                identity: plan.data.bindings.distances[i].identity.clone(),
                total_metres: 0.,
            })),
            Observer::Mirror(i) => work.push(Work::Mirror(mirrors::Work::Activate(i))),
            Observer::MirrorProvider(i) => {
                let provider = &plan.data.bindings.mirror_providers[i];
                if let (Some(manager), Some(source)) = (&provider.manager, &provider.source) {
                    work.push(Work::Mirror(mirrors::Work::Register(mirrors::Provider {
                        id: mirrors::ProviderId {
                            instance: 0,
                            identity: provider.identity.clone(),
                        },
                        manager: manager.clone(),
                        identifier: provider.identifier,
                        source: source.clone(),
                    })));
                }
            }
            Observer::Reporter(i) => work.push(Work::Reporter(reporters::Work::Count(i))),
            Observer::SetCounter(i) => work.push(Work::SetCount(i)),
            Observer::Set(i) => match &plan.data.bindings.sets[i].kind {
                SetKind::Boolean(link) => {
                    for participant in self.true_values(&plan.boolean_source(link)?) {
                        work.push(Work::AssignSet(i, participant, true));
                    }
                }
                SetKind::Players { manager } => {
                    for &participant in self.players.get(manager).into_iter().flatten() {
                        work.push(Work::AssignSet(i, participant, true));
                    }
                }
                SetKind::Predicate {
                    operation,
                    operands,
                } => {
                    let operands = operands
                        .iter()
                        .map(|s| self.source_members(plan, s).cloned().unwrap_or_default())
                        .collect::<Vec<_>>();
                    let desired =
                        nfs_world::scoring::participant_sets::evaluate(*operation, &operands)
                            .map_err(|_| Error::Bound)?;
                    for participant in desired {
                        work.push(Work::AssignSet(i, participant, true));
                    }
                }
                _ => {}
            },
            Observer::Spawner(i) => {
                let members = match &plan.data.bindings.spawners[i].controller {
                    Some(Controller::Boolean(link)) => {
                        self.true_values(&plan.boolean_source(link)?)
                    }
                    Some(Controller::Set(link)) => self
                        .source_members(plan, &plan.set_source(link)?)
                        .cloned()
                        .unwrap_or_default(),
                    None => BTreeSet::new(),
                };
                for participant in members {
                    work.push(Work::Spawner(i, participant, true));
                }
            }
            _ => {}
        }
        Ok(work)
    }

    pub(super) fn source_members(
        &self,
        plan: &Plan,
        source: &SetSource,
    ) -> Option<&BTreeSet<Participant>> {
        match source {
            SetSource::Local(id) => self.sets.get(plan.sets[id]),
            SetSource::Channel(key) => self.channel_sets.get(key),
        }
    }

    fn true_values(&self, source: &BooleanSource) -> BTreeSet<Participant> {
        match source {
            BooleanSource::Local(i) => self.properties[*i]
                .stored_values()
                .iter()
                .filter_map(|(&p, &v)| v.then_some(p))
                .collect(),
            BooleanSource::Predicate(i) => self.predicates[*i]
                .iter()
                .filter_map(|(&p, &v)| v.then_some(p))
                .collect(),
            BooleanSource::Channel(key) => self
                .booleans
                .iter()
                .filter_map(|((k, p), &v)| (k == key && v).then_some(*p))
                .collect(),
        }
    }
}

fn validate_sets(
    sets: &BTreeMap<Property, BTreeSet<Participant>>,
    max: usize,
) -> Result<(), Error> {
    if sets.len() > max || sets.values().any(|s| s.len() > states::MAX_PARTICIPANTS) {
        return Err(Error::Bound);
    }
    for key in sets.keys() {
        key.validate().map_err(|_| Error::Invalid)?;
    }
    Ok(())
}
