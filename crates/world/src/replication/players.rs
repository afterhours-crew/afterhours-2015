// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use std::collections::BTreeMap;

mod actors;
mod vehicles;

#[derive(Clone, Eq, PartialEq)]
pub struct Request {
    pub name: Vec<u8>,
    pub flag: bool,
    pub slot: u8,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("name_bytes", &self.name.len())
            .field("flag", &self.flag)
            .field("slot", &self.slot)
            .finish()
    }
}

#[derive(Clone, Debug)]
struct Player {
    identity: Identity,
    request: Request,
}

#[derive(Clone, Debug, Default)]
pub struct Players {
    objects: state::World,
    players: BTreeMap<(u8, u8), Player>,
    participants: BTreeMap<u16, u16>,
    actors: BTreeMap<u16, u16>,
    vehicles: BTreeMap<(u16, u64), u16>,
}

/// Native runtime index of the player on `connection` and local `slot`.
/// Official hosts give a connection's player index `connection - 1`: the E742
/// and E101 rosters map value8 1..6 to 0..5, and the E101 client on connection
/// 5 was answered as player 4 (E762). Further local slots are not observed;
/// they take a stride of 16 so sixteen connections fill the 7-bit width.
fn runtime_index(connection: u8, slot: u8) -> Option<u8> {
    let index = u16::from(connection)
        .checked_sub(1)?
        .checked_add(16 * u16::from(slot))?;
    u8::try_from(index).ok().filter(|i| *i < 128)
}

impl Players {
    pub fn objects(&self) -> &state::World {
        &self.objects
    }

    pub fn owns(&self, connection: u8, persona: u64, id: u16) -> bool {
        self.objects.get(id).is_some_and(|object| matches!(&object.initial,
            Initial::Player(player) if player.value8 == connection && player.identity.persona == persona))
    }

    pub fn owns_participant(&self, connection: u8, persona: u64, id: u16) -> bool {
        self.participants.iter().any(|(&player, &participant)| {
            participant == id && self.owns(connection, persona, player)
        })
    }

    pub fn join(
        &mut self,
        connection: u8,
        persona: u64,
        player: u16,
    ) -> Result<Option<Record>, Error> {
        if !self.owns(connection, persona, player) {
            return Err(Error::UnknownObject);
        }
        if self.participants.contains_key(&player) {
            return Ok(None);
        }
        let Some(state::Object {
            initial: Initial::Player(value),
            ..
        }) = self.objects.get(player)
        else {
            return Err(Error::TypeMismatch);
        };
        let identity = NamedIdentity {
            identity: value.identity.clone(),
            name: value.name.clone(),
        };
        let record = self.objects.spawn(
            Initial::Participant(ParticipantInit {
                selector: 0,
                serial: 0,
                identity: identity.clone(),
                player,
            }),
            Update::Participant(ParticipantUpdate {
                player: Some(player),
                entity: None,
                identity: Some(identity),
            }),
        )?;
        self.participants.insert(player, record.id);
        Ok(Some(record))
    }

    pub fn create_scenes(
        &mut self,
        levels: &[u16],
        creations: Vec<sublevel::Creation>,
        content: &sublevel::Content,
    ) -> Result<Vec<Record>, Error> {
        if creations.len() > sublevel::MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut next = self.objects.clone();
        next.register_levels(levels)?;
        let mut records = Vec::with_capacity(creations.len());
        for creation in creations {
            records.push(next.spawn_scene(creation, content)?);
        }
        self.objects = next;
        Ok(records)
    }

    pub fn spawn_entities(
        &mut self,
        creations: Vec<entity::creation::Creation>,
        content: &entity::creation::Content,
    ) -> Result<Vec<Record>, Error> {
        if creations.len() > entity::creation::MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut next = self.objects.clone();
        let mut records = Vec::with_capacity(creations.len());
        for creation in creations {
            records.push(next.spawn_entity(creation, content)?);
        }
        self.objects = next;
        Ok(records)
    }

    pub fn runtime_index(&self, connection: u8, persona: u64, participant: u16) -> Option<u8> {
        if !self.owns_participant(connection, persona, participant) {
            return None;
        }
        let player = self
            .participants
            .iter()
            .find(|(_, p)| **p == participant)
            .map(|(player, _)| *player)?;
        match &self.objects.get(player)?.initial {
            Initial::Player(init) => Some(init.runtime_index),
            _ => None,
        }
    }
    pub fn local_slot(&self, connection: u8, persona: u64, participant: u16) -> Option<u8> {
        if !self.owns_participant(connection, persona, participant) {
            return None;
        }
        let player = self
            .participants
            .iter()
            .find(|(_, p)| **p == participant)?
            .0;
        match &self.objects.get(*player)?.initial {
            Initial::Player(init) => Some(init.value255),
            _ => None,
        }
    }
    pub fn change(&mut self, id: u16, delta: Update) -> Result<Option<Record>, Error> {
        self.objects.change(id, delta)
    }
    pub fn remove_entities(&mut self, ids: &[u16]) -> Result<(), Error> {
        if ids.len() > entity::creation::MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut next = self.objects.clone();
        for &id in ids {
            let object = next.remove(id)?;
            if !matches!(object.initial, Initial::Entity { .. }) {
                return Err(Error::TypeMismatch);
            }
        }
        self.objects = next;
        Ok(())
    }

