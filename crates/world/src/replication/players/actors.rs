// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

impl Players {
    /// Allocate a fresh scripted path and assign it to this participant's Actor.
    /// Allocation and assignment commit together; an existing goal is not replaced.
    pub fn start_spline(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        definition: &spline::Definition,
    ) -> Result<(Record, Record), Error> {
        let actor = self
            .owned_actor(connection, persona, participant)
            .ok_or(Error::UnknownObject)?;
        let Some(state::Object {
            current: Update::Actor(current),
            ..
        }) = self.objects.get(actor)
        else {
            return Err(Error::TypeMismatch);
        };
        if current.references_a.as_ref().is_some_and(|v| !v.is_empty()) {
            return Err(Error::Shape);
        }
        let mut next = self.objects.clone();
        let goal = next.spawn(
            Initial::ScriptedSplineGoal(definition.initial()?),
            Update::ScriptedSplineGoal,
        )?;
        let assigned = next
            .change(
                actor,
                Update::Actor(ActorUpdate {
                    references_a: Some(vec![goal.id]),
                    ..ActorUpdate::default()
                }),
            )?
            .ok_or(Error::Shape)?;
        self.objects = next;
        Ok((goal, assigned))
    }
    /// Release the arrive flag and both owned Actor goal lists before deleting
    /// the completed rollout path object.
    pub fn finish_spline(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        goal: u16,
    ) -> Result<Record, Error> {
        let actor = self
            .owned_actor(connection, persona, participant)
            .ok_or(Error::UnknownObject)?;
        let Some(state::Object {
            current: Update::Actor(current),
            ..
        }) = self.objects.get(actor)
        else {
            return Err(Error::TypeMismatch);
        };
        if current.references_a.as_deref() != Some(&[goal])
            || !self
                .objects
                .get(goal)
                .is_some_and(|o| matches!(o.initial, Initial::ScriptedSplineGoal(_)))
        {
            return Err(Error::UnknownObject);
        }
        let mut next = self.objects.clone();
        let cleared = next
            .change(
                actor,
                Update::Actor(ActorUpdate {
                    flag_a: Some(false),
                    references_a: Some(Vec::new()),
                    references_b: Some(Vec::new()),
                    ..ActorUpdate::default()
                }),
            )?
            .ok_or(Error::Shape)?;
        next.remove(goal)?;
        self.objects = next;
        Ok(cleared)
    }

