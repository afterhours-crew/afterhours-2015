// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

impl Players {
    /// Acknowledge a driver's nitrous transition without replacing its fuel,
    /// equipment, motion or binding. Identical repeats produce no new delta.
    pub fn set_vehicle_nitrous_mode(
        &mut self,
        connection: u8,
        persona: u64,
        id: u16,
        active: bool,
    ) -> Result<Option<Record>, Error> {
        if !self.owned_vehicles(connection, persona).contains(&id) {
            return Err(Error::UnknownObject);
        }
        let Some(state::Object {
            current: Update::Vehicle { binding, fields },
            ..
        }) = self.objects.get(id)
        else {
            return Err(Error::TypeMismatch);
        };
        if fields
            .iter()
            .filter(|f| matches!(f, Some(vehicle::Update::Index { .. })))
            .count()
            != 1
        {
            return Err(Error::Shape);
        }
        let updates = fields
            .iter()
            .map(|field| {
                matches!(field, Some(vehicle::Update::Index { .. })).then_some(
                    vehicle::Update::Index {
                        index: Some(u8::from(active)),
                        value: None,
                    },
                )
            })
            .collect();
        self.objects.change(
            id,
            Update::Vehicle {
                binding: binding.clone(),
                fields: updates,
            },
        )
    }

    /// Publish only inventory-derived equipment fields, validating the complete
    /// delta before changing the owned object. Creation and motion are forbidden.
    pub fn set_vehicle_equipment(
        &mut self,
        connection: u8,
        persona: u64,
        id: u16,
        body: vehicle::Body,
    ) -> Result<Option<Record>, Error> {
        if !self.owned_vehicles(connection, persona).contains(&id) {
            return Err(Error::UnknownObject);
        }
        let Some(state::Object {
            current: Update::Vehicle { binding, fields },
            ..
        }) = self.objects.get(id)
        else {
            return Err(Error::TypeMismatch);
        };
        if body.creation.is_some() || body.updates.len() != fields.len() {
            return Err(Error::Shape);
        }
        let mut chassis = 0;
        for update in body.updates.iter().flatten() {
            match update {
                vehicle::Update::Chassis(c)
                    if c.physics.is_none()
                        && c.flag.is_none()
                        && c.custom.is_some()
                        && c.pair.is_none() =>
                {
                    chassis += 1
                }
                vehicle::Update::Tuning(Some(_))
                | vehicle::Update::Appearance(_)
                | vehicle::Update::Mesh(Some(_)) => (),
                _ => return Err(Error::Shape),
            }
        }
        if chassis != 1 {
            return Err(Error::Shape);
        }
        self.objects.change(
            id,
            Update::Vehicle {
                binding: binding.clone(),
                fields: body.updates,
            },
        )
    }
    /// Resolve a live car through this exact owned player's participant.
    pub fn player_vehicle_item(
        &self,
        connection: u8,
        persona: u64,
        player: u16,
        vehicle: u16,
    ) -> Option<(u16, u64)> {
        if !self.owns(connection, persona, player) {
            return None;
        }
        let participant = *self.participants.get(&player)?;
        self.vehicles.iter().find_map(|(&(owner, item), &ghost)| {
            (owner == participant && ghost == vehicle).then_some((participant, item))
        })
    }

    /// Publish only the owned car's chassis customization. Motion, health,
    /// authority, identity and every other component remain unchanged.
    pub fn set_vehicle_customization(
        &mut self,
        connection: u8,
        persona: u64,
        id: u16,
        custom: vehicle::Customization,
    ) -> Result<Option<Record>, Error> {
        if !self.owned_vehicles(connection, persona).contains(&id) {
            return Err(Error::UnknownObject);
        }
        let Some(state::Object {
            current: Update::Vehicle { binding, fields },
            ..
        }) = self.objects.get(id)
        else {
            return Err(Error::TypeMismatch);
        };
        if fields
            .iter()
            .filter(|f| matches!(f, Some(vehicle::Update::Chassis(_))))
            .count()
            != 1
        {
            return Err(Error::Shape);
        }
        let updates = fields
            .iter()
            .map(|field| {
                if matches!(field, Some(vehicle::Update::Chassis(_))) {
                    Some(vehicle::Update::Chassis(Box::new(vehicle::ChassisUpdate {
                        physics: None,
                        flag: None,
                        custom: Some(custom.clone()),
                        pair: None,
                    })))
                } else {
                    None
                }
            })
            .collect();
        self.objects.change(
            id,
            Update::Vehicle {
                binding: binding.clone(),
                fields: updates,
            },
        )
    }

