// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

impl Players {
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