    /// Begin the car replacement phase of garage exit. Both goal lists must be
    /// idle: an active goal must finish through its owning lifecycle first.
    pub fn actor_departing(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Result<Option<Record>, Error> {
        let actor = self
            .owned_actor(connection, persona, participant)
            .ok_or(Error::UnknownObject)?;
        let Some(state::Object {
            current: Update::Actor(current),
            ..
        }) = self.objects.get(actor)
        else {
            return Err(Error::TypeMismatch);
        };
        if [current.references_a.as_ref(), current.references_b.as_ref()]
            .into_iter()
            .flatten()
            .any(|goals| !goals.is_empty())
        {
            return Err(Error::Shape);
        }
        self.objects.change(
            actor,
            Update::Actor(ActorUpdate {
                flag_b: Some(true),
                references_a: Some(Vec::new()),
                references_b: Some(Vec::new()),
                ..ActorUpdate::default()
            }),
        )
    }

    /// End the replacement phase once the world car reports readiness.
    pub fn actor_vehicle_ready(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Result<Option<Record>, Error> {
        let actor = self
            .owned_actor(connection, persona, participant)
            .ok_or(Error::UnknownObject)?;
        self.objects.change(
            actor,
            Update::Actor(ActorUpdate {
                flag_b: Some(false),
                ..ActorUpdate::default()
            }),
        )
    }

    pub fn create_actor(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Result<Option<Record>, Error> {
        if !self.owns_participant(connection, persona, participant) {
            return Err(Error::UnknownObject);
        }
        if self.actors.contains_key(&participant) {
            return Ok(None);
        }
        let initial = ActorInit {
            participant,
            flag_a: false,
            flag_b: false,
            word_a: u16::MAX,
            word_b: u16::MAX,
        };
        let record = self.objects.spawn(
            Initial::Actor(initial.clone()),
            Update::Actor(ActorUpdate {
                flag_a: Some(initial.flag_a),
                flag_b: Some(initial.flag_b),
                word_pair: Some([initial.word_a, initial.word_b]),
                word_c: Some(2),
                references_a: None,
                references_b: None,
            }),
        )?;
        self.actors.insert(participant, record.id);
        Ok(Some(record))
    }

    pub fn owned_actor(&self, connection: u8, persona: u64, participant: u16) -> Option<u16> {
        self.owns_participant(connection, persona, participant)
            .then(|| self.actors.get(&participant).copied())
            .flatten()
    }

    /// Set the Actor's first replicated flag when its participant arrives in
    /// the world. The wire field's full meaning is not established; keep its
    /// neutral codec name instead of conflating it with movement authority.
    pub fn actor_arriving(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Result<Option<Record>, Error> {
        let actor = self
            .owned_actor(connection, persona, participant)
            .ok_or(Error::UnknownObject)?;
        self.objects.change(
            actor,
            Update::Actor(ActorUpdate {
                flag_a: Some(true),
                ..ActorUpdate::default()
            }),
        )
    }

    pub fn bind_actor(
        &mut self,
        gameplay_key: u32,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Result<Option<crate::actors::Binding>, Error> {
        let actor = self
            .owned_actor(connection, persona, participant)
            .ok_or(Error::UnknownObject)?;
        self.objects.bind_actor(gameplay_key, participant, actor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_flags_are_owned_repeat_safe_and_do_not_abandon_goals() {
        let mut world = Players::default();
        let a = join(&mut world, 3, 101, 0);
        let b = join(&mut world, 7, 202, 0);
        let actor = world.create_actor(3, 101, a).unwrap().unwrap().id;
        world.create_actor(7, 202, b).unwrap();
        let before = world.objects.snapshot();
        for (connection, persona, participant) in [(7, 202, a), (3, 202, a), (3, 101, b)] {
            assert_eq!(
                world.actor_departing(connection, persona, participant),
                Err(Error::UnknownObject)
            );
            assert_eq!(
                world.actor_vehicle_ready(connection, persona, participant),
                Err(Error::UnknownObject)
            );
        }
        assert_eq!(world.objects.snapshot(), before);
        let departing = world.actor_departing(3, 101, a).unwrap().unwrap();
        assert_eq!(departing.id, actor);
        assert_eq!(world.actor_departing(3, 101, a), Ok(None));
        assert_eq!(
            Record::decode(departing.encode().unwrap().span(), Some(Kind::Actor))
                .unwrap()
                .record,
            departing
        );
        let ready = world.actor_vehicle_ready(3, 101, a).unwrap().unwrap();
        assert!(matches!(
            ready.update,
            Update::Actor(ActorUpdate {
                flag_b: Some(false),
                ..
            })
        ));
        assert_eq!(world.actor_vehicle_ready(3, 101, a), Ok(None));
        // Either list can contain a goal owned by another lifecycle. Do not clear it.
        for second in [false, true] {
            let mut guarded = world.clone();
            guarded
                .objects
                .change(
                    actor,
                    Update::Actor(ActorUpdate {
                        references_a: (!second).then_some(vec![71]),
                        references_b: second.then_some(vec![72]),
                        ..Default::default()
                    }),
                )
                .unwrap();
            let before = guarded.objects.snapshot();
            assert_eq!(guarded.actor_departing(3, 101, a), Err(Error::Shape));
            assert_eq!(guarded.objects.snapshot(), before);
        }
        assert_eq!(world.objects.snapshot().last(), before.last());
    }

    #[test]
    fn arrival_flag_is_owned_repeat_safe_and_keeps_other_actor_fields() {
        let mut world = Players::default();
        let a = join(&mut world, 3, 101, 0);
        let b = join(&mut world, 7, 202, 0);
        let actor = world.create_actor(3, 101, a).unwrap().unwrap().id;
        world.create_actor(7, 202, b).unwrap();
        let before = world.objects.snapshot();
        for (connection, persona, participant) in [(7, 202, a), (3, 202, a), (3, 101, b)] {
            assert_eq!(
                world.actor_arriving(connection, persona, participant),
                Err(Error::UnknownObject)
            );
        }
        assert_eq!(world.objects.snapshot(), before);
        let record = world.actor_arriving(3, 101, a).unwrap().unwrap();
        assert_eq!(record.id, actor);
        assert!(record.initial.is_none());
        assert_eq!(
            record.update,
            Update::Actor(ActorUpdate {
                flag_a: Some(true),
                ..ActorUpdate::default()
            })
        );
        let bytes = record.encode().unwrap();
        assert_eq!(
            Record::decode(bytes.span(), Some(Kind::Actor))
                .unwrap()
                .record,
            record
        );
        assert_eq!(world.actor_arriving(3, 101, a), Ok(None));
        let mut expected = before;
        let Update::Actor(update) =
            &mut expected.iter_mut().find(|r| r.id == actor).unwrap().update
        else {
            panic!()
        };
        update.flag_a = Some(true);
        assert_eq!(world.objects.snapshot(), expected);
        assert_eq!(
            world.actor_arriving(7, 202, b).unwrap().unwrap().id,
            world.owned_actor(7, 202, b).unwrap()
        );
    }

    fn gameplay(world: &mut Players) -> u16 {
        use sublevel::{gameplay, ordinary};
        let content = sublevel::Content::new(vec![(
            crate::test_data::GAMEPLAY_KEY,
            gameplay::profile().unwrap(),
        )])
        .unwrap();
        let creation = ordinary::creation(
            1,
            crate::test_data::GAMEPLAY_KEY,
            None,
            nfs_protocol::world::rpc::Serial::new(0).unwrap(),
            &ordinary::unpopulated(content.profile(crate::test_data::GAMEPLAY_KEY).unwrap())
                .unwrap(),
            &content,
        )
        .unwrap();
        world.create_scenes(&[1], vec![creation], &content).unwrap()[0].id
    }

    #[test]
    fn actor_map_is_owned_repeat_safe_and_retained_in_fresh_scene_state() {
        let mut world = Players::default();
        let scene = gameplay(&mut world);
        let a = join(&mut world, 3, 101, 0);
        let b = join(&mut world, 7, 202, 0);
        let actor_a = world.create_actor(3, 101, a).unwrap().unwrap().id;
        let actor_b = world.create_actor(7, 202, b).unwrap().unwrap().id;
        let before = world.objects.snapshot();
        for (connection, persona, participant) in [(7, 202, a), (3, 202, a), (3, 101, b)] {
            assert_eq!(
                world.bind_actor(
                    crate::test_data::GAMEPLAY_KEY,
                    connection,
                    persona,
                    participant
                ),
                Err(Error::UnknownObject)
            );
        }
        assert_eq!(world.objects.snapshot(), before);
        let reply = world
            .bind_actor(crate::test_data::GAMEPLAY_KEY, 3, 101, a)
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                reply.endpoint.scene,
                reply.endpoint.selector,
                reply.participant,
                reply.actor
            ),
            (scene, 22, a, actor_a)
        );
        assert_eq!(
            world.bind_actor(crate::test_data::GAMEPLAY_KEY, 3, 101, a),
            Ok(None)
        );
        assert_eq!(
            world.bind_actor(crate::test_data::GAMEPLAY_KEY, 7, 202, a),
            Err(Error::UnknownObject)
        );
        world
            .bind_actor(crate::test_data::GAMEPLAY_KEY, 7, 202, b)
            .unwrap()
            .unwrap();
        let snapshot = world.objects.snapshot();
        let Some(Initial::SubLevel { fields, .. }) = &snapshot[0].initial else {
            panic!()
        };
        assert!(
            matches!(&fields[24], sublevel::Initial::RpcPairs { value, .. }
            if *value == vec![(a, actor_a), (b, actor_b)])
        );
        assert_eq!(world.objects.remove(actor_a), Err(Error::UnknownObject));
        assert_eq!(world.objects.snapshot(), snapshot);
        let mut other = Players::default();
        gameplay(&mut other);
        assert_eq!(other.objects.snapshot(), before[..1]);
    }

    #[test]
    fn missing_actor_or_scene_does_not_mutate_association() {
        let mut world = Players::default();
        let p = join(&mut world, 3, 101, 0);
        assert_eq!(
            world.bind_actor(crate::test_data::GAMEPLAY_KEY, 3, 101, p),
            Err(Error::UnknownObject)
        );
        world.create_actor(3, 101, p).unwrap();
        let before = world.objects.snapshot();
        assert_eq!(
            world.bind_actor(crate::test_data::GAMEPLAY_KEY, 3, 101, p),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);
        gameplay(&mut world);
        assert!(
            world
                .bind_actor(crate::test_data::GAMEPLAY_KEY, 3, 101, p)
                .unwrap()
                .is_some()
        );
    }

    fn join(world: &mut Players, connection: u8, persona: u64, slot: u8) -> u16 {
        let player = world
            .create(
                connection,
                persona,
                Request {
                    name: b"Driver".to_vec(),
                    flag: false,
                    slot,
                },
            )
            .unwrap()
            .unwrap()
            .id;
        world.join(connection, persona, player).unwrap().unwrap().id
    }

    #[test]
    fn ownership_precedes_repeat_lookup_and_rejections_preserve_allocation() {
        let mut world = Players::default();
        let a = join(&mut world, 3, 101, 0);
        let b = join(&mut world, 7, 202, 0);
        let before = world.objects.snapshot();
        for (connection, persona, participant) in [
            (7, 202, a),
            (3, 202, a),
            (3, 101, 1),
            (3, 101, 0),
            (3, 101, 8191),
        ] {
            assert_eq!(
                world.create_actor(connection, persona, participant),
                Err(Error::UnknownObject)
            );
        }
        assert_eq!(world.objects.snapshot(), before);
        let actor = world.create_actor(3, 101, a).unwrap().unwrap();
        assert_eq!(actor.id, 5);
        assert_eq!(world.owned_actor(3, 101, a), Some(5));
        assert_eq!(world.owned_actor(7, 202, a), None);
        assert_eq!(world.create_actor(7, 202, a), Err(Error::UnknownObject));
        assert_eq!(world.create_actor(3, 101, a).unwrap(), None);
        assert_eq!(world.create_actor(7, 202, b).unwrap().unwrap().id, 6);
        assert_eq!(world.objects.snapshot()[4], actor);
        let mut fresh = Players::default();
        let current = join(&mut fresh, 3, 101, 0);
        assert_eq!(fresh.owned_actor(3, 101, current), None);
        assert!(fresh.create_actor(3, 101, current).unwrap().is_some());
    }

    #[test]
    fn two_local_slots_and_worlds_keep_separate_actor_associations() {
        let mut world = Players::default();
        let a = join(&mut world, 1, 101, 0);
        let b = join(&mut world, 1, 101, 1);
        let first = world.create_actor(1, 101, a).unwrap().unwrap();
        let second = world.create_actor(1, 101, b).unwrap().unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(world.owned_actor(1, 101, a), Some(first.id));
        assert_eq!(world.owned_actor(1, 101, b), Some(second.id));
        assert_eq!(Players::default().owned_actor(1, 101, a), None);
        let bytes = second.encode().unwrap();
        assert_eq!(Record::decode(bytes.span(), None).unwrap().record, second);
    }

    #[test]
    fn exhausted_namespace_does_not_record_an_actor_that_was_not_spawned() {
        let mut world = Players::default();
        let participant = join(&mut world, 1, 101, 0);
        let object = world.objects.get(participant).unwrap().clone();
        for _ in 2..MAX_OBJECTS {
            world
                .objects
                .spawn(object.initial.clone(), object.current.clone())
                .unwrap();
        }
        assert_eq!(world.create_actor(1, 101, participant), Err(Error::Bound));
        assert_eq!(world.owned_actor(1, 101, participant), None);
        assert_eq!(world.objects.len(), MAX_OBJECTS);
    }
}
