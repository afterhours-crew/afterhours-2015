// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Session-owned roaming rivals. The client runs their authored wandering AI;
//! the host owns activation, participant/actor identity and retirement.
use super::Content;
use crate::{
    traffic_content::population::RIVAL_KEY_START,
    vehicle_content::{self, registrations},
};
use nfs_world::{
    content::{Message, bindings::Bindings},
    participants::HostRpc,
    replication::{
        Error, Record,
        entity::creation::{Asset, Catalog, Content as EntityContent, Parent, Prefix},
        players::{Players, ai},
        vehicle,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug)]
pub struct Focus {
    pub id: u16,
    pub position: [f32; 3],
}
pub struct Input<'a> {
    pub now: u64,
    pub focus: &'a [Focus],
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Racer {
    key: Option<u64>,
    /// Retained between the native ten membership-query cohorts.
    region_active: bool,
    persona_cursor: usize,
}

#[derive(Clone, Debug)]
pub struct Population {
    content: Arc<Content>,
    racers: Vec<Racer>,
    authority: vehicle::authority::Registry,
    next: u64,
    cohort: usize,
    clock: Option<u64>,
    owner: Option<(u8, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub leave: Vec<HostRpc>,
    pub messages: Vec<Message>,
    pub records: Vec<Record>,
    pub calls: Vec<HostRpc>,
    pub deleted: Vec<u16>,
    pub spawned: Option<ai::Ids>,
}

impl Population {
    pub fn new(bindings: &Bindings, content: Arc<Content>) -> Result<Self, Error> {
        if bindings
            .root_child(content.data.managers.scene)
            .map_err(|_| Error::Shape)?
            .id
            >= 2048
        {
            return Err(Error::Bound);
        }
        Ok(Self {
            racers: vec![Racer::default(); content.data.racers.len()],
            content,
            authority: vehicle::authority::Registry::default(),
            next: RIVAL_KEY_START,
            cohort: 0,
            clock: None,
            owner: None,
        })
    }
    pub fn count(&self) -> usize {
        self.racers.iter().filter(|r| r.key.is_some()).count()
    }
    pub fn delegated(&self, world: &Players, connection: u8, persona: u64) -> BTreeMap<u16, u64> {
        if self.owner != Some((connection, persona)) {
            return BTreeMap::new();
        }
        self.racers
            .iter()
            .filter_map(|r| {
                let key = r.key?;
                world
                    .ai_ids(connection, persona, key)
                    .map(|ids| (ids.vehicle, key))
            })
            .collect()
    }

