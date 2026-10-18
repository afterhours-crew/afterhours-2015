// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AI lifecycle in the world's shared object namespace. AI participants have
//! no Player or owned inventory entry. The host supplies actor configuration;
//! this layer does not infer delegation or movement policy from a mode number.
use super::*;

pub const MAX_AI: usize = 128;

/// Installed serializer ordinals, supplied by content preparation. They are
/// distinct from RPC selector numbers and allocated scene ghost IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Managers {
    pub scene: u32,
    pub participants: usize,
    pub actors: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bound {
    pub add: Option<crate::participants::Notification>,
    pub bind: Option<crate::actors::Binding>,
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
    /// Detach relations before publishing the deletion list.
    pub records: Vec<Record>,
    pub deleted: Vec<u16>,
}

#[derive(Clone, Debug)]
pub(super) struct Instance {
    request: Request,
    creation: vehicle::EntityCreation,
    ids: Ids,
    goal: Option<u16>,
    managers: Option<Managers>,
}

impl Players {
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
        if request.key == 0
            || request.name.is_empty()
            || request.name.contains(&0)
            || request.mode > 2
        {
            return Err(Error::Shape);
        }
        if request.name.len() > 64 {
            return Err(Error::Bound);
        }
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
                goal: None,
                managers: None,
            },
        );
        Ok(Some(Spawned {
            ids,
            records: vec![participant, actor, vehicle, attached],
        }))
    }

    /// Bind the AI participant and actor to their installed scene managers.
    /// Retained snapshots and both RPCs commit together; retry is inert.
    pub fn bind_ai_managers(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
        managers: Managers,
    ) -> Result<Bound, Error> {
        let instance = self
            .ai
            .get(&(connection, persona, key))
            .ok_or(Error::UnknownObject)?;
        if instance.managers.is_some_and(|old| old != managers) {
            return Err(Error::DuplicateObject);
        }
        let bound = self.objects.bind_ai_managers(
            managers,
            instance.ids.participant,
            instance.ids.actor,
        )?;
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
        if instance.goal.is_some() {
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
            .goal = Some(goal.id);
        Ok((goal, assigned))
    }

    /// Retire one host spawn; repeated removal is inert. Manager snapshots release
    /// their references atomically with deletion, matching client death listeners.
    pub fn remove_ai_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        key: u64,
    ) -> Result<Removed, Error> {
        let Some(instance) = self.ai.get(&(connection, persona, key)) else {
            return Ok(Removed::default());
        };
        let mut next = self.objects.clone();
        let mut result = Removed::default();
        if let Some(managers) = instance.managers {
            next.release_ai_managers(managers, instance.ids.participant, instance.ids.actor)?;
        }
        result.records.extend(next.change(
            instance.ids.participant,
            Update::Participant(ParticipantUpdate {
                entity: Some(0),
                ..ParticipantUpdate::default()
            }),
        )?);
        if let Some(goal) = instance.goal {
            result.records.extend(next.change(
                instance.ids.actor,
                Update::Actor(ActorUpdate {
                    references_a: Some(Vec::new()),
                    references_b: Some(Vec::new()),
                    ..ActorUpdate::default()
                }),
            )?);
            next.remove(goal)?;
            result.deleted.push(goal);
        }
        for id in [
            instance.ids.vehicle,
            instance.ids.actor,
            instance.ids.participant,
        ] {
            next.remove(id)?;
            result.deleted.push(id);
        }
        self.objects = next;
        self.ai.remove(&(connection, persona, key));
        Ok(result)
    }

    /// Session cleanup is all-or-nothing even when a retained scene reference
    /// prevents one of this connection's AI participants from being retired.
    pub fn remove_ai_for_connection(&mut self, connection: u8) -> Result<Removed, Error> {
        let keys: Vec<_> = self
            .ai
            .keys()
            .filter(|(c, _, _)| *c == connection)
            .copied()
            .collect();
        let mut next = self.clone();
        let mut result = Removed::default();
        for (c, p, k) in keys {
            let removed = next.remove_ai_vehicle(c, p, k)?;
            result.records.extend(removed.records);
            result.deleted.extend(removed.deleted);
        }
        *self = next;
        Ok(result)
    }
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
        assert!(world.ai[&(1, 100, 1)].goal.is_none());
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
        let bound = world.bind_ai_managers(1, 100, 1, managers).unwrap();
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
        let Initial::SubLevel { fields, .. } = &world.objects.get(scene).unwrap().initial else {
            panic!()
        };
        assert!(
            matches!(&fields[1],sublevel::Initial::RpcPairs {value,..} if value == &vec![(b.participant,b.actor)])
        );
        assert!(
            matches!(&fields[2],sublevel::Initial::RpcReferences {value,..} if value == &vec![b.participant])
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
}
