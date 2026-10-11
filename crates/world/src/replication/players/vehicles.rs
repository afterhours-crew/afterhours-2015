// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

impl Players {
    /// Vehicles of the participants owned by `connection` and `persona`: the
    /// cars that connection's client simulates.
    pub fn owned_vehicles(&self, connection: u8, persona: u64) -> std::collections::BTreeSet<u16> {
        self.vehicles
            .iter()
            .filter(|((participant, _), _)| {
                self.owns_participant(connection, persona, *participant)
            })
            .map(|(_, id)| *id)
            .collect()
    }
    pub fn owned_identity(
        &self,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Option<NamedIdentity> {
        if !self.owns_participant(connection, persona, participant) {
            return None;
        }
        match &self.objects.get(participant)?.initial {
            Initial::Participant(value) => Some(value.identity.clone()),
            _ => None,
        }
    }

    pub fn owned_vehicle(
        &self,
        connection: u8,
        persona: u64,
        participant: u16,
        item: u64,
    ) -> Option<u16> {
        self.owns_participant(connection, persona, participant)
            .then(|| self.vehicles.get(&(participant, item)).copied())
            .flatten()
    }

    /// Remove the participant's vehicle for `item` (garage display car before
    /// its world re-creation). Returns the removed ghost id.
    pub fn remove_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        item: u64,
    ) -> Result<u16, Error> {
        if !self.owns_participant(connection, persona, participant) {
            return Err(Error::UnknownObject);
        }
        let id = *self
            .vehicles
            .get(&(participant, item))
            .ok_or(Error::UnknownObject)?;
        let mut next = self.objects.clone();
        let object = next.remove(id)?;
        if !matches!(object.initial, Initial::Vehicle { .. }) {
            return Err(Error::TypeMismatch);
        }
        self.objects = next;
        self.vehicles.remove(&(participant, item));
        Ok(id)
    }
    pub fn create_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        item: u64,
        creation: vehicle::EntityCreation,
        content: &entity::creation::Content,
    ) -> Result<Record, Error> {
        if self.owned_actor(connection, persona, participant).is_none() {
            return Err(Error::UnknownObject);
        }
        if item == 0 {
            return Err(Error::Shape);
        }
        if self.vehicles.contains_key(&(participant, item)) {
            return Err(Error::DuplicateObject);
        }
        let record = self.objects.spawn_vehicle(creation, content)?;
        self.vehicles.insert((participant, item), record.id);
        Ok(record)
    }
}