    /// Stage the world, mesh namespace and population together. The caller must
    /// also reserve all content/replication queues before committing this result.
    pub fn advance(
        &mut self,
        players: &mut Players,
        registrations: &mut registrations::Registry,
        connection: u8,
        persona: u64,
        input: Input<'_>,
    ) -> Result<Changes, Error> {
        let Input { now, focus } = input;
        if self.clock.is_some_and(|clock| now < clock) || focus.len() > 128 {
            return Err(Error::Bound);
        }
        let mut ids = BTreeSet::new();
        for point in focus {
            if point
                .position
                .iter()
                .any(|n| !n.is_finite() || n.abs() > 1_000_000.)
            {
                return Err(Error::Bound);
            }
            if !ids.insert(point.id) {
                return Err(Error::DuplicateObject);
            }
        }
        if self.owner.is_some_and(|old| old != (connection, persona)) {
            return Err(Error::UnknownObject);
        }
        if self.clock == Some(now) {
            return Ok(Changes::default());
        }
        let mut next = self.clone();
        let mut world = players.clone();
        let mut meshes = registrations.clone();
        let mut changes = Changes::default();
        for ordinal in 0..next.racers.len() {
            let retained = match next.racers[ordinal].key {
                Some(key) => {
                    let position = world
                        .ai_vehicle_physics(connection, persona, key)?
                        .decode([0.; 3])?
                        .position;
                    focus
                        .iter()
                        .any(|f| horizontal_squared(f.position, position) < 400. * 400.)
                }
                None => false,
            };
            let racer = &mut next.racers[ordinal];
            if focus.is_empty() {
                racer.region_active = false;
            } else if !retained && ordinal % 10 == next.cohort {
                racer.region_active = next.content.data.racers[ordinal]
                    .regions
                    .iter()
                    .any(|region| focus.iter().any(|f| region.contains(f.position)));
            }
            if !retained && !racer.region_active {
                if let Some(key) = racer.key.take() {
                    let removed = world.remove_ai_vehicle(connection, persona, key)?;
                    changes.records.extend(removed.records);
                    changes
                        .leave
                        .extend(removed.leave.into_iter().map(HostRpc::Participant));
                    changes.deleted.extend(removed.deleted);
                    next.authority.remove_group(key);
                }
            } else if racer.key.is_none()
                && changes.spawned.is_none()
                && world.ai_count() < ai::MAX_AI
            {
                // Bounded publication admits one new car per edge tick. Other
                // active racers remain eligible while queues drain.
                next.spawn(
                    &mut world,
                    &mut meshes,
                    connection,
                    persona,
                    ordinal,
                    &mut changes,
                )?;
            }
        }
        next.clock = Some(now);
        next.cohort = (next.cohort + 1) % 10;
        next.owner = Some((connection, persona));
        *self = next;
        *players = world;
        *registrations = meshes;
        Ok(changes)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        world: &mut Players,
        registrations: &mut registrations::Registry,
        connection: u8,
        persona: u64,
        ordinal: usize,
        changes: &mut Changes,
    ) -> Result<(), Error> {
        let racer = &self.content.data.racers[ordinal];
        let cursor = self.racers[ordinal].persona_cursor;
        let selected = racer.personas[cursor % racer.personas.len()];
        let provider = &self.content.data.personas[selected];
        let definition = self
            .content
            .vehicles
            .definition(&provider.preset.vehicle)
            .ok_or(Error::Shape)?;
        let prepared = registrations
            .prepare(&[&definition.bundle])
            .map_err(|_| Error::Shape)?;
        let registration = *prepared.bindings.first().ok_or(Error::Shape)?;
        changes.messages.extend(prepared.messages);
        let parent = if let Some(scene) = world.objects().scene(registration.content_key) {
            scene
        } else {
            use nfs_world::replication::sublevel;
            let scene_content = sublevel::Content::new(vec![(
                registration.content_key,
                sublevel::Profile::new(false, &[sublevel::Kind::Noop])?,
            )])?;
            let creation = sublevel::passive::creation(
                registration.level,
                registration.content_key,
                nfs_protocol::world::rpc::Serial::new(0).ok_or(Error::Shape)?,
                &scene_content,
            )?;
            let scenes =
                world.create_scenes(&[registration.level], vec![creation], &scene_content)?;
            let id = scenes.first().ok_or(Error::Shape)?.id;
            changes.records.extend(scenes);
            id
        };
        let blueprint = world
            .objects()
            .scene(self.content.data.managers.scene)
            .ok_or(Error::UnknownObject)?;
        let key = self.next;
        let components = definition
            .profile
            .kinds()
            .iter()
            .enumerate()
            .filter(|(_, kind)| matches!(kind, vehicle::Kind::FourBit))
            .map(|(i, _)| {
                key.checked_mul(512)
                    .and_then(|v| v.checked_add(i as u64 + 1))
                    .ok_or(Error::Bound)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = self.content.vehicles.construct_with_overrides(
            vehicle_content::Request {
                items: &self.content.presets[selected],
                vehicle: 1,
                locator: racer.position,
                basis: racer.basis,
                origin: [0.; 3],
                speed: Some(0.),
                connection: None,
                owner: vehicle::Resource {
                    words: [0; 2],
                    name: vec![],
                },
                customization_attached: true,
                placement: vehicle_content::Placement::Rival,
                authority_group: key,
                authority_components: &components,
            },
            provider.driving,
            &mut self.authority,
            |name| registrations.resolve_mesh(name, &mut changes.messages).ok(),
        )?;
        let asset = Asset {
            bundle: registration.asset_bundle,
            type_id: definition.asset_type,
            local_index: definition.asset_index,
        };
        let entity_content = EntityContent::new(
            vec![Catalog::new(asset.bundle, &definition.catalog)?],
            vec![],
        )?
        .with_vehicles(vec![(asset, definition.profile.clone())])?;
        let spawned = world
            .spawn_ai_vehicle(
                connection,
                persona,
                ai::Request {
                    key,
                    name: provider.name.as_bytes().to_vec(),
                    flags: [true, false],
                    words: [provider.provider_index, provider.personality_index],
                    // Roaming racers use the full vehicle driving mode.
                    // The ambient-traffic mode selects different physics.
                    mode: 2,
                },
                vehicle::EntityCreation {
                    prefix: Prefix {
                        parent: Some(Parent {
                            reference: Some(parent),
                        }),
                        blueprint,
                        sub_id: 2,
                        owner: None,
                        asset,
                    },
                    body,
                },
                &entity_content,
            )?
            .ok_or(Error::DuplicateObject)?;
        changes.records.extend(spawned.records);
        let bound = world.bind_ai_managers(connection, persona, key, self.content.data.managers)?;
        changes.calls.extend(bound.add.map(HostRpc::Participant));
        changes.calls.extend(bound.bind.map(HostRpc::Actor));
        changes.calls.extend(bound.enter.map(HostRpc::Participant));
        let (goal, assigned) =
            world.start_ai_wander_goal(connection, persona, key, [false, false, true])?;
        changes.records.extend([goal, assigned]);
        changes.spawned = Some(spawned.ids);
        self.racers[ordinal].key = Some(key);
        self.racers[ordinal].persona_cursor = (cursor + 1) % racer.personas.len();
        self.next = key.checked_add(1).ok_or(Error::Bound)?;
        Ok(())
    }
}

fn horizontal_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)
}

#[cfg(test)]
pub(crate) mod tests;
