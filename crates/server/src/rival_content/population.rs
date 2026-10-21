// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Session-owned roaming rivals, simulated by the host like the official
//! server (E986/E987). An active racer keeps one participant and actor (a
//! slot) while the host moves it along the road graph with the native wander
//! rules. Within [`HANDOVER_IN`] of a focus the host creates a car for it and
//! the client drives it; beyond [`HANDOVER_OUT`] the car is removed and the
//! host continues from the car's last reported pose.
use super::Content;
use crate::{
    traffic_content::population::RIVAL_KEY_START,
    vehicle_content::{self, registrations},
};
use nfs_world::{
    content::{Message, bindings::Bindings},
    participants::HostRpc,
    replication::{
        Error, Record,
        players::{Players, ai},
        vehicle,
    },
    road_graph::Graph,
    road_wander::{Flags, NativeRandom, Wander},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub use nfs_world::road_wander::WANDER_SPEED;

/// A rival participant and its world position for the rival map.
pub type Location = (u16, [f32; 3]);

/// Native roaming-manager retention: a Genesis view's outer radius, measured
/// horizontally (E986).
const VIEW: f32 = 400.;
/// Farthest observed first appearance of an official rival car (E986). Like
/// the removal, measured in 3D so a car never qualifies for both at once.
pub const HANDOVER_IN: f32 = 790.;
/// The native AI director removes a delegated car whose 3D distance from its
/// player exceeds this (`04616330`: x, y and z differences squared and summed
/// against 640,000; E987, E990).
pub const HANDOVER_OUT: f32 = 800.;
/// Official rival cars carry the WanderGoal flags (random offset only).
const GOAL: [bool; 3] = [false, false, true];
/// Official AI cars are created a quarter of the road width from the road's
/// centre line, on the side opposite to up x forward: the driving lane
/// (E990: 168 of 186 official traffic creations; rivals at the same distance).
const LANE_OFFSET: f32 = -0.25;
/// A stalled tick never moves the host simulation farther than this.
const MAX_STEP_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug)]
pub struct Focus {
    pub id: u16,
    pub position: [f32; 3],
}
pub struct Input<'a> {
    pub now: u64,
    pub focus: &'a [Focus],
    /// Fresh host randomness; seeds the wander of racers activated this tick.
    pub random: u32,
    /// Participants still owned by an active game mode's preserve property.
    pub preserved: &'a [u16],
}

