// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Prepared event opponents and transactional grid creation. The event lifecycle
//! supplies persona selection and spawn keys, then owns membership and goals.
use crate::vehicle_content::{self, registrations};
use nfs_content::events::ai::{Completion, Roster};
use nfs_world::{
    content::Message,
    items::Collection,
    participants::HostRpc,
    replication::{
        Error, Record,
        players::{Players, ai},
        vehicle,
    },
};
use std::{collections::BTreeSet, sync::Arc};

mod commands;
pub use commands::Commands;
pub mod navigation;

/// Separate from ambient-traffic and roaming-racer keys, including slot keys.
pub(crate) const KEY_START: u64 = 1 << 48;

/// Portable ordering for the native provider's shared cyclic cursor. Content
/// ordinals replace native pointer order; captured selections are not replayed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cursor {
    next: u64,
    persona: usize,
}

impl Cursor {
    pub fn select(&mut self, content: &Content) -> Result<Vec<Selection>, Error> {
        let mut next = self.clone();
        let mut chosen = BTreeSet::new();
        let mut selections = Vec::new();
        for spawn in &content.roster.spawns {
            let selected = (0..spawn.personas.len())
                .map(|offset| (next.persona + offset) % spawn.personas.len())
                .find(|&index| !chosen.contains(&spawn.personas[index]))
                .ok_or(Error::Bound)?;
            let persona = spawn.personas[selected];
            chosen.insert(persona);
            next.persona = (selected + 1) % spawn.personas.len();
            let key = KEY_START.checked_add(next.next).ok_or(Error::Bound)?;
            if key > u64::MAX / 512 - 1 {
                return Err(Error::Bound);
            }
            next.next += 1;
            selections.push(Selection {
                persona,
                slot_key: key,
                car_key: key,
            });
        }
        *self = next;
        Ok(selections)
    }
}

#[derive(Clone, Debug)]
pub struct Content {
    roster: Roster,
    vehicles: vehicle_content::Content,
    presets: Vec<Arc<Collection>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub persona: usize,
    pub slot_key: u64,
    pub car_key: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Member {
    pub selection: Selection,
    pub ids: ai::Ids,
}

#[derive(Clone, Debug)]
pub struct Racers {
    pub members: Vec<Member>,
    pub authority: vehicle::authority::Registry,
}

pub struct Created {
    pub racers: Racers,
    pub messages: Vec<Message>,
    pub records: Vec<Record>,
    pub calls: Vec<HostRpc>,
}

impl Content {
    pub fn new(
        roster: Roster,
        vehicles: vehicle_content::Content,
        items: &nfs_services::item_content::ItemContent,
    ) -> Result<Self, Error> {
        let catalog = items.inventory().ok_or(Error::Unsupported)?;
        let offers = items.purchase_offers().ok_or(Error::Unsupported)?;
        let presets = roster
            .personas
            .iter()
            .map(|p| {
                vehicles
                    .definition(&p.preset.vehicle)
                    .ok_or(Error::Unsupported)?;
                vehicles
                    .rival_preset(&p.preset, catalog, offers)
                    .map(Arc::new)
                    .map_err(|_| Error::Shape)
            })
            .collect::<Result<_, _>>()?;
        Self::prepared(roster, vehicles, presets)
    }

