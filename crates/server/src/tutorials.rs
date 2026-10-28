// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tutorial logics per participant, from the installed spawners of the
//! gameplay scene's tutorial layer. A tutorial is live while it is not seen
//! and its gate holds; the host creates it then and deletes it once either
//! stops (official E1026: 32 ungated at the participant's logic batch,
//! remindertoclub with the drive-to-club mode and deleted when its objective
//! ends, earningreputation and remindertogarage with the return mode, the
//! phone tutorials 40 s and 90 s later, nine more 5 s after the intro
//! completes). The client accepts a prompt with the logic's EventSync id;
//! the seen value is committed first, then the logic is deleted and a
//! notified seen value is sent (E1026 579.192 s accept, 579.322 s delete,
//! 579.421 s s135 m0). A server context reporter's rows fire as the
//! participant's held mode categories change (race 1 574.121 s, row [3]).
//! The client decides when a passive tutorial prompts.
use crate::prologue::membership::presentation::Definition;
use nfs_content::tutorials::{
    Gate, Tutorial,
    gates::{Rule, Term, Trigger},
};
use nfs_world::{
    logic::{EntityRef, Fire},
    participants::{Endpoint, HostRpc},
    replication::{Error, Initial, Record, players::Players, state::World},
};
use std::collections::{BTreeMap, BTreeSet};

/// RaceObjective outputs the mission chapters read (installed property ids:
/// introdrivetoclub #29 feeds both to the chapter interface).
pub const OBJECTIVE_ACTIVE: u32 = 1_010_522_579;
pub const OBJECTIVE_COMPLETED: u32 = 1_514_830_046;
/// The return prefab's gate output: it also feeds the intro logic's
/// networked LaunchGameMode row, so it rises when that mode launches
/// (E1026 891.324 s gate, 891.523 s `progression_intro e8 [7]`).
pub const MODE_LAUNCHED: u32 = 1_530_399_826;
const SCENE: &str = "levels/genesis01/gameplay";
const CHAPTER: &str = "gameplay/progression/missionchapters/intro";
const ACCEPT_ENTITY: u32 = 1;
const MAX_TRIGGERS: usize = 32;

#[derive(Clone, Debug)]
struct Entry {
    /// Durable identity: the name hash of the logic blueprint.
    key: u32,
    tutorial: Tutorial,
    spawn: Definition,
}

#[derive(Clone, Debug)]
pub struct Content {
    scene_key: u32,
    entries: Vec<Entry>,
    rules: BTreeMap<String, Vec<(usize, Option<f32>)>>,
    triggers: Vec<Trigger>,
}

impl Content {
    pub fn prepare(
        index: &mut nfs_content::assets::AssetIndex,
    ) -> Result<Self, nfs_content::Error> {
        let tutorials = index.tutorials()?;
        let rules = index.tutorial_gates(CHAPTER)?;
        let mut entries = Vec::with_capacity(tutorials.len());
        for tutorial in tutorials {
            let spawn = Definition::prepare(index, SCENE, &tutorial.blueprint)?;
            entries.push((tutorial, spawn));
        }
        Self::new(nfs_content::assets::scene_key(SCENE), entries, rules)
            .map_err(|e| nfs_content::Error::Content(format!("tutorials: {e:?}")))
    }

    pub(crate) fn new(
        scene_key: u32,
        tutorials: Vec<(Tutorial, Definition)>,
        rules: Vec<Rule>,
    ) -> Result<Self, Error> {
        let mut content = Self {
            scene_key,
            entries: vec![],
            rules: BTreeMap::new(),
            triggers: vec![],
        };
        for (tutorial, spawn) in tutorials {
            if !spawn.channels().contains(&(scene_key, tutorial.channel)) {
                return Err(Error::Shape);
            }
            let key = crate::persistent::key(&tutorial.blueprint);
            if content.entries.iter().any(|e| e.key == key) {
                return Err(Error::Shape);
            }
            content.entries.push(Entry {
                key,
                tutorial,
                spawn,
            });
        }
        for rule in rules {
            let mut terms = vec![];
            for term in rule.all {
                let (trigger, seconds) = match term {
                    Term::Since { trigger, seconds } => (trigger, Some(seconds)),
                    Term::While(trigger) => (trigger, None),
                };
                match (&trigger, seconds) {
                    (Trigger::PrefabEvent { event, .. }, Some(_)) if *event == MODE_LAUNCHED => {}
                    (Trigger::LauncherStarted { .. }, Some(_)) => {}
                    (Trigger::PrefabProperty { property, .. }, Some(_))
                        if *property == OBJECTIVE_COMPLETED => {}
                    (
                        Trigger::PrefabProperty { property, .. }
                        | Trigger::Objective { property, .. },
                        None,
                    ) if *property == OBJECTIVE_ACTIVE => {}
                    _ => return Err(Error::Unsupported),
                }
                let index = match content.triggers.iter().position(|t| *t == trigger) {
                    Some(i) => i,
                    None if content.triggers.len() < MAX_TRIGGERS => {
                        content.triggers.push(trigger);
                        content.triggers.len() - 1
                    }
                    None => return Err(Error::Bound),
                };
                terms.push((index, seconds));
            }
            content.rules.insert(rule.gate, terms);
        }
        Ok(content)
    }