    pub fn create(
        &mut self,
        connection: u8,
        persona: u64,
        request: Request,
    ) -> Result<Option<Record>, Error> {
        if connection == 0 || persona == 0 {
            return Err(Error::Shape);
        }
        if request.flag || request.slot > 7 {
            return Err(Error::Unsupported);
        }
        if request.name.len() > 16 {
            return Err(Error::Bound);
        }
        if request.name.contains(&0) {
            return Err(Error::Shape);
        }
        let name = if request.name.is_empty() {
            format!("Player{connection}").into_bytes()
        } else {
            request.name.clone()
        };
        let identity = Identity {
            persona,
            bytes: name.clone(),
        };
        let key = (connection, request.slot);
        if let Some(player) = self.players.get(&key) {
            return if player.request == request && player.identity == identity {
                Ok(None)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.players.len() >= 128 {
            return Err(Error::Bound);
        }
        let index = runtime_index(connection, request.slot).ok_or(Error::Bound)?;
        if self
            .players
            .keys()
            .any(|&(c, s)| runtime_index(c, s) == Some(index))
        {
            return Err(Error::DuplicateObject);
        }
        let initial = Initial::Player(PlayerInit {
            value8: connection,
            name: name.clone(),
            value255: request.slot,
            runtime_index: index,
            identity: identity.clone(),
            flag_a: false,
            flag_b: false,
        });
        let empty_identity = Identity {
            persona: 0,
            bytes: Vec::new(),
        };
        let update = Update::Player(PlayerUpdate {
            base: Some(PlayerBaseUpdate {
                value16: Some(0),
                name_identity: Some(NamedIdentity {
                    name,
                    identity: identity.clone(),
                }),
                identity_a: Some(empty_identity.clone()),
                value8: Some(connection),
                configured_pair: Some((31, 262143)),
                empty_assets: true,
                empty_structured: true,
                asset: Some(EmptyAsset::Implicit),
                // Created for its own connection: the official host marks the
                // receiving client's Player this way (E763 analysis).
                empty_local: true,
                identity_b: Some(empty_identity),
            }),
            component_present: true,
        });
        let record = self.objects.spawn(initial, update)?;
        self.players.insert(key, Player { identity, request });
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            name: b"LocalPlayer".to_vec(),
            flag: false,
            slot: 0,
        }
    }
    #[test]
    fn owned_connections_allocate_distinct_ids_and_repeat_without_output() {
        let mut world = Players::default();
        let one = world.create(1, 100, request()).unwrap().unwrap();
        let two = world.create(2, 200, request()).unwrap().unwrap();
        assert_eq!((one.id, two.id), (1, 2));
        let Some(Initial::Player(p)) = &two.initial else {
            panic!()
        };
        assert_eq!(
            (p.value8, p.value255, p.runtime_index, p.identity.persona),
            (2, 0, 1, 200)
        );
        assert_eq!(world.create(1, 100, request()).unwrap(), None);
        assert_eq!(world.objects().len(), 2);
        assert_eq!(
            Players::default().create(1, 100, request()).unwrap(),
            Some(one)
        );
        assert_eq!(world.objects().snapshot()[1], two);
    }
    #[test]
    fn rejection_is_transactional_and_does_not_consume_ids() {
        let mut world = Players::default();
        for request in [
            Request {
                flag: true,
                ..request()
            },
            Request {
                slot: 255,
                ..request()
            },
            Request {
                name: vec![b'x'; 17],
                ..request()
            },
            Request {
                name: vec![0],
                ..request()
            },
        ] {
            assert!(world.create(1, 100, request).is_err());
            assert!(world.objects().is_empty());
        }
        assert!(world.create(0, 100, request()).is_err());
        assert!(world.create(1, 0, request()).is_err());
        let first = world.create(1, 100, request()).unwrap().unwrap();
        assert_eq!(first.id, 1);
        assert_eq!(world.create(1, 101, request()), Err(Error::DuplicateObject));
        assert_eq!(world.objects().snapshot(), vec![first]);
    }
    #[test]
    fn native_runtime_width_bounds_the_world_and_slots_remain_separate() {
        let mut world = Players::default();
        for i in 0..128u16 {
            let request = Request {
                slot: (i % 8) as u8,
                ..request()
            };
            world
                .create((i / 8 + 1) as u8, 100 + u64::from(i), request)
                .unwrap();
        }
        assert_eq!(world.objects().len(), 128);
        assert_eq!(world.create(17, 999, request()), Err(Error::Bound));
        assert_eq!(world.objects().len(), 128);
    }
    #[test]
    fn requested_name_and_identity_are_current_and_missing_names_have_owned_fallback() {
        let mut world = Players::default();
        let record = world
            .create(
                3,
                1234,
                Request {
                    name: Vec::new(),
                    ..request()
                },
            )
            .unwrap()
            .unwrap();
        let Some(Initial::Player(p)) = &record.initial else {
            panic!()
        };
        assert_eq!(p.name, b"Player3");
        assert_eq!(p.identity.bytes, p.name);
        let wire = record.encode().unwrap();
        assert_eq!(Record::decode(wire.span(), None).unwrap().record, record);
    }

    #[test]
    fn runtime_index_is_the_connection_less_one_as_on_official_hosts() {
        let mut world = Players::default();
        for (connection, persona, expected) in [(5, 500, 4), (4, 400, 3), (1, 100, 0)] {
            let record = world
                .create(connection, persona, request())
                .unwrap()
                .unwrap();
            let Some(Initial::Player(p)) = &record.initial else {
                panic!()
            };
            assert_eq!((p.value8, p.runtime_index), (connection, expected));
        }
        // A second slot on connection 1 takes index 16; connection 17 would
        // need the same index and is refused without consuming an id.
        let slot = Request {
            slot: 1,
            ..request()
        };
        let second = world.create(1, 101, slot).unwrap().unwrap();
        let Some(Initial::Player(p)) = &second.initial else {
            panic!()
        };
        assert_eq!(p.runtime_index, 16);
        let before = world.objects().len();
        assert_eq!(
            world.create(17, 1700, request()),
            Err(Error::DuplicateObject)
        );
        assert_eq!(world.objects().len(), before);
        assert_eq!(runtime_index(129, 0), None);
        assert_eq!(runtime_index(1, 8), None);
        assert_eq!(runtime_index(0, 0), None);
    }
    #[test]
    fn own_player_carries_the_empty_local_group_and_entries_stay_unsupported() {
        let mut world = Players::default();
        let record = world.create(4, 400, request()).unwrap().unwrap();
        let Update::Player(PlayerUpdate {
            base: Some(base), ..
        }) = &record.update
        else {
            panic!()
        };
        assert!(base.empty_local);
        let wire = record.encode().unwrap();
        assert_eq!(Record::decode(wire.span(), None).unwrap().record, record);
        // Remote players (E132) carry only the group's absent presence bit.
        let mut remote = record.clone();
        let Update::Player(PlayerUpdate {
            base: Some(base), ..
        }) = &mut remote.update
        else {
            panic!()
        };
        base.empty_local = false;
        let remote_wire = remote.encode().unwrap();
        assert_eq!(wire.len(), remote_wire.len() + 7);
        assert_eq!(
            Record::decode(remote_wire.span(), None).unwrap().record,
            remote
        );
        // The first differing bit is the presence bit; a count of one entry
        // is not modeled.
        let (own, other) = (wire.span(), remote_wire.span());
        let at = (0..other.len())
            .find(|&i| own.read_u32(i, 1) != other.read_u32(i, 1))
            .unwrap();
        let mut entry = crate::bits::BitWriter::new();
        for i in 0..own.len() {
            let bit = if i == at + 7 {
                1
            } else {
                own.read_u32(i, 1).unwrap()
            };
            entry.put(u64::from(bit), 1);
        }
        assert_eq!(
            Record::decode(entry.span(), None).map(|_| ()),
            Err(Error::Unsupported)
        );
    }
    #[test]
    fn runtime_index_follows_the_owning_player_and_ownership() {
        let mut world = Players::default();
        let one = world.create(1, 100, request()).unwrap().unwrap();
        let two = world.create(2, 200, request()).unwrap().unwrap();
        let a = world.join(1, 100, one.id).unwrap().unwrap().id;
        let b = world.join(2, 200, two.id).unwrap().unwrap().id;
        assert_eq!(world.runtime_index(1, 100, a), Some(0));
        assert_eq!(world.runtime_index(2, 200, b), Some(1));
        assert_eq!(world.runtime_index(1, 100, b), None);
        assert_eq!(world.runtime_index(1, 100, one.id), None);
        assert_eq!(world.runtime_index(1, 100, 8000), None);
    }
}