    pub(crate) fn prepared(
        roster: Roster,
        vehicles: vehicle_content::Content,
        presets: Vec<Arc<Collection>>,
    ) -> Result<Self, Error> {
        if roster.managers.is_none()
            || roster.completion != Some(Completion::GiveIndividualToDirectorAtCompletion)
            || roster.spawns.is_empty()
            || roster.spawns.len() > nfs_world::race_pacing::MAX_OPPONENTS
            || roster.personas.is_empty()
            || roster.personas.len() > nfs_content::events::ai::MAX_PERSONAS
            || roster.personas.len() != presets.len()
            || roster.pacing_skills.len() != presets.len()
            || roster
                .pacing_skills
                .iter()
                .any(|v| v.is_none_or(|v| !v.is_finite()))
        {
            return Err(Error::Shape);
        }
        for spawn in &roster.spawns {
            if spawn.once
                || spawn.store_participant
                || spawn.ignore_availability
                || spawn.personas.is_empty()
                || spawn.personas.iter().any(|&i| i >= presets.len())
                || spawn.personas.iter().collect::<BTreeSet<_>>().len() != spawn.personas.len()
                || spawn.position.iter().any(|v| !v.is_finite())
            {
                return Err(Error::Shape);
            }
            vehicle::orientation::quaternion_from_basis(spawn.basis)?;
        }
        Ok(Self {
            roster,
            vehicles,
            presets,
        })
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    pub(crate) fn car_id(&self, persona: usize) -> Result<u32, Error> {
        let provider = self.roster.personas.get(persona).ok_or(Error::Bound)?;
        let definition = self
            .vehicles
            .definition(&provider.preset.vehicle)
            .ok_or(Error::Shape)?;
        Ok(crate::event_logic::car_id(&definition.blueprint))
    }

    /// All-or-nothing allocation. Selection is explicit and must select one
    /// distinct eligible persona per grid slot. A failed later car leaves world
    /// IDs, manager lists, registrations and the caller's keys untouched.
    pub fn create(
        &self,
        players: &mut Players,
        registry: &mut registrations::Registry,
        connection: u8,
        persona: u64,
        selections: &[Selection],
    ) -> Result<Created, Error> {
        if selections.len() != self.roster.spawns.len()
            || selections
                .iter()
                .map(|s| s.persona)
                .collect::<BTreeSet<_>>()
                .len()
                != selections.len()
            || selections
                .iter()
                .map(|s| s.slot_key)
                .collect::<BTreeSet<_>>()
                .len()
                != selections.len()
            || selections
                .iter()
                .map(|s| s.car_key)
                .collect::<BTreeSet<_>>()
                .len()
                != selections.len()
        {
            return Err(Error::Shape);
        }
        for (selection, spawn) in selections.iter().zip(&self.roster.spawns) {
            if !spawn.personas.contains(&selection.persona)
                || selection.slot_key == 0
                || selection.car_key == 0
                || selection.car_key > u64::MAX / 512 - 1
            {
                return Err(Error::Shape);
            }
        }
        let managers = self.roster.managers.ok_or(Error::Shape)?;
        let mut world = players.clone();
        let mut registrations = registry.clone();
        let mut authority = vehicle::authority::Registry::default();
        let mut out = Created {
            racers: Racers {
                members: Vec::new(),
                authority: authority.clone(),
            },
            messages: Vec::new(),
            records: Vec::new(),
            calls: Vec::new(),
        };
        for (selection, spawn) in selections.iter().zip(&self.roster.spawns) {
            let provider = &self.roster.personas[selection.persona];
            let slot = world
                .create_ai_slot(
                    connection,
                    persona,
                    ai::Request {
                        key: selection.slot_key,
                        name: provider.name.as_bytes().to_vec(),
                        flags: [false, true],
                        words: [provider.provider_index, provider.personality_index],
                        mode: 2,
                    },
                    managers,
                )?
                .ok_or(Error::DuplicateObject)?;
            out.records.extend(slot.records);
            out.calls.extend(slot.bound.add.map(HostRpc::Participant));
            out.calls.extend(slot.bound.bind.map(HostRpc::Actor));
            out.calls.extend(slot.bound.enter.map(HostRpc::Participant));
            let attached = vehicle_content::ai::attach(
                &self.vehicles,
                &mut world,
                &mut registrations,
                &mut authority,
                vehicle_content::ai::Spawn {
                    connection,
                    persona,
                    slot: selection.slot_key,
                    key: selection.car_key,
                    manager_scene: managers.scene,
                    provider,
                    preset: &self.presets[selection.persona],
                    locator: spawn.position,
                    basis: spawn.basis,
                    speed: 0.,
                },
            )?;
            out.records.extend(attached.records);
            out.messages.extend(attached.messages);
            out.calls.push(HostRpc::WorldActivation(attached.tracked));
            out.racers.members.push(Member {
                selection: *selection,
                ids: attached.ids,
            });
        }
        out.racers.authority = authority;
        *players = world;
        *registry = registrations;
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests;