    /// Whether the world holds the gameplay scene the logics bind.
    pub fn ready(&self, world: &World) -> bool {
        world.scene(self.scene_key).is_some()
    }

    pub fn assets(
        &self,
    ) -> impl Iterator<Item = nfs_world::replication::entity::creation::Asset> + '_ {
        self.entries.iter().map(|e| e.spawn.asset())
    }

    fn endpoint(&self, world: &World, serializer: usize) -> Result<Endpoint, Error> {
        let scene = world.scene(self.scene_key).ok_or(Error::UnknownObject)?;
        let Some(Initial::SubLevel { fields, .. }) = world.get(scene).map(|o| &o.initial) else {
            return Err(Error::TypeMismatch);
        };
        let rpc = fields
            .get(serializer)
            .and_then(nfs_world::replication::sublevel::Initial::rpc)
            .ok_or(Error::TypeMismatch)?;
        Ok(Endpoint {
            scene,
            selector: rpc.selector,
            serial: rpc.serial,
        })
    }

    fn notification(
        &self,
        world: &World,
        entry: &Entry,
        seen: bool,
    ) -> Result<Option<HostRpc>, Error> {
        let Some(serializer) = entry.tutorial.notification else {
            return Ok(None);
        };
        Ok(Some(HostRpc::ModeNotification(
            nfs_world::garage::presence::Notification {
                endpoint: self.endpoint(world, serializer)?,
                enabled: seen,
            },
        )))
    }

    /// The participant's initial notification flush: every notified seen
    /// value, unchanged ones included (E1026 fresh account: s164, s135 m1 at
    /// 36.220 s, s134, s162 m1 at 36.319 s; E912: s135, s134 m0).
    pub fn notifications(
        &self,
        world: &World,
        seen: &BTreeSet<u32>,
    ) -> Result<Vec<HostRpc>, Error> {
        let mut out = vec![];
        for entry in &self.entries {
            out.extend(self.notification(world, entry, seen.contains(&entry.key))?);
        }
        Ok(out)
    }

    pub fn session(&self, participant: u16, seen: BTreeSet<u32>) -> Session {
        Session {
            participant,
            seen,
            rises: BTreeMap::new(),
            holding: BTreeSet::new(),
            locations: BTreeSet::new(),
            live: BTreeMap::new(),
            accepting: BTreeSet::new(),
            context: BTreeMap::new(),
            notify: vec![],
            driven: 0.0,
            watches: BTreeMap::new(),
        }
    }

    fn holds(&self, session: &Session, entry: &Entry, seconds: f64) -> bool {
        match &entry.tutorial.gate {
            Gate::None => true,
            Gate::Location(volume) => session.locations.contains(volume),
            Gate::Identified(asset) => self.rules.get(asset).is_some_and(|terms| {
                terms.iter().all(|&(trigger, delay)| match delay {
                    Some(delay) => session
                        .rises
                        .get(&trigger)
                        .is_some_and(|&at| seconds >= at + f64::from(delay)),
                    None => session.holding.contains(&trigger),
                })
            }),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub records: Vec<Record>,
    pub deleted: Vec<u16>,
    pub calls: Vec<HostRpc>,
    /// Seen values a server check set (accelerate): commit like an accept.
    pub commits: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    participant: u16,
    seen: BTreeSet<u32>,
    /// First rise of each latched trigger (world seconds).
    rises: BTreeMap<usize, f64>,
    /// Triggers whose value is now true.
    holding: BTreeSet<usize>,
    /// Location volumes the participant is inside.
    locations: BTreeSet<nfs_content::tutorials::Volume>,
    /// Entry index to its logic ghost.
    live: BTreeMap<usize, u16>,
    /// Accepted entries whose seen commit is outstanding.
    accepting: BTreeSet<usize>,
    /// Last evaluated context reporter value of each live entry.
    context: BTreeMap<usize, bool>,
    /// Committed seen values to notify at the next advance.
    notify: Vec<usize>,
    /// Metres driven in this session.
    driven: f64,
    /// Distance checks: entry index to (watch start, finished).
    watches: BTreeMap<usize, (f64, bool)>,
}

impl Session {
    pub fn seen(&self) -> &BTreeSet<u32> {
        &self.seen
    }
    /// The live tutorial logic ghosts.
    pub fn ghosts(&self) -> impl Iterator<Item = u16> + '_ {
        self.live.values().copied()
    }
    fn rise(&mut self, content: &Content, matches: impl Fn(&Trigger) -> bool, seconds: f64) {
        for (index, trigger) in content.triggers.iter().enumerate() {
            if matches(trigger) {
                self.rises.entry(index).or_insert(seconds);
            }
        }
    }
    /// The mode of `prefab` launched (its launcher started).
    pub fn launched(&mut self, content: &Content, prefab: &str, seconds: f64) {
        self.rise(
            content,
            |t| match t {
                Trigger::PrefabEvent { prefab: p, event } => p == prefab && *event == MODE_LAUNCHED,
                Trigger::LauncherStarted { prefab: p } => p == prefab,
                _ => false,
            },
            seconds,
        );
    }
    /// The committed objectives: `state` answers (active, completed) per
    /// stable objective id. An active objective holds its While terms; a
    /// completion's first observation is its rise (a restored completion
    /// rises at the session's first pass, E912 35.636 s).
    pub fn objectives(
        &mut self,
        content: &Content,
        state: impl Fn(u64) -> (bool, bool),
        seconds: f64,
    ) {
        for (index, trigger) in content.triggers.iter().enumerate() {
            match *trigger {
                Trigger::PrefabProperty {
                    property,
                    objective,
                    ..
                }
                | Trigger::Objective {
                    property,
                    objective,
                    ..
                } if property == OBJECTIVE_ACTIVE => {
                    if state(objective).0 {
                        self.holding.insert(index);
                    } else {
                        self.holding.remove(&index);
                    }
                }
                Trigger::PrefabProperty {
                    property,
                    objective,
                    ..
                } if property == OBJECTIVE_COMPLETED && state(objective).1 => {
                    self.rises.entry(index).or_insert(seconds);
                }
                _ => {}
            }
        }
    }
    /// Distance the participant's car covered (metres, non-negative).
    pub fn drove(&mut self, metres: f64) {
        if metres.is_finite() && metres > 0.0 {
            self.driven += metres;
        }
    }
    /// A location volume crossing of this participant.
    pub fn location(&mut self, volume: nfs_content::tutorials::Volume, inside: bool) {
        if inside {
            self.locations.insert(volume);
        } else {
            self.locations.remove(&volume);
        }
    }
    /// Leave every location volume `keep` rejects (the membership went).
    pub fn keep_locations(&mut self, keep: impl Fn(nfs_content::tutorials::Volume) -> bool) {
        self.locations.retain(|v| keep(*v));
    }
    /// The location volumes a tutorial gate tests.
    pub fn watches(content: &Content, volume: nfs_content::tutorials::Volume) -> bool {
        content
            .entries
            .iter()
            .any(|e| e.tutorial.gate == Gate::Location(volume))
    }

    /// Create and delete logics so the live set matches the gates, fire
    /// context rows on changes and send committed seen notifications.
    pub fn advance(
        &mut self,
        content: &Content,
        players: &mut Players,
        seconds: f64,
        held: &[&BTreeSet<u32>],
    ) -> Result<Output, Error> {
        let mut out = Output::default();
        for index in std::mem::take(&mut self.notify) {
            out.calls.extend(content.notification(
                players.objects(),
                &content.entries[index],
                true,
            )?);
        }
        for (index, entry) in content.entries.iter().enumerate() {
            // A logic waits until every scene its references bind exists.
            let wanted = !self.seen.contains(&entry.key)
                && !self.accepting.contains(&index)
                && content.holds(self, entry, seconds)
                && entry
                    .spawn
                    .channels()
                    .iter()
                    .all(|&(key, _)| players.objects().scene(key).is_some());
            match (wanted, self.live.get(&index).copied()) {
                (true, None) => {
                    let record = entry.spawn.spawn(players, self.participant)?;
                    self.live.insert(index, record.id);
                    out.records.push(record);
                }
                (false, Some(ghost)) if !self.accepting.contains(&index) => {
                    players.remove_entities(&[ghost])?;
                    self.live.remove(&index);
                    self.context.remove(&index);
                    out.deleted.push(ghost);
                }
                _ => {}
            }
        }
        // Distance checks: the watch starts at `start` metres and, when it
        // runs out, either sets the seen value or fires the prompt rows.
        for (&index, &ghost) in &self.live {
            let entry = &content.entries[index];
            let Some(check) = &entry.tutorial.distance else {
                continue;
            };
            let watch = match self.watches.get(&index).copied() {
                Some(watch) => watch,
                None if self.driven >= f64::from(check.start) => {
                    self.watches.insert(index, (seconds, false));
                    (seconds, false)
                }
                None => continue,
            };
            if watch.1 || seconds < watch.0 + f64::from(check.millis) / 1000.0 {
                continue;
            }
            self.watches.insert(index, (watch.0, true));
            if self.driven >= f64::from(check.enough) {
                // Deleted by the commit: the cancel row is never sent
                // (official E1026 146.119 s, a delete without a fire).
                if self.accepting.insert(index) {
                    out.commits.push(entry.key);
                }
            } else {
                out.calls.extend(check.prompt.iter().map(|&(bus, ordinal)| {
                    HostRpc::Event(Fire {
                        target: EntityRef { ghost, entity: bus },
                        event: ordinal,
                        player: None,
                    })
                }));
            }
        }
        for (&index, &ghost) in &self.live {
            let Some(context) = &content.entries[index].tutorial.context else {
                continue;
            };
            let value = held.iter().any(|c| !c.is_disjoint(&context.categories));
            let previous = self.context.insert(index, value).unwrap_or(false);
            if value == previous {
                continue;
            }
            let rows = if value {
                &context.on_true
            } else {
                &context.on_false
            };
            out.calls.extend(rows.iter().map(|&(bus, ordinal)| {
                HostRpc::Event(Fire {
                    target: EntityRef { ghost, entity: bus },
                    event: ordinal,
                    player: None,
                })
            }));
        }
        Ok(out)
    }

    /// A client accept: the durable key to commit, when it names one of this
    /// participant's live tutorials with its EventSync id, the logic's
    /// entity and the local player. `Ok(None)` when the ghost is no tutorial.
    pub fn accept(
        &mut self,
        content: &Content,
        event: u32,
        target: EntityRef,
        player: u8,
    ) -> Result<Option<u32>, Error> {
        let Some((&index, _)) = self.live.iter().find(|(_, g)| **g == target.ghost) else {
            return Ok(None);
        };
        let entry = &content.entries[index];
        if entry.tutorial.accept != Some(event) || target.entity != ACCEPT_ENTITY || player != 0 {
            return Err(Error::Unsupported);
        }
        if self.accepting.insert(index) {
            Ok(Some(entry.key))
        } else {
            // A repeated accept while the commit is outstanding.
            Ok(None)
        }
    }

    /// The seen value committed: the logic goes and its notification follows
    /// at the next advance.
    pub fn committed(&mut self, content: &Content, key: u32) {
        let Some(index) = content.entries.iter().position(|e| e.key == key) else {
            return;
        };
        self.accepting.remove(&index);
        if self.seen.insert(key) && content.entries[index].tutorial.notification.is_some() {
            self.notify.push(index);
        }
    }

    /// The commit failed: the logic stays live for another accept.
    pub fn failed(&mut self, content: &Content, key: u32) {
        if let Some(index) = content.entries.iter().position(|e| e.key == key) {
            self.accepting.remove(&index);
        }
    }

    pub fn owns(&self, ghost: u16) -> bool {
        self.live.values().any(|g| *g == ghost)
    }
}

#[cfg(test)]
mod tests;