#[derive(Clone, Debug, PartialEq)]
struct Active {
    /// Spawn key of the persistent participant and actor.
    slot: u64,
    /// One car model per participant for its whole activation.
    persona: usize,
    /// Host simulation; frozen while a client drives the car.
    wander: Wander,
    car: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Racer {
    /// Retained between the native ten membership-query cohorts.
    region_active: bool,
    persona_cursor: usize,
    active: Option<Active>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Adopted {
    slot: u64,
    car: u64,
    ids: ai::Ids,
}

#[derive(Clone, Debug)]
pub struct Population {
    content: Arc<Content>,
    graph: Arc<Graph>,
    racers: Vec<Racer>,
    /// Event imports have no authored regions and never respawn.
    adopted: BTreeMap<u64, Adopted>,
    authority: vehicle::authority::Registry,
    /// Next car spawn key.
    next: u64,
    /// Next slot key; slots and cars are separate world namespaces.
    slots: u64,
    cohort: usize,
    clock: Option<u64>,
    owner: Option<(u8, u64)>,
    /// Player-chosen cap on racers active at once; `None` keeps the original
    /// region-driven number.
    limit: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Changes {
    /// Alive and reset-volume leaves of retired slots, before their deletion.
    pub leave: Vec<HostRpc>,
    pub messages: Vec<Message>,
    pub records: Vec<Record>,
    /// Manager bindings and reset-volume changes, after the records.
    pub calls: Vec<HostRpc>,
    pub deleted: Vec<u16>,
    /// The slot activated this tick.
    pub activated: Option<ai::SlotIds>,
    /// The car handed to the client this tick.
    pub spawned: Option<ai::Ids>,
}

impl Population {
    pub fn new(bindings: &Bindings, content: Arc<Content>) -> Result<Self, Error> {
        if bindings
            .root_child(content.data.managers.scene)
            .map_err(|_| Error::Shape)?
            .id
            >= 2048
        {
            return Err(Error::Bound);
        }
        let graph = content.graph.clone().ok_or(Error::Unsupported)?;
        Ok(Self {
            racers: vec![Racer::default(); content.data.racers.len()],
            adopted: BTreeMap::new(),
            content,
            graph,
            authority: vehicle::authority::Registry::default(),
            next: RIVAL_KEY_START,
            slots: 1,
            cohort: 0,
            clock: None,
            owner: None,
            limit: None,
        })
    }

    /// Cap the racers active at once (0 turns roaming rivals off).
    pub fn with_limit(mut self, limit: Option<u16>) -> Self {
        self.limit = limit.map(usize::from);
        self
    }

    /// Active racers, with or without a car.
    pub fn count(&self) -> usize {
        self.racers.iter().filter(|r| r.active.is_some()).count() + self.adopted.len()
    }

    /// Cars currently driven by the client, by vehicle ghost.
    pub fn delegated(&self, world: &Players, connection: u8, persona: u64) -> BTreeMap<u16, u64> {
        if self.owner != Some((connection, persona)) {
            return BTreeMap::new();
        }
        self.racers
            .iter()
            .filter_map(|r| {
                let key = r.active.as_ref()?.car?;
                world
                    .ai_ids(connection, persona, key)
                    .map(|ids| (ids.vehicle, key))
            })
            .chain(self.adopted.values().filter_map(|r| {
                (world.ai_ids(connection, persona, r.car) == Some(r.ids))
                    .then_some((r.ids.vehicle, r.car))
            }))
            .collect()
    }

    /// Every active racer's participant and world position for the rival map:
    /// its car's while the client drives it, otherwise the host simulation's.
    pub fn locations(
        &self,
        world: &Players,
        connection: u8,
        persona: u64,
    ) -> Result<Vec<Location>, Error> {
        if self.owner != Some((connection, persona)) {
            return Ok(Vec::new());
        }
        self.racers
            .iter()
            .filter_map(|r| r.active.as_ref())
            .map(|active| {
                let slot = world
                    .ai_slot(connection, persona, active.slot)
                    .ok_or(Error::UnknownObject)?;
                Ok((
                    slot.participant,
                    self.position(world, connection, persona, active)?,
                ))
            })
            .chain(self.adopted.values().map(|r| {
                Ok((
                    r.ids.participant,
                    world
                        .ai_vehicle_physics(connection, persona, r.car)?
                        .decode([0.; 3])?
                        .position,
                ))
            }))
            .collect()
    }

    /// Adopt an event's existing participant, actor and car. Imported racers
    /// have view retention, not an authored spawn region. The caller stages the
    /// event and this population together before publishing their output.
    pub fn adopt(
        &mut self,
        world: &mut Players,
        connection: u8,
        persona: u64,
        member: crate::event_ai::Member,
        source: &mut vehicle::authority::Registry,
    ) -> Result<ai::GoalChanges, Error> {
        let key = member.selection.car_key;
        let slot = member.selection.slot_key;
        if self
            .owner
            .is_some_and(|owner| owner != (connection, persona))
            || world.ai_ids(connection, persona, key) != Some(member.ids)
            || !world.ai_slot(connection, persona, slot).is_some_and(|s| {
                s.participant == member.ids.participant
                    && s.actor == member.ids.actor
                    && s.car == Some(key)
            })
        {
            return Err(Error::UnknownObject);
        }
        let adopted = Adopted {
            slot,
            car: key,
            ids: member.ids,
        };
        if let Some(existing) = self.adopted.get(&key) {
            return if *existing == adopted {
                Ok(ai::GoalChanges::default())
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.adopted.len() >= ai::MAX_AI
            || self
                .racers
                .iter()
                .filter_map(|r| r.active.as_ref())
                .any(|r| r.slot == slot || r.car == Some(key))
        {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        let mut players = world.clone();
        let mut authority = source.clone();
        authority.transfer_group(&mut next.authority, key)?;
        let changes = players.replace_ai_goals(
            connection,
            persona,
            key,
            vec![(
                nfs_world::replication::Initial::WanderGoal(
                    nfs_world::replication::wander::Initial {
                        allocation_serial: 0,
                        flags: GOAL,
                    },
                ),
                nfs_world::replication::Update::WanderGoal,
            )],
            None,
        )?;
        next.adopted.insert(key, adopted);
        next.owner = Some((connection, persona));
        *self = next;
        *world = players;
        *source = authority;
        Ok(changes)
    }

    fn position(
        &self,
        world: &Players,
        connection: u8,
        persona: u64,
        active: &Active,
    ) -> Result<[f32; 3], Error> {
        Ok(match active.car {
            Some(car) => {
                world
                    .ai_vehicle_physics(connection, persona, car)?
                    .decode([0.; 3])?
                    .position
            }
            None => {
                active
                    .wander
                    .pose(&self.graph)
                    .map_err(|_| Error::Shape)?
                    .position
            }
        })
    }

    /// Stage the world, mesh namespace and population together. The caller must
    /// also reserve all content/replication queues before committing this result.
    pub fn advance(
        &mut self,
        players: &mut Players,
        registrations: &mut registrations::Registry,
        connection: u8,
        persona: u64,
        input: Input<'_>,
    ) -> Result<Changes, Error> {
        let Input {
            now,
            focus,
            random,
            preserved,
        } = input;
        if self.clock.is_some_and(|clock| now < clock)
            || focus.len() > 128
            || preserved.len() > ai::MAX_AI
        {
            return Err(Error::Bound);
        }
        let mut ids = BTreeSet::new();
        for point in focus {
            if point
                .position
                .iter()
                .any(|n| !n.is_finite() || n.abs() > 1_000_000.)
            {
                return Err(Error::Bound);
            }
            if !ids.insert(point.id) {
                return Err(Error::DuplicateObject);
            }
        }
        if self.owner.is_some_and(|old| old != (connection, persona)) {
            return Err(Error::UnknownObject);
        }
        let mut preserve = BTreeSet::new();
        for &id in preserved {
            if !preserve.insert(id) || !players.owns_ai_participant(connection, persona, id) {
                return Err(Error::UnknownObject);
            }
        }
        if self.clock == Some(now) {
            return Ok(Changes::default());
        }
        let seconds = self.clock.map_or(0, |clock| (now - clock).min(MAX_STEP_MS)) as f32 * 0.001;
        let mut next = self.clone();
        let mut world = players.clone();
        let mut meshes = registrations.clone();
        let mut changes = Changes::default();
        let nearest = |position: [f32; 3], measure: fn([f32; 3], [f32; 3]) -> f32| {
            focus
                .iter()
                .map(|f| measure(f.position, position))
                .fold(f32::INFINITY, f32::min)
        };
        for ordinal in 0..next.racers.len() {
            if let Some(active) = &mut next.racers[ordinal].active
                && active.car.is_none()
            {
                active
                    .wander
                    .advance(&next.graph, seconds)
                    .map_err(|_| Error::Shape)?;
            }
            let position = match &next.racers[ordinal].active {
                Some(active) => Some(next.position(&world, connection, persona, active)?),
                None => None,
            };
            let retained = position.is_some_and(|p| nearest(p, horizontal_squared) < VIEW * VIEW)
                || next.racers[ordinal].active.as_ref().is_some_and(|active| {
                    world
                        .ai_slot(connection, persona, active.slot)
                        .is_some_and(|slot| preserve.contains(&slot.participant))
                });
            let racer = &mut next.racers[ordinal];
            if focus.is_empty() {
                racer.region_active = false;
            } else if !retained && ordinal % 10 == next.cohort {
                racer.region_active = next.content.data.racers[ordinal]
                    .regions
                    .iter()
                    .any(|region| focus.iter().any(|f| region.contains(f.position)));
            }
            if !retained && !racer.region_active {
                if let Some(active) = racer.active.take() {
                    let removed = world.remove_ai_slot(connection, persona, active.slot)?;
                    changes.records.extend(removed.records);
                    changes
                        .leave
                        .extend(removed.leave.into_iter().map(HostRpc::Participant));
                    changes
                        .leave
                        .extend(removed.untracked.into_iter().map(HostRpc::WorldActivation));
                    changes.deleted.extend(removed.deleted);
                    if let Some(car) = active.car {
                        next.authority.remove_group(car);
                    }
                }
                continue;
            }
            if racer.active.is_none() {
                // Bounded publication activates at most one racer per tick.
                if changes.activated.is_some()
                    || world.ai_slot_count() >= ai::MAX_AI
                    || next.limit.is_some_and(|limit| next.count() >= limit)
                {
                    continue;
                }
                let seed = random.wrapping_add((ordinal as u32).wrapping_mul(0x9e37_79b9));
                if !next.activate(&mut world, connection, persona, ordinal, seed, &mut changes)? {
                    continue;
                }
            }
            let Some(active) = next.racers[ordinal].active.clone() else {
                continue;
            };
            let distance = nearest(
                next.position(&world, connection, persona, &active)?,
                squared,
            );
            match active.car {
                Some(car) if distance > HANDOVER_OUT * HANDOVER_OUT => {
                    next.hand_back(&mut world, connection, persona, ordinal, car, &mut changes)?;
                }
                None if distance <= HANDOVER_IN * HANDOVER_IN
                    && changes.spawned.is_none()
                    && world.ai_count() < ai::MAX_AI =>
                {
                    next.hand_over(
                        &mut world,
                        &mut meshes,
                        connection,
                        persona,
                        ordinal,
                        &mut changes,
                    )?;
                }
                _ => {}
            }
        }
        // Imported map (+0x98), separate from authored racers (+0x60).
        // No primary region, activation cohort or host-side respawn applies.
        for (&key, adopted) in &self.adopted {
            let position = world
                .ai_vehicle_physics(connection, persona, adopted.car)?
                .decode([0.; 3])?
                .position;
            if preserve.contains(&adopted.ids.participant)
                || nearest(position, horizontal_squared) < VIEW * VIEW
            {
                continue;
            }
            let removed = world.remove_ai_slot(connection, persona, adopted.slot)?;
            changes.records.extend(removed.records);
            changes
                .leave
                .extend(removed.leave.into_iter().map(HostRpc::Participant));
            changes
                .leave
                .extend(removed.untracked.into_iter().map(HostRpc::WorldActivation));
            changes.deleted.extend(removed.deleted);
            next.authority.remove_group(key);
            next.adopted.remove(&key);
        }
        next.clock = Some(now);
        next.cohort = (next.cohort + 1) % 10;
        next.owner = Some((connection, persona));
        *self = next;
        *players = world;
        *registrations = meshes;
        Ok(changes)
    }

    fn key(&mut self) -> Result<u64, Error> {
        let key = self.next;
        if key >= crate::event_ai::KEY_START {
            return Err(Error::Bound);
        }
        self.next = key.checked_add(1).ok_or(Error::Bound)?;
        Ok(key)
    }

    /// Create the racer's participant and actor at its authored spawn and start
    /// the host simulation there (native spawn `04683c70`). False when no road
    /// is near the spawn.
    fn activate(
        &mut self,
        world: &mut Players,
        connection: u8,
        persona: u64,
        ordinal: usize,
        seed: u32,
        changes: &mut Changes,
    ) -> Result<bool, Error> {
        let content = self.content.clone();
        let racer = &content.data.racers[ordinal];
        let Some(wander) = Wander::start(
            &self.graph,
            racer.position,
            racer.basis[2],
            Flags::from_goal(GOAL),
            NativeRandom::new(seed),
        )
        .map_err(|_| Error::Shape)?
        else {
            return Ok(false);
        };
        let cursor = self.racers[ordinal].persona_cursor;
        let selected = racer.personas[cursor % racer.personas.len()];
        let provider = &content.data.personas[selected];
        let personas = racer.personas.len();
        let slot = self.slots;
        if slot >= crate::event_ai::KEY_START {
            return Err(Error::Bound);
        }
        self.slots = slot.checked_add(1).ok_or(Error::Bound)?;
        let created = world
            .create_ai_slot(
                connection,
                persona,
                ai::Request {
                    key: slot,
                    name: provider.name.as_bytes().to_vec(),
                    flags: [true, true],
                    words: [provider.provider_index, provider.personality_index],
                    // Roaming racers use the full vehicle driving mode.
                    // The ambient-traffic mode selects different physics.
                    mode: 2,
                },
                content.data.managers,
            )?
            .ok_or(Error::DuplicateObject)?;
        changes.records.extend(created.records);
        changes
            .calls
            .extend(created.bound.add.map(HostRpc::Participant));
        changes.calls.extend(created.bound.bind.map(HostRpc::Actor));
        changes
            .calls
            .extend(created.bound.enter.map(HostRpc::Participant));
        changes.activated = Some(created.ids);
        self.racers[ordinal].persona_cursor = (cursor + 1) % personas;
        self.racers[ordinal].active = Some(Active {
            slot,
            persona: selected,
            wander,
            car: None,
        });
        Ok(true)
    }

    /// Remove the client's car and continue the host simulation from its last
    /// reported pose. The participant and actor stay (E986).
    fn hand_back(
        &mut self,
        world: &mut Players,
        connection: u8,
        persona: u64,
        ordinal: usize,
        car: u64,
        changes: &mut Changes,
    ) -> Result<(), Error> {
        let motion = world
            .ai_vehicle_physics(connection, persona, car)?
            .decode([0.; 3])?;
        let removed = world.remove_ai_vehicle(connection, persona, car)?;
        changes.records.extend(removed.records);
        changes.deleted.extend(removed.deleted);
        // Official order: binding and goal cleared, then the reset-volume leave.
        changes
            .calls
            .extend(removed.untracked.into_iter().map(HostRpc::WorldActivation));
        self.authority.remove_group(car);
        let heading = if horizontal_squared(motion.velocity, [0.; 3]) > 1. {
            motion.velocity
        } else {
            vehicle::orientation::basis_from_quaternion(motion.quaternion)?[2]
        };
        let active = self.racers[ordinal]
            .active
            .as_mut()
            .ok_or(Error::UnknownObject)?;
        // A car left far from every road resumes where the host last had it.
        if let Some(wander) = active
            .wander
            .relocate(&self.graph, motion.position, heading)
            .map_err(|_| Error::Shape)?
        {
            active.wander = wander;
        }
        active.car = None;
        Ok(())
    }

    /// Create a car for an active racer at its simulated pose, already moving
    /// at the wander speed along the road, and hand it to the client.
    fn hand_over(
        &mut self,
        world: &mut Players,
        registrations: &mut registrations::Registry,
        connection: u8,
        persona: u64,
        ordinal: usize,
        changes: &mut Changes,
    ) -> Result<(), Error> {
        let active = self.racers[ordinal]
            .active
            .clone()
            .ok_or(Error::UnknownObject)?;
        let pose = active.wander.pose(&self.graph).map_err(|_| Error::Shape)?;
        let basis = road_basis(pose.tangent).ok_or(Error::Shape)?;
        let locator =
            std::array::from_fn(|i| pose.position[i] + basis[0][i] * LANE_OFFSET * pose.width);
        let content = self.content.clone();
        let provider = &content.data.personas[active.persona];
        let key = self.key()?;
        let attached = vehicle_content::ai::attach(
            &content.vehicles,
            world,
            registrations,
            &mut self.authority,
            vehicle_content::ai::Spawn {
                connection,
                persona,
                slot: active.slot,
                key,
                manager_scene: content.data.managers.scene,
                provider,
                preset: &content.presets[active.persona],
                locator,
                basis,
                speed: WANDER_SPEED,
            },
        )?;
        changes.messages.extend(attached.messages);
        changes.records.extend(attached.records);
        changes
            .calls
            .push(HostRpc::WorldActivation(attached.tracked));
        let (goal, assigned) = world.start_ai_wander_goal(connection, persona, key, GOAL)?;
        changes.records.extend([goal, assigned]);
        changes.spawned = Some(attached.ids);
        if let Some(active) = &mut self.racers[ordinal].active {
            active.car = Some(key);
        }
        Ok(())
    }
}

/// Right, up and forward axes of a car on a road with tangent `forward`: up is
/// world up made perpendicular to the road, so the car follows its slope.
fn road_basis(forward: [f32; 3]) -> Option<[[f32; 3]; 3]> {
    let forward = normalized(forward)?;
    let up = normalized([
        -forward[1] * forward[0],
        1. - forward[1] * forward[1],
        -forward[1] * forward[2],
    ])?;
    let right = [
        up[1] * forward[2] - up[2] * forward[1],
        up[2] * forward[0] - up[0] * forward[2],
        up[0] * forward[1] - up[1] * forward[0],
    ];
    Some([right, up, forward])
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (length.is_finite() && length > 1e-3).then(|| v.map(|c| c / length))
}

fn horizontal_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)
}

fn squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    horizontal_squared(a, b) + (a[1] - b[1]).powi(2)
}

#[cfg(test)]
pub(crate) mod tests;
