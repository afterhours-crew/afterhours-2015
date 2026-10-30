// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::super::player_manager::{self, Entry};
use super::*;
impl Players {
    /// One manager belongs to this World. Its slots refer to the World's players.
    pub fn enable_player_manager(&mut self) -> Result<Option<Record>, Error> {
        if self.manager.is_some() {
            return Ok(None);
        }
        let mut update = player_manager::Update::empty_snapshot();
        for (id, object) in self.objects.iter() {
            if let Initial::Player(player) = &object.initial {
                let slot = update
                    .entries
                    .get_mut(usize::from(player.runtime_index))
                    .ok_or(Error::Bound)?;
                if slot.as_ref().is_some_and(Option::is_some) {
                    return Err(Error::DuplicateObject);
                }
                *slot = Some(Some(Entry::unbound(id)));
            }
        }
        let record = self
            .objects
            .spawn(Initial::PlayerManager, Update::PlayerManager(update))?;
        self.manager = Some(record.id);
        Ok(Some(record))
    }
    /// Publish newly allocated players without overwriting existing seat state.
    pub fn sync_player_manager(&mut self) -> Result<Option<Record>, Error> {
        let Some(id) = self.manager else {
            return Ok(None);
        };
        let Some(state::Object {
            current: Update::PlayerManager(current),
            ..
        }) = self.objects.get(id)
        else {
            return Err(Error::TypeMismatch);
        };
        let mut delta = player_manager::Update::default();
        for (player_id, object) in self.objects.iter() {
            if let Initial::Player(player) = &object.initial {
                let slot = usize::from(player.runtime_index);
                let existing = current.entries.get(slot).ok_or(Error::Bound)?;
                if let Some(Some(entry)) = existing {
                    if entry.player != player_id {
                        return Err(Error::DuplicateObject);
                    }
                } else {
                    delta.entries[slot] = Some(Some(Entry::unbound(player_id)));
                }
            }
        }
        self.objects.change(id, Update::PlayerManager(delta))
    }
    /// Re-publish only one owned player's current manager and participant links.
    /// A completion handoff can require these records even when their values
    /// have not changed. Read at publication time so a pending refresh never
    /// resurrects an earlier vehicle binding.
    pub fn vehicle_binding_snapshot(
        &self,
        connection: u8,
        persona: u64,
        participant: u16,
    ) -> Result<Vec<Record>, Error> {
        let slot = usize::from(
            self.runtime_index(connection, persona, participant)
                .ok_or(Error::UnknownObject)?,
        );
        let manager = self.manager.ok_or(Error::Unsupported)?;
        let Some(state::Object {
            current: Update::PlayerManager(current),
            ..
        }) = self.objects.get(manager)
        else {
            return Err(Error::TypeMismatch);
        };
        let entry = current
            .entries
            .get(slot)
            .and_then(Option::as_ref)
            .and_then(Option::as_ref)
            .ok_or(Error::UnknownObject)?;
        if !self.owns(connection, persona, entry.player) {
            return Err(Error::UnknownObject);
        }
        let Some(state::Object {
            current: Update::Participant(current),
            ..
        }) = self.objects.get(participant)
        else {
            return Err(Error::TypeMismatch);
        };
        let mut delta = player_manager::Update::default();
        delta.entries[slot] = Some(Some(entry.clone()));
        Ok(vec![
            Record {
                id: manager,
                initial: None,
                update: Update::PlayerManager(delta),
            },
            Record {
                id: participant,
                initial: None,
                update: Update::Participant(ParticipantUpdate {
                    entity: Some(current.entity.unwrap_or(0)),
                    ..ParticipantUpdate::default()
                }),
            },
        ])
    }
    /// Bind the manager seats, participant entity and player view to its own vehicle.
    /// The participant reference is used by teleport and scripted vehicle control.
    /// Clear all three relations before removing a bound vehicle.
    /// Ownership and every index are checked before any state changes.
    pub fn bind_player_vehicle(
        &mut self,
        connection: u8,
        persona: u64,
        participant: u16,
        vehicle: Option<u16>,
    ) -> Result<Vec<Record>, Error> {
        if !self.owns_participant(connection, persona, participant) {
            return Err(Error::UnknownObject);
        }
        if let Some(car) = vehicle {
            if !self.participant_has_vehicle(participant, car) {
                return Err(Error::UnknownObject);
            }
            if !matches!(
                self.objects.get(car).map(|o| &o.initial),
                Some(Initial::Vehicle { .. })
            ) {
                return Err(Error::TypeMismatch);
            }
        }
        let player = *self
            .participants
            .iter()
            .find(|(_, p)| **p == participant)
            .ok_or(Error::UnknownObject)?
            .0;
        let slot = usize::from(
            self.runtime_index(connection, persona, participant)
                .ok_or(Error::UnknownObject)?,
        );
        let id = self.manager.ok_or(Error::Unsupported)?;
        let mut delta = player_manager::Update::default();
        *delta.entries.get_mut(slot).ok_or(Error::Bound)? = Some(Some(match vehicle {
            Some(car) => Entry::driving(player, car),
            None => Entry::unbound(player),
        }));
        let mut next = self.objects.clone();
        let mut records = Vec::with_capacity(3);
        records.extend(next.change(id, Update::PlayerManager(delta))?);
        records.extend(next.change(
            participant,
            Update::Participant(ParticipantUpdate {
                entity: Some(vehicle.unwrap_or(0)),
                ..ParticipantUpdate::default()
            }),
        )?);
        if let Some(&view) = self.views.get(&player) {
            records.extend(next.change(view, super::views::seat_update(vehicle))?);
        }
        self.objects = next;
        Ok(records)
    }
}