    /// Current chassis state, only for a car owned by this connection/persona.
    pub fn vehicle_physics(
        &self,
        connection: u8,
        persona: u64,
        id: u16,
    ) -> Result<vehicle::Physics, Error> {
        if !self.owned_vehicles(connection, persona).contains(&id) {
            return Err(Error::UnknownObject);
        }
        let Some(state::Object {
            current: Update::Vehicle { fields, .. },
            ..
        }) = self.objects.get(id)
        else {
            return Err(Error::TypeMismatch);
        };
        let mut matches = fields.iter().filter_map(|field| match field {
            Some(vehicle::Update::Chassis(c)) => c.physics.as_ref(),
            _ => None,
        });
        let physics = matches.next().ok_or(Error::Shape)?.clone();
        if matches.next().is_some() {
            return Err(Error::Shape);
        }
        Ok(physics)
    }
    /// Change only the chassis physics field. Other component state and the
    /// car's asset/participant binding are retained by the world model.
    pub fn set_vehicle_physics(
        &mut self,
        connection: u8,
        persona: u64,
        id: u16,
        physics: vehicle::Physics,
    ) -> Result<Option<Record>, Error> {
        self.vehicle_physics(connection, persona, id)?;
        let Some(state::Object {
            current: Update::Vehicle { binding, fields },
            ..
        }) = self.objects.get(id)
        else {
            return Err(Error::TypeMismatch);
        };
        let updates = fields
            .iter()
            .map(|f| {
                if matches!(f, Some(vehicle::Update::Chassis(_))) {
                    Some(vehicle::Update::Chassis(Box::new(vehicle::ChassisUpdate {
                        physics: Some(physics.clone()),
                        flag: None,
                        custom: None,
                        pair: None,
                    })))
                } else {
                    None
                }
            })
            .collect();
        self.objects.change(
            id,
            Update::Vehicle {
                binding: binding.clone(),
                fields: updates,
            },
        )
    }

    /// Vehicles of the participants owned by `connection` and `persona`: the
    /// cars that connection's client simulates.
    pub fn owned_vehicles(&self, connection: u8, persona: u64) -> std::collections::BTreeSet<u16> {
        self.vehicles
            .iter()
            .chain(self.staged_vehicles.iter())
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
        // A caller must publish the detach before deleting a referenced car.
        if matches!(self.objects.get(participant).map(|o| &o.current),
            Some(Update::Participant(p)) if p.entity == Some(id))
        {
            return Err(Error::Shape);
        }
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

    pub fn staged_vehicle(
        &self,
        connection: u8,
        persona: u64,
        participant: u16,
        item: u64,
    ) -> Option<u16> {
        self.owns_participant(connection, persona, participant)
            .then(|| self.staged_vehicles.get(&(participant, item)).copied())
            .flatten()
    }
    /// Prepare one replacement without changing the driven vehicle binding.
    pub fn stage_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        item: u64,
        creation: vehicle::EntityCreation,
        content: &entity::creation::Content,
    ) -> Result<Record, Error> {
        if self
            .owned_vehicle(connection, persona, participant, item)
            .is_none()
        {
            return Err(Error::UnknownObject);
        }
        if self.staged_vehicles.contains_key(&(participant, item)) {
            return Err(Error::DuplicateObject);
        }
        // At most one replacement for each bounded, existing owned vehicle.
        let record = self.objects.spawn_vehicle(creation, content)?;
        self.staged_vehicles.insert((participant, item), record.id);
        Ok(record)
    }
    /// Commit a prepared replacement only after publishing a participant/view detach.
    pub fn promote_staged_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        item: u64,
    ) -> Result<u16, Error> {
        let id = self
            .staged_vehicle(connection, persona, participant, item)
            .ok_or(Error::UnknownObject)?;
        let removed = self.remove_vehicle(connection, persona, participant, item)?;
        self.staged_vehicles.remove(&(participant, item));
        self.vehicles.insert((participant, item), id);
        Ok(removed)
    }
}
