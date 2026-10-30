// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AI lifecycle in the world's shared object namespace. AI participants have
//! no Player or owned inventory entry. The host supplies actor configuration;
//! this layer does not infer delegation or movement policy from a mode number.
use super::*;

pub const MAX_AI: usize = 128;
mod goals;
pub use goals::GoalChanges;

/// Installed serializer ordinals, supplied by content preparation. They are
/// distinct from RPC selector numbers and allocated scene ghost IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Managers {
    pub scene: u32,
    pub participants: usize,
    pub actors: usize,
    pub alive: State,
    /// Server-controlled streamed trigger-volume collection consumed by the
    /// client's AI reset tracker. Its membership is not retained in scene
    /// state; the host enters each spawned AI vehicle and leaves it at removal.
    pub reset_volumes: State,
}

/// Installed participant-state serializer, possibly in a different scene.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub scene: u32,
    pub serializer: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bound {
    pub add: Option<crate::participants::Notification>,
    pub bind: Option<crate::actors::Binding>,
    pub enter: Option<crate::participants::Notification>,
    /// Reset-volume tracking for the bound vehicle; follows `enter` on wire.
    pub tracked: Option<crate::world_activation::Notification>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Host-issued spawn identity; independent of ghost IDs and inventory IDs.
    pub key: u64,
    pub name: Vec<u8>,
    pub flags: [bool; 2],
    pub words: [u16; 2],
    pub mode: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ids {
    pub participant: u16,
    pub actor: u16,
    pub vehicle: u16,
}

#[derive(Clone, Debug)]
pub struct Spawned {
    pub ids: Ids,
    pub records: Vec<Record>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Removed {
    /// Leave the live state before publishing relation changes or deletion.
    pub leave: Vec<crate::participants::Notification>,
    /// Reset-volume leave for each tracked vehicle, sent after `leave`.
    pub untracked: Vec<crate::world_activation::Notification>,
    /// Detach relations before publishing the deletion list.
    pub records: Vec<Record>,
    pub deleted: Vec<u16>,
}

#[derive(Clone, Debug)]
pub(super) struct Instance {
    request: Request,
    creation: vehicle::EntityCreation,
    ids: Ids,
    goals: Vec<u16>,
    managers: Option<Managers>,
    /// The requested control-suppression flag has been cleared after the
    /// car's first accepted client report.
    released: bool,
    /// The persistent slot this car belongs to; removing the car then keeps
    /// the slot's participant and actor.
    slot: Option<u64>,
}

/// A persistent AI participant and actor whose car is created and removed
/// separately. Official roaming rivals keep their participant for the whole
/// time the racer is active while the host hands cars to and from the client
/// (E986): each car gets its own goal, reset-volume entry and control hold.
#[derive(Clone, Debug)]
pub(super) struct Slot {
    request: Request,
    participant: u16,
    actor: u16,
    managers: Managers,
    car: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotIds {
    pub participant: u16,
    pub actor: u16,
    /// Spawn key of the slot's current car.
    pub car: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotCreated {
    pub ids: SlotIds,
    pub records: Vec<Record>,
    /// Participant add, actor binding and Alive enter; no reset tracking until
    /// the slot has a car.
    pub bound: Bound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotAttached {
    pub ids: Ids,
    pub records: Vec<Record>,
    /// Reset-volume enter for the participant, sent after the car is bound.
    pub tracked: crate::world_activation::Notification,
}

fn validate_managers(managers: Managers) -> Result<(), Error> {
    let manager_fields = [managers.participants, managers.actors];
    if (managers.alive.scene == managers.scene
        && manager_fields.contains(&managers.alive.serializer))
        || (managers.reset_volumes.scene == managers.scene
            && manager_fields.contains(&managers.reset_volumes.serializer))
        || managers.reset_volumes == managers.alive
    {
        return Err(Error::Shape);
    }
    Ok(())
}

fn validate_request(request: &Request) -> Result<(), Error> {
    if request.key == 0 || request.name.is_empty() || request.name.contains(&0) || request.mode > 2
    {
        return Err(Error::Shape);
    }
    if request.name.len() > 64 {
        return Err(Error::Bound);
    }
    Ok(())
}

impl Players {
    /// Shared budget for all host-owned AI populations in this world.
    pub fn ai_count(&self) -> usize {
        self.ai.len()
    }

    /// Persistent AI participants in this world, with or without a car.
    pub fn ai_slot_count(&self) -> usize {
        self.ai_slots.len()
    }

    pub fn ai_ids(&self, connection: u8, persona: u64, key: u64) -> Option<Ids> {
        self.ai.get(&(connection, persona, key)).map(|a| a.ids)
    }

    /// Allocate the participant, actor and car together. A failed car record or
    /// exhausted namespace consumes neither IDs nor the host's retry key.
    pub fn spawn_ai_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        request: Request,
        creation: vehicle::EntityCreation,
        content: &entity::creation::Content,
    ) -> Result<Option<Spawned>, Error> {
        if self.account_runtime_index(connection, persona).is_none() {
            return Err(Error::UnknownObject);
        }
        validate_request(&request)?;
        let key = (connection, persona, request.key);
        if let Some(old) = self.ai.get(&key) {
            return if old.request == request && old.creation == creation {
                Ok(None)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.ai.len() >= MAX_AI {
            return Err(Error::Bound);
        }
        let mut next = self.objects.clone();
        let identity = NamedIdentity {
            identity: Identity {
                persona: 0,
                bytes: Vec::new(),
            },
            name: request.name.clone(),
        };
        let participant = next.spawn(
            Initial::Participant(ParticipantInit {
                selector: 0,
                serial: 0,
                identity: identity.clone(),
                player: 0,
            }),
            Update::Participant(ParticipantUpdate {
                player: Some(0),
                entity: None,
                identity: Some(identity),
            }),
        )?;
        let actor = next.spawn(
            Initial::Actor(ActorInit {
                participant: participant.id,
                flag_a: request.flags[0],
                flag_b: request.flags[1],
                word_a: request.words[0],
                word_b: request.words[1],
            }),
            Update::Actor(ActorUpdate {
                flag_a: Some(request.flags[0]),
                flag_b: Some(request.flags[1]),
                word_pair: Some(request.words),
                word_c: Some(request.mode),
                ..ActorUpdate::default()
            }),
        )?;
        let vehicle = next.spawn_vehicle(creation.clone(), content)?;
        let attached = next
            .change(
                participant.id,
                Update::Participant(ParticipantUpdate {
                    entity: Some(vehicle.id),
                    ..ParticipantUpdate::default()
                }),
            )?
            .ok_or(Error::Shape)?;
        let ids = Ids {
            participant: participant.id,
            actor: actor.id,
            vehicle: vehicle.id,
        };
        self.objects = next;
        self.ai.insert(
            key,
            Instance {
                request,
                creation,
                ids,
                goals: Vec::new(),
                managers: None,
                released: false,
                slot: None,
            },
        );
        Ok(Some(Spawned {
            ids,
            records: vec![participant, actor, vehicle, attached],
        }))
    }

    /// Whether `participant` is one of this session's AI participants.
    pub fn owns_ai_participant(&self, connection: u8, persona: u64, participant: u16) -> bool {
        self.ai.iter().any(|(&(c, p, _), instance)| {
            c == connection && p == persona && instance.ids.participant == participant
        }) || self.ai_slots.iter().any(|(&(c, p, _), slot)| {
            c == connection && p == persona && slot.participant == participant
        })
    }

    pub fn ai_slot(&self, connection: u8, persona: u64, key: u64) -> Option<SlotIds> {
        self.ai_slots
            .get(&(connection, persona, key))
            .map(|slot| SlotIds {
                participant: slot.participant,
                actor: slot.actor,
                car: slot.car,
            })
    }

    /// Create a persistent AI participant and actor without a car and bind
    /// them to the installed managers in one step. Retrying the same request
    /// is inert; a different request under the same key is rejected.
    pub fn create_ai_slot(
        &mut self,
        connection: u8,
        persona: u64,
        request: Request,
        managers: Managers,
    ) -> Result<Option<SlotCreated>, Error> {
        if self.account_runtime_index(connection, persona).is_none() {
            return Err(Error::UnknownObject);
        }
        validate_request(&request)?;
        validate_managers(managers)?;
        let key = (connection, persona, request.key);
        if let Some(old) = self.ai_slots.get(&key) {
            return if old.request == request && old.managers == managers {
                Ok(None)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.ai_slots.len() >= MAX_AI {
            return Err(Error::Bound);
        }
        let mut next = self.objects.clone();
        let identity = NamedIdentity {
            identity: Identity {
                persona: 0,
                bytes: Vec::new(),
            },
            name: request.name.clone(),
        };
        let participant = next.spawn(
            Initial::Participant(ParticipantInit {
                selector: 0,
                serial: 0,
                identity: identity.clone(),
                player: 0,
            }),
            Update::Participant(ParticipantUpdate {
                player: Some(0),
                entity: None,
                identity: Some(identity),
            }),
        )?;
        let actor = next.spawn(
            Initial::Actor(ActorInit {
                participant: participant.id,
                flag_a: request.flags[0],
                flag_b: request.flags[1],
                word_a: request.words[0],
                word_b: request.words[1],
            }),
            Update::Actor(ActorUpdate {
                flag_a: Some(request.flags[0]),
                flag_b: Some(request.flags[1]),
                word_pair: Some(request.words),
                word_c: Some(request.mode),
                ..ActorUpdate::default()
            }),
        )?;
        let mut bound = next.bind_ai_managers(managers, participant.id, actor.id)?;
        bound.enter = next.ai_state(managers.alive, participant.id, true)?;
        let ids = SlotIds {
            participant: participant.id,
            actor: actor.id,
            car: None,
        };
        self.objects = next;
        self.ai_slots.insert(
            key,
            Slot {
                request,
                participant: ids.participant,
                actor: ids.actor,
                managers,
                car: None,
            },
        );
        Ok(Some(SlotCreated {
            ids,
            records: vec![participant, actor],
            bound,
        }))
    }

    /// Give a slot a new car under spawn key `car`. The car's participant is
    /// the slot's; it enters the reset-volume collection and its control stays
    /// held until the client's first report, like the official host (E986).
    pub fn attach_ai_slot_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        slot: u64,
        car: u64,
        creation: vehicle::EntityCreation,
        content: &entity::creation::Content,
    ) -> Result<SlotAttached, Error> {
        let owner = self
            .ai_slots
            .get(&(connection, persona, slot))
            .ok_or(Error::UnknownObject)?;
        if owner.car.is_some() || car == 0 || self.ai.contains_key(&(connection, persona, car)) {
            return Err(Error::DuplicateObject);
        }
        if self.ai.len() >= MAX_AI {
            return Err(Error::Bound);
        }
        let mut next = self.objects.clone();
        let vehicle = next.spawn_vehicle(creation.clone(), content)?;
        let attached = next
            .change(
                owner.participant,
                Update::Participant(ParticipantUpdate {
                    entity: Some(vehicle.id),
                    ..ParticipantUpdate::default()
                }),
            )?
            .ok_or(Error::Shape)?;
        let tracked = next.ai_tracking(owner.managers.reset_volumes, owner.participant, true)?;
        let ids = Ids {
            participant: owner.participant,
            actor: owner.actor,
            vehicle: vehicle.id,
        };
        let request = Request {
            key: car,
            ..owner.request.clone()
        };
        let managers = owner.managers;
        self.objects = next;
        self.ai.insert(
            (connection, persona, car),
            Instance {
                request,
                creation,
                ids,
                goals: Vec::new(),
                managers: Some(managers),
                released: false,
                slot: Some(slot),
            },
        );
        if let Some(owner) = self.ai_slots.get_mut(&(connection, persona, slot)) {
            owner.car = Some(car);
        }
        Ok(SlotAttached {
            ids,
            records: vec![vehicle, attached],
            tracked,
        })
    }

    /// Retire a slot with its car, if any, as one transaction.
    pub fn remove_ai_slot(
        &mut self,
        connection: u8,
        persona: u64,
        slot: u64,
    ) -> Result<Removed, Error> {
        let Some(owner) = self.ai_slots.get(&(connection, persona, slot)).cloned() else {
            return Ok(Removed::default());
        };
        let car = owner.car.and_then(|key| {
            self.ai
                .get(&(connection, persona, key))
                .map(|i| (key, i.clone()))
        });
        let mut next = self.objects.clone();
        let result = self.retire(
            &mut next,
            connection,
            persona,
            Retired {
                participant: owner.participant,
                actor: owner.actor,
                vehicle: car.as_ref().map(|(_, i)| i.ids.vehicle),
                goals: car
                    .as_ref()
                    .map(|(_, i)| i.goals.clone())
                    .unwrap_or_default(),
                alive: Some(owner.managers),
                tracked: car.is_some().then_some(owner.managers),
            },
        )?;
        self.objects = next;
        if let Some((key, _)) = car {
            self.ai.remove(&(connection, persona, key));
        }
        self.ai_slots.remove(&(connection, persona, slot));
        Ok(result)
    }

    /// Bind the AI participant and actor to their installed scene managers.
    /// Manager/Alive snapshots and their RPCs commit together; retry is inert.
    /// The first binding also enters the car's participant into the reset-volume
    /// collection, as the official host does once the car exists (E981).
    pub fn bind_ai_managers(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
        managers: Managers,
    ) -> Result<Bound, Error> {
        validate_managers(managers)?;
        let instance = self
            .ai
            .get(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?;
        if instance.managers.is_some_and(|old| old != managers) {
            return Err(Error::DuplicateObject);
        }
        let first = instance.managers.is_none();
        let mut next = self.objects.clone();
        let mut bound =
            next.bind_ai_managers(managers, instance.ids.participant, instance.ids.actor)?;
        bound.enter = next.ai_state(managers.alive, instance.ids.participant, true)?;
        if first {
            bound.tracked =
                Some(next.ai_tracking(managers.reset_volumes, instance.ids.participant, true)?);
        }
        self.objects = next;
        self.ai
            .get_mut(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?
            .managers = Some(managers);
        Ok(bound)
    }

    /// Attach an installed TrafficGoal. The global goal serial is shared with
    /// scripted human goals and advances only after a successful assignment.
    pub fn start_ai_traffic_goal(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
        mut initial: traffic::Initial,
    ) -> Result<(Record, Record), Error> {
        self.start_ai_goal(connection, persona, key, |serial| {
            initial.allocation_serial = serial;
            (Initial::TrafficGoal(initial), Update::TrafficGoal)
        })
    }

    /// Attach the native wandering goal to an owned AI actor. Its allocation
    /// counter is shared with all other goals; failed assignment is atomic.
    pub fn start_ai_wander_goal(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
        flags: [bool; 3],
    ) -> Result<(Record, Record), Error> {
        self.start_ai_goal(connection, persona, key, |serial| {
            (
                Initial::WanderGoal(wander::Initial {
                    allocation_serial: serial,
                    flags,
                }),
                Update::WanderGoal,
            )
        })
    }

    fn start_ai_goal(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
        create: impl FnOnce(u8) -> (Initial, Update),
    ) -> Result<(Record, Record), Error> {
        let instance = self
            .ai
            .get(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?;
        if !instance.goals.is_empty() {
            return Err(Error::DuplicateObject);
        }
        let allocation_serial = (self.goal_allocation_serial + 1) & 63;
        let (initial, update) = create(allocation_serial);
        let mut next = self.objects.clone();
        let goal = next.spawn(initial, update)?;
        let assigned = next
            .change(
                instance.ids.actor,
                Update::Actor(ActorUpdate {
                    references_a: Some(vec![goal.id]),
                    ..ActorUpdate::default()
                }),
            )?
            .ok_or(Error::Shape)?;
        self.objects = next;
        self.goal_allocation_serial = allocation_serial;
        self.ai
            .get_mut(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?
            .goals = vec![goal.id];
        Ok((goal, assigned))
    }

    /// Retire one host spawn; repeated removal is inert. Manager snapshots release
    /// their references atomically with deletion, matching client death listeners.
    /// A slot's car is detached instead: the participant and actor stay, the
    /// car leaves the reset volumes and its control is held again (E986).
    pub fn remove_ai_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
    ) -> Result<Removed, Error> {
        let Some(instance) = self.ai.get(&(connection, persona, key)).cloned() else {
            return Ok(Removed::default());
        };
        let mut next = self.objects.clone();
        let result = if let Some(slot) = instance.slot {
            let result = self.detach(&mut next, &instance)?;
            if let Some(owner) = self.ai_slots.get_mut(&(connection, persona, slot)) {
                owner.car = None;
            }
            result
        } else {
            self.retire(
                &mut next,
                connection,
                persona,
                Retired {
                    participant: instance.ids.participant,
                    actor: instance.ids.actor,
                    vehicle: Some(instance.ids.vehicle),
                    goals: instance.goals,
                    alive: instance.managers,
                    tracked: instance.managers,
                },
            )?
        };
        self.objects = next;
        self.ai.remove(&(connection, persona, key));
        Ok(result)
    }

    /// Remove a slot car, keeping its participant and actor.
    fn detach(&self, next: &mut state::World, instance: &Instance) -> Result<Removed, Error> {
        let mut result = Removed::default();
        result.records.extend(next.change(
            instance.ids.participant,
            Update::Participant(ParticipantUpdate {
                entity: Some(0),
                ..ParticipantUpdate::default()
            }),
        )?);
        if !instance.goals.is_empty() {
            result.records.extend(next.change(
                instance.ids.actor,
                Update::Actor(ActorUpdate {
                    references_a: Some(Vec::new()),
                    references_b: Some(Vec::new()),
                    ..ActorUpdate::default()
                }),
            )?);
            for &goal in &instance.goals {
                next.remove(goal)?;
                result.deleted.push(goal);
            }
        }
        next.remove(instance.ids.vehicle)?;
        result.deleted.push(instance.ids.vehicle);
        if instance.request.flags[1] && instance.released {
            result.records.extend(next.change(
                instance.ids.actor,
                Update::Actor(ActorUpdate {
                    flag_b: Some(true),
                    ..ActorUpdate::default()
                }),
            )?);
        }
        if let Some(managers) = instance.managers {
            result.untracked.push(next.ai_tracking(
                managers.reset_volumes,
                instance.ids.participant,
                false,
            )?);
        }
        Ok(result)
    }

    /// Delete a participant, its actor and optional car and goal, leaving the
    /// managers, reset volumes and the session's rival map in one transaction.
    fn retire(
        &self,
        next: &mut state::World,
        connection: u8,
        persona: u64,
        retired: Retired,
    ) -> Result<Removed, Error> {
        let mut result = Removed::default();
        if let Some(managers) = retired.alive {
            result
                .leave
                .extend(next.ai_state(managers.alive, retired.participant, false)?);
        }
        if let Some(managers) = retired.tracked {
            result.untracked.push(next.ai_tracking(
                managers.reset_volumes,
                retired.participant,
                false,
            )?);
        }
        if let Some(managers) = retired.alive {
            next.release_ai_managers(managers, retired.participant, retired.actor)?;
        }
        // A listed rival must leave the session's map before its participant
        // can be deleted; the map update precedes the deletion on wire.
        if let Some(&(map, _)) = self.rival_maps.get(&(connection, persona)) {
            let Some(state::Object {
                current: Update::ParticipantLocationMap(current),
                ..
            }) = next.get(map)
            else {
                return Err(Error::TypeMismatch);
            };
            let groups: Vec<_> = current
                .groups
                .iter()
                .flatten()
                .map(|g| super::super::location_map::Group {
                    value: g.value,
                    entries: g
                        .entries
                        .iter()
                        .filter(|e| e.participant != retired.participant)
                        .cloned()
                        .collect(),
                })
                .filter(|g| !g.entries.is_empty())
                .collect();
            if current.groups.as_ref() != Some(&groups) {
                result.records.extend(next.change(
                    map,
                    Update::ParticipantLocationMap(super::super::location_map::Update {
                        groups: Some(groups),
                    }),
                )?);
            }
        }
        if retired.vehicle.is_some() {
            result.records.extend(next.change(
                retired.participant,
                Update::Participant(ParticipantUpdate {
                    entity: Some(0),
                    ..ParticipantUpdate::default()
                }),
            )?);
        }
        if !retired.goals.is_empty() {
            result.records.extend(next.change(
                retired.actor,
                Update::Actor(ActorUpdate {
                    references_a: Some(Vec::new()),
                    references_b: Some(Vec::new()),
                    ..ActorUpdate::default()
                }),
            )?);
            for goal in retired.goals {
                next.remove(goal)?;
                result.deleted.push(goal);
            }
        }
        for id in retired
            .vehicle
            .into_iter()
            .chain([retired.actor, retired.participant])
        {
            next.remove(id)?;
            result.deleted.push(id);
        }
        Ok(result)
    }

    /// Session cleanup is all-or-nothing even when a retained scene reference
    /// prevents one of this connection's AI participants from being retired.
    pub fn remove_ai_for_connection(&mut self, connection: u8) -> Result<Removed, Error> {
        let slots: Vec<_> = self
            .ai_slots
            .keys()
            .filter(|(c, _, _)| *c == connection)
            .copied()
            .collect();
        let mut next = self.clone();
        let mut result = Removed::default();
        let groups: Vec<_> = next
            .pacing_groups
            .iter()
            .filter(|(_, (c, _))| *c == connection)
            .map(|(&id, &owner)| (id, owner))
            .collect();
        if !groups.is_empty() {
            for &(group, (c, p)) in &groups {
                let human_goals: Vec<_> = next
                    .actors
                    .iter()
                    .filter_map(|(&participant, &actor)| {
                        if !next.owns_participant(c, p, participant) {
                            return None;
                        }
                        let Update::Actor(current) = &next.objects.get(actor)?.current else {
                            return None;
                        };
                        let [goal, pacing] = current.references_a.as_deref()? else {
                            return None;
                        };
                        (*pacing == group).then_some((participant, *goal))
                    })
                    .collect();
                for (participant, goal) in human_goals {
                    let released =
                        next.release_player_race_goals(c, p, participant, goal, group)?;
                    result.records.extend(released.records);
                    result.deleted.extend(released.deleted);
                }
            }
            let cars: Vec<_> = next
                .ai
                .keys()
                .filter(|(c, _, _)| *c == connection)
                .copied()
                .collect();
            for (c, p, k) in cars {
                let released = next.replace_ai_goals(c, p, k, vec![], None)?;
                result.records.extend(released.records);
                result.deleted.extend(released.deleted);
            }
            for (id, (c, p)) in groups {
                next.remove_ai_pacing_group(c, p, id)?;
                result.deleted.push(id);
            }
        }
        let mut merge = |removed: Removed| {
            result.leave.extend(removed.leave);
            result.untracked.extend(removed.untracked);
            result.records.extend(removed.records);
            result.deleted.extend(removed.deleted);
        };
        for (c, p, k) in slots {
            merge(next.remove_ai_slot(c, p, k)?);
        }
        let keys: Vec<_> = next
            .ai
            .keys()
            .filter(|(c, _, _)| *c == connection)
            .copied()
            .collect();
        for (c, p, k) in keys {
            merge(next.remove_ai_vehicle(c, p, k)?);
        }
        *self = next;
        Ok(result)
    }

    /// Clear the actor's control-suppression flag once the car's client has
    /// reported it. Official traffic and rival actors are created with
    /// `flag_b` set and cleared one tick after the first report (E981); the
    /// client sends zero control while it is set. Later reports are inert.
    pub fn release_ai_control(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
    ) -> Result<Option<Record>, Error> {
        let instance = self
            .ai
            .get(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?;
        if instance.released || !instance.request.flags[1] {
            return Ok(None);
        }
        let actor = instance.ids.actor;
        let record = self
            .objects
            .change(
                actor,
                Update::Actor(ActorUpdate {
                    flag_b: Some(false),
                    ..ActorUpdate::default()
                }),
            )?
            .ok_or(Error::Shape)?;
        self.ai
            .get_mut(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?
            .released = true;
        Ok(Some(record))
    }
}

/// What [`Players::retire`] deletes and leaves.
struct Retired {
    participant: u16,
    actor: u16,
    vehicle: Option<u16>,
    goals: Vec<u16>,
    alive: Option<Managers>,
    tracked: Option<Managers>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity::creation::{Asset, Catalog, Content, Prefix};
    use nfs_protocol::world::rpc::Serial;

    fn world() -> Players {
        let mut world = Players::default();
        for (connection, persona) in [(1, 100), (2, 200)] {
            let player = world
                .create(
                    connection,
                    persona,
                    super::super::Request {
                        name: b"Local".to_vec(),
                        flag: false,
                        slot: 0,
                    },
                )
                .unwrap()
                .unwrap();
            world.join(connection, persona, player.id).unwrap();
        }
        world
    }
    fn request(key: u64) -> Request {
        Request {
            key,
            name: b"Constructed traffic".to_vec(),
            flags: [false; 2],
            words: [u16::MAX; 2],
            mode: 0,
        }
    }
    fn car() -> (vehicle::EntityCreation, Content) {
        let asset = Asset {
            bundle: 7,
            type_id: 100,
            local_index: 0,
        };
        let profile = vehicle::Profile::new(vec![vehicle::Kind::Root {
            property_owner: true,
        }])
        .unwrap();
        let content = Content::new(vec![Catalog::new(7, &[(100, 1)]).unwrap()], vec![])
            .unwrap()
            .with_vehicles(vec![(asset, profile)])
            .unwrap();
        let root = vehicle::root::State::new(Some(1)).unwrap();
        let initial = root
            .initial(
                [0.; 3],
                [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                [0.; 3],
                entity::Rpc {
                    selector: 0,
                    serial: Serial::new(1).unwrap(),
                },
            )
            .unwrap();
        (
            vehicle::EntityCreation {
                prefix: Prefix {
                    parent: None,
                    blueprint: 0,
                    sub_id: 0,
                    owner: None,
                    asset,
                },
                body: vehicle::Body {
                    creation: Some(vehicle::Creation {
                        setup: None,
                        connection_id: 0,
                        fields: vec![vehicle::Initial::Root(Box::new(initial))],
                    }),
                    updates: vec![Some(vehicle::Update::Root(Some(0)))],
                },
            },
            content,
        )
    }
    fn goal() -> traffic::Initial {
        traffic::Initial {
            asset: Asset {
                bundle: 1,
                type_id: 99,
                local_index: 0,
            },
            catalog: Catalog::new(1, &[(99, 1)]).unwrap(),
            allocation_serial: 63,
            asset_mode: 2,
        }
    }

    #[test]
    fn waypoint_completion_requires_delegation_and_keeps_the_sequence_in_control() {
        use crate::{bits::BitWriter, replication::goto_waypoint};
        let mut world = world();
        let (creation, content) = car();
        let ids = world
            .spawn_ai_vehicle(1, 100, request(1), creation, &content)
            .unwrap()
            .unwrap()
            .ids;
        let goals = || {
            vec![(
                Initial::GotoWaypointGoal(goto_waypoint::Initial {
                    selector: 3,
                    serial: 1,
                    allocation_serial: 0,
                    waypoint: 0,
                    word: 1,
                    points: vec![[0; 3]],
                    flags: [false; 2],
                    radius: 2f32.to_bits(),
                    progress_updates: false,
                }),
                Update::GotoWaypointGoal(goto_waypoint::Update::NoProgress),
            )]
        };
        let assigned = world.replace_ai_goals(1, 100, 1, goals(), None).unwrap();
        let request = |goal: u16| {
            let mut payload = BitWriter::new();
            payload.put(3, 9).put(0, 32).put(127, 7);
            let mut body = BitWriter::new();
            body.put(0, 32)
                .put(0, 32)
                .put(1, 8)
                .put(goal.into(), 13)
                .put(payload.bytes().len() as u64, 9)
                .put_span(payload.span());
            body
        };
        let body = request(assigned.goals[0]);
        let before = world.objects.snapshot();
        for delegation in [
            BTreeMap::new(),
            BTreeMap::from([(ids.vehicle, 2)]),
            BTreeMap::from([(ids.vehicle + 1, 1)]),
        ] {
            assert_eq!(
                world.receive_goal_completion(1, 100, body.span(), &delegation),
                Err(Error::UnknownObject)
            );
        }
        let delegated = BTreeMap::from([(ids.vehicle, 1)]);
        assert_eq!(
            world.receive_goal_completion(2, 200, body.span(), &delegated),
            Err(Error::UnknownObject)
        );
        let completed = world
            .receive_goal_completion(1, 100, body.span(), &delegated)
            .unwrap()
            .unwrap();
        assert_eq!(
            (completed.actor, completed.participant, completed.first),
            (ids.actor, ids.participant, true)
        );
        assert!(
            !world
                .receive_goal_completion(1, 100, body.span(), &delegated)
                .unwrap()
                .unwrap()
                .first
        );
        assert_eq!(world.objects.snapshot(), before);
        let replaced = world.replace_ai_goals(1, 100, 1, goals(), None).unwrap();
        assert!(
            !world
                .receive_goal_completion(1, 100, body.span(), &delegated)
                .unwrap()
                .unwrap()
                .first
        );
        assert!(
            world
                .receive_goal_completion(1, 100, request(replaced.goals[0]).span(), &delegated)
                .unwrap()
                .unwrap()
                .first
        );
    }

    #[test]
    fn race_commands_replace_atomically_and_share_pacing_until_last_release() {
        use crate::replication::{goto_waypoint, pacing, speed_limit};
        let mut world = world();
        let (creation, content) = car();
        let a = world
            .spawn_ai_vehicle(1, 100, request(1), creation.clone(), &content)
            .unwrap()
            .unwrap()
            .ids;
        let b = world
            .spawn_ai_vehicle(1, 100, request(2), creation.clone(), &content)
            .unwrap()
            .unwrap()
            .ids;
        let other = world
            .spawn_ai_vehicle(2, 200, request(1), creation, &content)
            .unwrap()
            .unwrap()
            .ids;
        let entry = |p| pacing::Entry {
            participant: p,
            value: 0.5f32.to_bits(),
        };
        let before = world.objects.snapshot();
        assert!(
            world
                .create_ai_pacing_group(
                    1,
                    100,
                    vec![entry(a.participant), entry(other.participant)]
                )
                .is_err()
        );
        assert_eq!(world.objects.snapshot(), before);
        let group = world
            .create_ai_pacing_group(1, 100, vec![entry(a.participant), entry(b.participant)])
            .unwrap()
            .id;
        let route = || {
            (
                Initial::GotoWaypointGoal(goto_waypoint::Initial {
                    selector: 0,
                    serial: 1,
                    allocation_serial: 0,
                    waypoint: 0,
                    word: 1,
                    points: vec![[0; 3], [10f32.to_bits(), 0, 0]],
                    flags: [false; 2],
                    radius: 150f32.to_bits(),
                    progress_updates: true,
                }),
                Update::GotoWaypointGoal(goto_waypoint::Update::Progress(Some([0, 0]))),
            )
        };
        let speed = || {
            (
                Initial::SpeedLimitGoal(speed_limit::Initial {
                    selector: 0,
                    serial: 1,
                    allocation_serial: 0,
                    limit: 25f32.to_bits(),
                }),
                Update::SpeedLimitGoal,
            )
        };
        let burnout = world
            .replace_ai_goals(
                1,
                100,
                1,
                vec![(
                    Initial::ScriptedInputGoal(crate::replication::scripted_input::Initial {
                        selector: 0,
                        serial: 1,
                        allocation_serial: 0,
                        throttle: 1f32.to_bits(),
                        steering: 0,
                        brake: 1f32.to_bits(),
                        handbrake: 0,
                        use_nos: false,
                    }),
                    Update::ScriptedInputGoal,
                )],
                None,
            )
            .unwrap();
        assert_eq!(burnout.goals.len(), 1);
        let intro = world
            .replace_ai_goals(1, 100, 1, vec![route(), speed()], None)
            .unwrap();
        assert_eq!(intro.goals.len(), 2);
        assert_eq!(intro.deleted, burnout.goals);
        assert!(world.objects.get(burnout.goals[0]).is_none());
        let before = world.objects.snapshot();
        let serial = world.goal_allocation_serial;
        let mut invalid = speed();
        if let Initial::SpeedLimitGoal(g) = &mut invalid.0 {
            g.limit = f32::NAN.to_bits();
        }
        assert!(
            world
                .replace_ai_goals(1, 100, 1, vec![route(), invalid], Some(group))
                .is_err()
        );
        assert_eq!(world.objects.snapshot(), before);
        assert_eq!(world.goal_allocation_serial, serial);
        assert!(
            world
                .replace_ai_goals(2, 200, 1, vec![route()], Some(group))
                .is_err()
        );
        assert_eq!(world.objects.snapshot(), before);
        let race = world
            .replace_ai_goals(1, 100, 1, vec![route()], Some(group))
            .unwrap();
        assert_eq!(race.deleted, intro.goals);
        assert!(
            race.deleted
                .iter()
                .all(|&id| world.objects.get(id).is_none())
        );
        world
            .replace_ai_goals(1, 100, 2, vec![route()], Some(group))
            .unwrap();
        let Update::Actor(actor) = &world.objects.get(a.actor).unwrap().current else {
            panic!()
        };
        assert_eq!(actor.references_a, Some(vec![race.goals[0], group]));
        assert!(world.remove_ai_pacing_group(1, 100, group).is_err());
        world.replace_ai_goals(1, 100, 1, vec![], None).unwrap();
        assert!(world.objects.get(group).is_some());
        assert!(world.remove_ai_pacing_group(1, 100, group).is_err());
        world.replace_ai_goals(1, 100, 2, vec![], None).unwrap();
        assert!(world.remove_ai_pacing_group(2, 200, group).is_err());
        assert!(world.remove_ai_pacing_group(1, 100, group).unwrap());
        assert!(!world.remove_ai_pacing_group(1, 100, group).unwrap());
        world
            .replace_ai_goals(1, 100, 1, vec![route(), speed()], None)
            .unwrap();
        let removed = world.remove_ai_vehicle(1, 100, 1).unwrap();
        assert_eq!(removed.deleted.len(), 5);
        assert!(world.objects.get(other.vehicle).is_some());
        assert!(world.objects.get(b.vehicle).is_some());
        assert!(
            world
                .remove_ai_vehicle(1, 100, 1)
                .unwrap()
                .deleted
                .is_empty()
        );
        let group = world
            .create_ai_pacing_group(1, 100, vec![entry(b.participant)])
            .unwrap()
            .id;
        world
            .replace_ai_goals(1, 100, 2, vec![route()], Some(group))
            .unwrap();
        let removed = world.remove_ai_for_connection(1).unwrap();
        assert!(removed.deleted.contains(&group));
        assert!(world.pacing_groups.is_empty());
        assert!(world.objects.get(other.vehicle).is_some());
    }

    #[test]
    fn wandering_and_traffic_share_serials_and_keep_owned_lifecycles_isolated() {
        let mut world = world();
        let (car, content) = car();
        for (connection, persona) in [(1, 100), (2, 200)] {
            world
                .spawn_ai_vehicle(connection, persona, request(1), car.clone(), &content)
                .unwrap();
        }
        let before = world.objects.snapshot();
        assert!(matches!(
            world.start_ai_wander_goal(2, 100, 1, [false, false, true]),
            Err(Error::UnknownObject)
        ));
        assert_eq!(world.objects.snapshot(), before);
        world.goal_allocation_serial = 62;
        let (traffic, _) = world.start_ai_traffic_goal(1, 100, 1, goal()).unwrap();
        assert!(
            matches!(traffic.initial, Some(Initial::TrafficGoal(v)) if v.allocation_serial == 63)
        );
        let (wander, assigned) = world
            .start_ai_wander_goal(2, 200, 1, [false, false, true])
            .unwrap();
        assert_eq!(
            wander.initial,
            Some(Initial::WanderGoal(wander::Initial {
                allocation_serial: 0,
                flags: [false, false, true],
            }))
        );
        assert!(
            matches!(assigned.update, Update::Actor(v) if v.references_a == Some(vec![wander.id]))
        );
        let before = world.objects.snapshot();
        assert!(matches!(
            world.start_ai_wander_goal(2, 200, 1, [true; 3]),
            Err(Error::DuplicateObject)
        ));
        assert_eq!(world.objects.snapshot(), before);
        assert_eq!(world.goal_allocation_serial, 0);
        assert!(
            world
                .remove_ai_for_connection(2)
                .unwrap()
                .deleted
                .contains(&wander.id)
        );
        assert!(world.objects.get(traffic.id).is_some());
        assert_eq!(
            world.remove_ai_for_connection(2).unwrap(),
            Removed::default()
        );
    }

    #[test]
    fn suppressed_control_is_released_once_per_spawn_and_isolated() {
        let mut world = world();
        let (car, content) = car();
        let held = Request {
            flags: [true, true],
            ..request(1)
        };
        let a = world
            .spawn_ai_vehicle(1, 100, held.clone(), car.clone(), &content)
            .unwrap()
            .unwrap();
        let Some(Initial::Actor(init)) = &a.records[1].initial else {
            panic!()
        };
        assert!(init.flag_a && init.flag_b);
        world
            .spawn_ai_vehicle(2, 200, held, car.clone(), &content)
            .unwrap();
        world
            .spawn_ai_vehicle(1, 100, request(2), car, &content)
            .unwrap();
        // Unknown spawn keys and other sessions are rejected without changes.
        let before = world.objects.snapshot();
        assert_eq!(
            world.release_ai_control(1, 200, 1),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);
        let record = world.release_ai_control(1, 100, 1).unwrap().unwrap();
        assert_eq!(record.id, a.ids.actor);
        assert!(matches!(
            record.update,
            Update::Actor(ActorUpdate {
                flag_b: Some(false),
                flag_a: None,
                references_a: None,
                ..
            })
        ));
        record.encode().unwrap();
        assert!(matches!(
            world.objects.get(a.ids.actor).unwrap().current,
            Update::Actor(ActorUpdate {
                flag_b: Some(false),
                ..
            })
        ));
        // Repeated reports and actors created unsuppressed stay inert.
        assert_eq!(world.release_ai_control(1, 100, 1), Ok(None));
        assert_eq!(world.release_ai_control(1, 100, 2), Ok(None));
        // The other session's identical spawn is still held.
        let other = world.ai_ids(2, 200, 1).unwrap();
        assert!(matches!(
            world.objects.get(other.actor).unwrap().current,
            Update::Actor(ActorUpdate {
                flag_b: Some(true),
                ..
            })
        ));
        world.remove_ai_vehicle(1, 100, 1).unwrap();
        assert_eq!(
            world.release_ai_control(1, 100, 1),
            Err(Error::UnknownObject)
        );
    }

    #[test]
    fn failed_wandering_actor_assignment_consumes_neither_id_nor_serial() {
        let mut world = world();
        let (car, content) = car();
        let actor = world
            .spawn_ai_vehicle(1, 100, request(1), car, &content)
            .unwrap()
            .unwrap()
            .ids
            .actor;
        // Inject a stale actor reference to exercise failure after goal encoding.
        world.objects.remove(actor).unwrap();
        let before = world.objects.snapshot();
        assert!(matches!(
            world.start_ai_wander_goal(1, 100, 1, [false, false, true]),
            Err(Error::UnknownObject)
        ));
        assert_eq!(world.objects.snapshot(), before);
        assert_eq!(world.goal_allocation_serial, 0);
        assert!(world.ai[&(1, 100, 1)].goals.is_empty());
    }
    #[test]
    fn ai_uses_null_player_identity_and_cannot_be_used_as_owned_inventory() {
        let mut world = world();
        let (car, content) = car();
        let spawned = world
            .spawn_ai_vehicle(1, 100, request(1), car.clone(), &content)
            .unwrap()
            .unwrap();
        let ids = spawned.ids;
        let Some(Initial::Participant(p)) = &spawned.records[0].initial else {
            panic!()
        };
        assert_eq!(p.player, 0);
        assert_eq!(p.identity.identity.persona, 0);
        assert!(p.identity.identity.bytes.is_empty());
        assert!(!world.owns_participant(1, 100, ids.participant));
        assert!(world.owned_vehicle(1, 100, ids.participant, 1).is_none());
        assert!(world.player_vehicle_item(1, 100, 1, ids.vehicle).is_none());
        assert!(world.create_actor(1, 100, ids.participant).is_err());
        assert!(
            world
                .spawn_ai_vehicle(1, 100, request(1), car.clone(), &content)
                .unwrap()
                .is_none()
        );
        let b = world
            .spawn_ai_vehicle(2, 200, request(1), car, &content)
            .unwrap()
            .unwrap();
        assert_ne!(ids, b.ids);
        assert_eq!(
            world.remove_ai_vehicle(2, 100, 1).unwrap(),
            Removed::default()
        );
        assert!(
            matches!(world.objects.get(ids.participant).unwrap().current,
            Update::Participant(ParticipantUpdate{entity:Some(id),..}) if id==ids.vehicle)
        );
    }
    #[test]
    fn invalid_late_creation_and_duplicate_retry_leave_namespace_unchanged() {
        let mut world = world();
        let (car, content) = car();
        let before = world.objects.snapshot();
        let mut invalid = car.clone();
        invalid.prefix.owner = Some(8191);
        assert!(
            world
                .spawn_ai_vehicle(1, 100, request(1), invalid, &content)
                .is_err()
        );
        assert_eq!(world.objects.snapshot(), before);
        assert!(world.ai_ids(1, 100, 1).is_none());
        assert!(
            world
                .spawn_ai_vehicle(1, 200, request(1), car.clone(), &content)
                .is_err()
        );
        let first = world
            .spawn_ai_vehicle(1, 100, request(1), car.clone(), &content)
            .unwrap()
            .unwrap();
        assert_eq!(first.ids.participant, 5);
        let mut changed = request(1);
        changed.name = b"Other".to_vec();
        let before = world.objects.snapshot();
        assert!(
            world
                .spawn_ai_vehicle(1, 100, changed, car, &content)
                .is_err()
        );
        assert_eq!(world.objects.snapshot(), before);
    }
    #[test]
    fn goal_assignment_and_connection_cleanup_are_isolated_and_repeat_safe() {
        let mut world = world();
        let (car, content) = car();
        let a = world
            .spawn_ai_vehicle(1, 100, request(1), car.clone(), &content)
            .unwrap()
            .unwrap()
            .ids;
        let b = world
            .spawn_ai_vehicle(2, 200, request(1), car.clone(), &content)
            .unwrap()
            .unwrap()
            .ids;
        let mut invalid = goal();
        invalid.asset.local_index = 4;
        let before = world.objects.snapshot();
        assert!(world.start_ai_traffic_goal(1, 100, 1, invalid).is_err());
        assert_eq!(world.objects.snapshot(), before);
        assert_eq!(world.goal_allocation_serial, 0);
        let (goal, assigned) = world.start_ai_traffic_goal(1, 100, 1, goal()).unwrap();
        assert_eq!(world.goal_allocation_serial, 1);
        assert_eq!(assigned.id, a.actor);
        let independent = world.clone();
        let removed = world.remove_ai_for_connection(1).unwrap();
        assert_eq!(
            removed.deleted,
            vec![goal.id, a.vehicle, a.actor, a.participant]
        );
        assert_eq!(removed.records.len(), 2);
        assert!(removed.deleted.iter().all(|id| world.objects.retired(*id)));
        assert!(world.objects.get(b.vehicle).is_some());
        assert!(independent.objects.get(a.vehicle).is_some());
        assert_eq!(
            world.remove_ai_for_connection(1).unwrap(),
            Removed::default()
        );
        let replacement = world
            .spawn_ai_vehicle(1, 100, request(2), car, &content)
            .unwrap()
            .unwrap()
            .ids;
        assert!(replacement.participant > goal.id);
    }
    #[test]
    fn population_bound_does_not_consume_a_retry_key_or_ghost_id() {
        let mut world = world();
        let (car, content) = car();
        for i in 1..=MAX_AI as u64 {
            world
                .spawn_ai_vehicle(1, 100, request(i), car.clone(), &content)
                .unwrap();
        }
        let before = world.objects.snapshot();
        assert!(matches!(
            world.spawn_ai_vehicle(1, 100, request(999), car.clone(), &content),
            Err(Error::Bound)
        ));
        assert_eq!(world.objects.snapshot(), before);
        world.remove_ai_vehicle(1, 100, 1).unwrap();
        assert!(
            world
                .spawn_ai_vehicle(1, 100, request(999), car, &content)
                .unwrap()
                .is_some()
        );
    }

    fn managers(world: &mut Players) -> (Managers, u16) {
        let key = 912;
        let profile = sublevel::Profile::new(
            false,
            &[
                sublevel::Kind::Noop,
                sublevel::Kind::RpcPairs,
                sublevel::Kind::RpcReferences,
                sublevel::Kind::RpcReferences,
                sublevel::Kind::Rpc,
            ],
        )
        .unwrap();
        let content = sublevel::Content::new(vec![(key, profile.clone())]).unwrap();
        let creation = sublevel::ordinary::creation(
            1,
            key,
            None,
            Serial::new(7).unwrap(),
            &sublevel::ordinary::unpopulated(&profile).unwrap(),
            &content,
        )
        .unwrap();
        let scene = world.create_scenes(&[1], vec![creation], &content).unwrap()[0].id;
        (
            Managers {
                alive: State {
                    scene: key,
                    serializer: 3,
                },
                reset_volumes: State {
                    scene: key,
                    serializer: 4,
                },
                scene: key,
                participants: 2,
                actors: 1,
            },
            scene,
        )
    }

    #[test]
    fn manager_registration_is_atomic_retained_and_owned_and_death_releases_both_lists() {
        let mut world = world();
        let (managers, scene) = managers(&mut world);
        let (car, content) = car();
        let a = world
            .spawn_ai_vehicle(1, 100, request(1), car.clone(), &content)
            .unwrap()
            .unwrap()
            .ids;
        let b = world
            .spawn_ai_vehicle(2, 200, request(1), car, &content)
            .unwrap()
            .unwrap()
            .ids;
        let before = world.objects.snapshot();
        assert_eq!(
            world.bind_ai_managers(2, 100, 1, managers),
            Err(Error::UnknownObject)
        );
        // Fail after adding the participant to a pending copy, at the actor field.
        assert_eq!(
            world.bind_ai_managers(
                1,
                100,
                1,
                Managers {
                    actors: 0,
                    ..managers
                }
            ),
            Err(Error::TypeMismatch)
        );
        assert_eq!(world.objects.snapshot(), before);
        // Alive can be in another scene; an unresolved endpoint must roll back
        // both successfully prepared manager memberships.
        assert_eq!(
            world.bind_ai_managers(
                1,
                100,
                1,
                Managers {
                    alive: State {
                        scene: 999,
                        serializer: 1
                    },
                    ..managers
                }
            ),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);
        // The reset-volume collection must be a plain RPC field in a live scene.
        for reset_volumes in [
            State {
                scene: managers.scene,
                serializer: 2,
            },
            State {
                scene: 999,
                serializer: 4,
            },
        ] {
            assert!(
                world
                    .bind_ai_managers(
                        1,
                        100,
                        1,
                        Managers {
                            reset_volumes,
                            ..managers
                        }
                    )
                    .is_err()
            );
            assert_eq!(world.objects.snapshot(), before);
        }
        let bound = world.bind_ai_managers(1, 100, 1, managers).unwrap();
        let tracked = bound.tracked.unwrap();
        assert_eq!(tracked.participant, Some(a.participant));
        assert_eq!(
            tracked.call,
            crate::world_activation::Call::StreamedParticipant
        );
        assert_eq!(
            (
                tracked.endpoint.scene,
                tracked.endpoint.selector,
                tracked.endpoint.serial.value()
            ),
            (scene, 3, 7)
        );
        // Method 3 with no arguments: 5 zero padding bits after the method.
        let wire = tracked.encode().unwrap();
        let envelope = nfs_protocol::world::rpc::Envelope::decode(
            wire.span(),
            nfs_protocol::world::rpc::Limits {
                max_input_bits: 163,
                max_references: 2,
                max_payload_bytes: 7,
            },
        )
        .unwrap();
        assert_eq!(envelope.references(), &[scene, a.participant]);
        let route = envelope
            .route(nfs_protocol::world::rpc::RouteProfile::ClientReceive)
            .unwrap();
        assert_eq!((route.selector(), route.method_index()), (3, 3));
        let enter = bound.enter.unwrap();
        assert_eq!(enter.participant, a.participant);
        assert_eq!(enter.call, crate::participants::Call::Enter);
        assert_eq!((enter.endpoint.scene, enter.endpoint.selector), (scene, 2));
        enter.encode().unwrap();
        let add = bound.add.unwrap();
        let bind = bound.bind.unwrap();
        assert_eq!(
            (
                add.endpoint.scene,
                add.endpoint.selector,
                add.endpoint.serial.value()
            ),
            (scene, 1, 7)
        );
        assert_eq!(
            (
                bind.endpoint.scene,
                bind.endpoint.selector,
                bind.participant,
                bind.actor
            ),
            (scene, 0, a.participant, a.actor)
        );
        let wire = add.encode().unwrap();
        let envelope = nfs_protocol::world::rpc::Envelope::decode(
            wire.span(),
            nfs_protocol::world::rpc::Limits {
                max_input_bits: 163,
                max_references: 2,
                max_payload_bytes: 7,
            },
        )
        .unwrap();
        assert_eq!(envelope.references(), &[scene, a.participant]);
        assert_eq!(
            envelope
                .route(nfs_protocol::world::rpc::RouteProfile::ClientReceive)
                .unwrap()
                .method_index(),
            0
        );
        assert_eq!(
            crate::actors::Binding::decode(bind.encode().unwrap().span()).unwrap(),
            bind
        );
        assert_eq!(
            world.bind_ai_managers(1, 100, 1, managers).unwrap(),
            Bound::default()
        );
        assert_eq!(
            world.bind_ai_managers(
                1,
                100,
                1,
                Managers {
                    scene: 999,
                    ..managers
                }
            ),
            Err(Error::DuplicateObject)
        );
        world.bind_ai_managers(2, 200, 1, managers).unwrap();
        let removed = world.remove_ai_for_connection(1).unwrap();
        assert!(removed.deleted.contains(&a.participant));
        assert_eq!(removed.leave.len(), 1);
        assert_eq!(removed.leave[0].participant, a.participant);
        assert_eq!(removed.leave[0].call, crate::participants::Call::Leave);
        removed.leave[0].encode().unwrap();
        assert_eq!(removed.untracked.len(), 1);
        assert_eq!(removed.untracked[0].participant, Some(a.participant));
        assert_eq!(removed.untracked[0].endpoint, tracked.endpoint);
        assert_eq!(
            removed.untracked[0].call,
            crate::world_activation::Call::StreamedParticipantLeave
        );
        removed.untracked[0].encode().unwrap();
        let Initial::SubLevel { fields, .. } = &world.objects.get(scene).unwrap().initial else {
            panic!()
        };
        assert!(
            matches!(&fields[1],sublevel::Initial::RpcPairs {value,..} if value == &vec![(b.participant,b.actor)])
        );
        assert!(
            matches!(&fields[2],sublevel::Initial::RpcReferences {value,..} if value == &vec![b.participant])
        );
        assert!(
            matches!(&fields[3],sublevel::Initial::RpcReferences {value,..} if value == &vec![b.participant])
        );
        for record in world.objects.snapshot() {
            record.encode().unwrap();
        }
        world.remove_ai_for_connection(2).unwrap();
        assert_eq!(
            world.remove_ai_for_connection(2).unwrap(),
            Removed::default()
        );
    }

    #[test]
    fn failed_deletion_restores_manager_snapshots_and_ai_ownership() {
        let mut world = world();
        let (managers, _) = managers(&mut world);
        let (car, content) = car();
        let a = world
            .spawn_ai_vehicle(1, 100, request(1), car, &content)
            .unwrap()
            .unwrap()
            .ids;
        world.bind_ai_managers(1, 100, 1, managers).unwrap();
        // A separate consumer retains this vehicle. It must be released first.
        let key = 913;
        let profile = sublevel::Profile::new(
            false,
            &[sublevel::Kind::Noop, sublevel::Kind::RpcReferences],
        )
        .unwrap();
        let content = sublevel::Content::new(vec![(key, profile)]).unwrap();
        let creation = sublevel::ordinary::creation(
            2,
            key,
            None,
            Serial::new(1).unwrap(),
            &[
                sublevel::ordinary::Value::Noop,
                sublevel::ordinary::Value::References(vec![a.vehicle]),
            ],
            &content,
        )
        .unwrap();
        let blocker = world.create_scenes(&[2], vec![creation], &content).unwrap()[0].id;
        let before = world.objects.snapshot();
        assert!(world.remove_ai_for_connection(1).is_err());
        assert_eq!(world.objects.snapshot(), before);
        assert_eq!(world.ai_ids(1, 100, 1), Some(a));
        assert_eq!(
            world.bind_ai_managers(1, 100, 1, managers).unwrap(),
            Bound::default()
        );
        world.objects.remove(blocker).unwrap();
        assert_eq!(world.remove_ai_for_connection(1).unwrap().deleted.len(), 3);
    }

    #[test]
    fn alive_in_another_scene_is_retained_and_cleanup_preserves_other_members() {
        let mut world = world();
        let (mut managers, _) = managers(&mut world);
        let (car, content) = car();
        let a = world
            .spawn_ai_vehicle(1, 100, request(1), car.clone(), &content)
            .unwrap()
            .unwrap()
            .ids;
        let b = world
            .spawn_ai_vehicle(2, 200, request(1), car, &content)
            .unwrap()
            .unwrap()
            .ids;
        let key = 914;
        let profile = sublevel::Profile::new(
            false,
            &[sublevel::Kind::Noop, sublevel::Kind::RpcReferences],
        )
        .unwrap();
        let content = sublevel::Content::new(vec![(key, profile)]).unwrap();
        let creation = sublevel::ordinary::creation(
            2,
            key,
            None,
            Serial::new(9).unwrap(),
            &[
                sublevel::ordinary::Value::Noop,
                sublevel::ordinary::Value::References(vec![b.participant]),
            ],
            &content,
        )
        .unwrap();
        let scene = world.create_scenes(&[2], vec![creation], &content).unwrap()[0].id;
        managers.alive = State {
            scene: key,
            serializer: 1,
        };
        let bound = world.bind_ai_managers(1, 100, 1, managers).unwrap();
        let enter = bound.enter.unwrap();
        assert_eq!(
            (
                enter.endpoint.scene,
                enter.endpoint.selector,
                enter.endpoint.serial.value()
            ),
            (scene, 0, 9)
        );
        assert_eq!(
            world.bind_ai_managers(1, 100, 1, managers).unwrap(),
            Bound::default()
        );
        let Initial::SubLevel { fields, .. } = &world.objects.get(scene).unwrap().initial else {
            panic!()
        };
        assert!(
            matches!(&fields[1], sublevel::Initial::RpcReferences {value,..} if value == &vec![b.participant,a.participant])
        );
        let removed = world.remove_ai_vehicle(1, 100, 1).unwrap();
        assert_eq!(removed.leave[0].endpoint, enter.endpoint);
        let Initial::SubLevel { fields, .. } = &world.objects.get(scene).unwrap().initial else {
            panic!()
        };
        assert!(
            matches!(&fields[1], sublevel::Initial::RpcReferences {value,..} if value == &vec![b.participant])
        );
        for record in world.objects.snapshot() {
            record.encode().unwrap();
        }
    }

    fn rival_request(key: u64) -> Request {
        Request {
            key,
            name: b"ID_AIPERSONA_ROAMER_040".to_vec(),
            flags: [true, true],
            words: [7, 16],
            mode: 2,
        }
    }

    #[test]
    fn persistent_slots_keep_their_participant_while_cars_come_and_go() {
        let mut world = world();
        let (managers, scene) = managers(&mut world);
        let (car, content) = car();
        // Invalid managers and unknown sessions fail without mutation.
        let before = world.objects.snapshot();
        assert_eq!(
            world.create_ai_slot(
                1,
                100,
                rival_request(50),
                Managers {
                    reset_volumes: managers.alive,
                    ..managers
                }
            ),
            Err(Error::Shape)
        );
        assert_eq!(
            world.create_ai_slot(3, 100, rival_request(50), managers),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);

        let created = world
            .create_ai_slot(1, 100, rival_request(50), managers)
            .unwrap()
            .unwrap();
        assert_eq!(created.records.len(), 2);
        assert!(matches!(
            created.records[0].initial,
            Some(Initial::Participant(_))
        ));
        let Some(Initial::Actor(init)) = &created.records[1].initial else {
            panic!()
        };
        assert!(init.flag_a && init.flag_b);
        assert_eq!((init.word_a, init.word_b), (7, 16));
        let enter = created.bound.enter.unwrap();
        assert_eq!(enter.participant, created.ids.participant);
        assert!(created.bound.add.is_some() && created.bound.bind.is_some());
        assert!(created.bound.tracked.is_none());
        assert_eq!(created.ids.car, None);
        // Retry is inert; a different request under the key is rejected.
        assert!(
            world
                .create_ai_slot(1, 100, rival_request(50), managers)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            world.create_ai_slot(
                1,
                100,
                Request {
                    words: [2, 16],
                    ..rival_request(50)
                },
                managers
            ),
            Err(Error::DuplicateObject)
        );

        let first = world
            .attach_ai_slot_vehicle(1, 100, 50, 900, car.clone(), &content)
            .unwrap();
        assert_eq!(first.ids.participant, created.ids.participant);
        assert_eq!(first.ids.actor, created.ids.actor);
        assert!(matches!(
            first.records[1].update,
            Update::Participant(ParticipantUpdate { entity: Some(v), .. }) if v == first.ids.vehicle
        ));
        assert_eq!(first.tracked.participant, Some(created.ids.participant));
        assert_eq!(
            first.tracked.call,
            crate::world_activation::Call::StreamedParticipant
        );
        assert_eq!(first.tracked.endpoint.scene, scene);
        assert_eq!(world.ai_ids(1, 100, 900), Some(first.ids));
        assert_eq!(world.ai_slot(1, 100, 50).unwrap().car, Some(900));
        // One car per slot; spawn keys stay unique; other sessions are refused.
        for (c, p, slot, key) in [(1, 100, 50, 901), (1, 100, 51, 902), (2, 200, 50, 903)] {
            assert!(
                world
                    .attach_ai_slot_vehicle(c, p, slot, key, car.clone(), &content)
                    .is_err()
            );
        }
        world
            .start_ai_wander_goal(1, 100, 900, [false, false, true])
            .unwrap();
        assert!(world.release_ai_control(1, 100, 900).unwrap().is_some());

        // Removing the car keeps the slot: no Alive leave, the car leaves the
        // reset volumes, its goal and car are deleted and control is held again.
        let detached = world.remove_ai_vehicle(1, 100, 900).unwrap();
        assert!(detached.leave.is_empty());
        assert_eq!(detached.untracked.len(), 1);
        assert_eq!(
            detached.untracked[0].call,
            crate::world_activation::Call::StreamedParticipantLeave
        );
        assert_eq!(detached.deleted.len(), 2);
        assert!(detached.deleted.contains(&first.ids.vehicle));
        assert!(!detached.deleted.contains(&created.ids.participant));
        assert!(detached.records.iter().any(|r| matches!(
            r.update,
            Update::Participant(ParticipantUpdate {
                entity: Some(0),
                ..
            })
        )));
        assert!(detached.records.iter().any(|r| matches!(
            r.update,
            Update::Actor(ActorUpdate {
                flag_b: Some(true),
                ..
            })
        )));
        assert!(world.objects.get(created.ids.participant).is_some());
        assert!(world.objects.get(created.ids.actor).is_some());
        assert!(world.objects.get(first.ids.vehicle).is_none());
        assert_eq!(world.ai_ids(1, 100, 900), None);
        assert_eq!(world.ai_slot(1, 100, 50).unwrap().car, None);
        assert!(world.owns_ai_participant(1, 100, created.ids.participant));

        // A new car under the same slot is held again until its first report.
        let second = world
            .attach_ai_slot_vehicle(1, 100, 50, 901, car.clone(), &content)
            .unwrap();
        assert_ne!(second.ids.vehicle, first.ids.vehicle);
        assert!(world.release_ai_control(1, 100, 901).unwrap().is_some());
        assert_eq!(world.release_ai_control(1, 100, 901), Ok(None));

        // The other session's slot is independent.
        let other = world
            .create_ai_slot(2, 200, rival_request(50), managers)
            .unwrap()
            .unwrap();
        assert_ne!(other.ids.participant, created.ids.participant);

        let removed = world.remove_ai_slot(1, 100, 50).unwrap();
        assert_eq!(removed.leave.len(), 1);
        assert_eq!(removed.untracked.len(), 1);
        for id in [
            created.ids.participant,
            created.ids.actor,
            second.ids.vehicle,
        ] {
            assert!(removed.deleted.contains(&id));
        }
        assert_eq!(world.ai_slot(1, 100, 50), None);
        assert_eq!(world.ai_ids(1, 100, 901), None);
        assert_eq!(
            world.remove_ai_slot(1, 100, 50).unwrap(),
            Removed::default()
        );
        // A slot without a car has no reset-volume leave.
        let cleanup = world.remove_ai_for_connection(2).unwrap();
        assert_eq!(cleanup.leave.len(), 1);
        assert!(cleanup.untracked.is_empty());
        assert!(cleanup.deleted.contains(&other.ids.participant));
        for record in world.objects.snapshot() {
            record.encode().unwrap();
        }
    }
}
