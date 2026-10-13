// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::super::location_map::{Coordinate, Entry, Group};
use super::*;

impl Players {
    /// Publish an owner's locations and bind the resulting human map to the
    /// configured location service. The map precedes its reference in the
    /// returned batch. Returning to the garage clears both atomically; a later
    /// exit reuses the map and restores the binding. Other map slots are kept.
    pub fn sync_location_service(
        &mut self,
        connection: u8,
        persona: u64,
        cars: &BTreeMap<u16, u16>,
        origin: [f32; 3],
        site: crate::world_activation::Site,
    ) -> Result<Vec<Record>, Error> {
        let scene = self
            .objects
            .scene(site.scene_key)
            .ok_or(Error::UnknownObject)?;
        let Some(state::Object {
            current: Update::SubLevel { profile, fields },
            ..
        }) = self.objects.get(scene)
        else {
            return Err(Error::TypeMismatch);
        };
        if profile.kinds().get(site.serializer) != Some(&sublevel::Kind::RpcFourReferences) {
            return Err(Error::TypeMismatch);
        }
        let profile = profile.clone();
        let bound = match fields.get(site.serializer) {
            Some(Some(sublevel::Update::FourReferences(values))) => values[3].unwrap_or(0),
            _ => return Err(Error::TypeMismatch),
        };
        let owned = self.location_maps.get(&(connection, persona)).copied();
        if bound != 0 && Some(bound) != owned {
            return Err(Error::UnknownObject);
        }
        let mut next = self.clone();
        let mut records: Vec<_> = next
            .sync_location_map(connection, persona, cars, origin)?
            .into_iter()
            .collect();
        let target = if cars.is_empty() {
            0
        } else {
            *next
                .location_maps
                .get(&(connection, persona))
                .ok_or(Error::UnknownObject)?
        };
        if bound != target {
            let mut fields = vec![None; profile.kinds().len()];
            fields[site.serializer] = Some(sublevel::Update::FourReferences([
                None,
                None,
                None,
                Some(target),
            ]));
            records.extend(
                next.objects
                    .change(scene, Update::SubLevel { profile, fields })?,
            );
        }
        *self = next;
        Ok(records)
    }

    /// Publish this owner's active world cars. Empty input clears an existing
    /// map; no map is allocated until there is a world car. The caller selects
    /// world cars, while ownership and participant/car association are checked
    /// here before any mutation. Repeated quantized positions produce no output.
    pub fn sync_location_map(
        &mut self,
        connection: u8,
        persona: u64,
        cars: &BTreeMap<u16, u16>,
        origin: [f32; 3],
    ) -> Result<Option<Record>, Error> {
        if cars.len() > 128 {
            return Err(Error::Bound);
        }
        if connection == 0 || persona == 0 || origin.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        let mut entries = Vec::with_capacity(cars.len());
        for (&participant, &car) in cars {
            if !self.owns_participant(connection, persona, participant)
                || !self
                    .vehicles
                    .iter()
                    .any(|(&(p, _), &v)| p == participant && v == car)
            {
                return Err(Error::UnknownObject);
            }
            let position = self
                .vehicle_physics(connection, persona, car)?
                .position
                .to_position(origin, 7)?;
            entries.push(Entry {
                participant,
                x: Coordinate::from_world(position[0])?,
                z: Coordinate::from_world(position[2])?,
            });
        }
        let groups = if entries.is_empty() {
            Vec::new()
        } else {
            // Observed local-human group. Other group values remain codec data.
            vec![Group { value: 1, entries }]
        };
        let key = (connection, persona);
        if groups.is_empty() && !self.location_maps.contains_key(&key) {
            return Ok(None);
        }
        let update = Update::ParticipantLocationMap(location_map::Update {
            groups: Some(groups),
        });
        if let Some(&id) = self.location_maps.get(&key) {
            return self.objects.change(id, update);
        }
        if self.location_maps.len() >= 128 {
            return Err(Error::Bound);
        }
        let record = self
            .objects
            .spawn(Initial::ParticipantLocationMap, update)?;
        self.location_maps.insert(key, record.id);
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests;
