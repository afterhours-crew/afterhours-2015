// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Content, Request, layout::Layout, registrations};
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    content::Message,
    garage::{
        Slots,
        vehicle::{Binding, Loaded, Pending},
    },
    items::Collection,
    participants::Endpoint,
    replication::{
        Error, Initial, Record,
        entity::creation::{Asset, Catalog, Content as EntityContent, Parent, Prefix},
        players::Players,
        sublevel,
        vehicle::{self, authority},
    },
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
struct Spawned {
    slots: Slots,
    items: Arc<Collection>,
    origin: [u32; 3],
}
#[derive(Clone, Debug)]
pub struct Population {
    registrations: registrations::Registry,
    authority: authority::Registry,
    spawned: BTreeMap<u16, Spawned>,
    pending: Pending,
    next_group: u64,
}
/// Sub id of the world car creation (garage display cars use 1; E748).
pub const WORLD_SUB_ID: u32 = 2;
#[derive(Clone, Debug, Default)]
pub struct WorldSpawn {
    pub deleted: Vec<u16>,
    pub records: Vec<Record>,
    pub messages: Vec<Message>,
}
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub messages: Vec<Message>,
    pub records: Vec<Record>,
    pub bindings: Vec<Binding>,
}
pub struct Owner {
    pub connection: u8,
    pub persona: u64,
    pub participant: u16,
}
#[derive(Clone, Debug)]
pub struct Inventory {
    pub slots: Slots,
    pub items: Arc<Collection>,
}
impl Population {
    pub fn new(messages: &[Message]) -> Result<Self, Error> {
        Ok(Self {
            registrations: registrations::Registry::from_messages(messages)
                .map_err(|_| Error::Shape)?,
            authority: authority::Registry::default(),
            spawned: BTreeMap::new(),
            pending: Pending::default(),
            next_group: 1,
        })
    }
    pub fn spawn(
        &mut self,
        players: &mut Players,
        owner: Owner,
        inventory: Inventory,
        content: &Content,
        layout: &Layout,
        origin: [f32; 3],
    ) -> Result<Output, Error> {
        let identity = players
            .owned_identity(owner.connection, owner.persona, owner.participant)
            .ok_or(Error::UnknownObject)?;
        if players
            .owned_actor(owner.connection, owner.persona, owner.participant)
            .is_none()
        {
            return Err(Error::UnknownObject);
        }
        if origin.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        if let Some(old) = self.spawned.get(&owner.participant) {
            return if old.slots == inventory.slots
                && old.items == inventory.items
                && old.origin == origin.map(f32::to_bits)
            {
                Ok(Output::default())
            } else {
                Err(Error::Unsupported)
            };
        }
        if self.spawned.len() >= 128 {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        let mut world = players.clone();
        let mut output = Output::default();
        let occupied = layout.occupied(inventory.slots, &inventory.items)?;
        let blueprint = world
            .objects()
            .scene(layout.item_scene_key)
            .ok_or(Error::UnknownObject)?;
        let resource = vehicle::Resource {
            words: [
                identity.identity.persona as u32,
                (identity.identity.persona >> 32) as u32,
            ],
            name: identity.name,
        };
        for entry in occupied {
            let Some(Initial::SubLevel { fields, .. }) =
                world.objects().get(blueprint).map(|o| &o.initial)
            else {
                return Err(Error::TypeMismatch);
            };
            if !matches!(
                fields.get(entry.slot.item_component_index),
                Some(sublevel::Initial::Rpc(_))
            ) {
                return Err(Error::TypeMismatch);
            }
            let item = inventory
                .items
                .items
                .get(&entry.item)
                .ok_or(Error::UnknownObject)?;
            let definition = content
                .definition(&item.definition)
                .ok_or(Error::Unsupported)?;
            let prepared = next
                .registrations
                .prepare(&[&definition.bundle])
                .map_err(|_| Error::Unsupported)?;
            let registration = prepared.bindings[0];
            output.messages.extend(prepared.messages);
            let parent = if let Some(scene) = world.objects().scene(registration.content_key) {
                scene
            } else {
                let scene_content = sublevel::Content::new(vec![(
                    registration.content_key,
                    sublevel::Profile::new(false, &[sublevel::Kind::Noop])?,
                )])?;
                let creation = sublevel::passive::creation(
                    registration.level,
                    registration.content_key,
                    Serial::new(0).ok_or(Error::Shape)?,
                    &scene_content,
                )?;
                let scenes =
                    world.create_scenes(&[registration.level], vec![creation], &scene_content)?;
                let id = scenes[0].id;
                output.records.extend(scenes);
                id
            };
            let group = next.next_group;
            next.next_group = next.next_group.checked_add(1).ok_or(Error::Bound)?;
            let components = definition
                .profile
                .kinds()
                .iter()
                .enumerate()
                .filter(|(_, kind)| matches!(kind, vehicle::Kind::FourBit))
                .map(|(i, _)| {
                    group
                        .checked_mul(512)
                        .and_then(|g| g.checked_add(i as u64 + 1))
                        .ok_or(Error::Bound)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let body = content.construct(
                Request {
                    items: &inventory.items,
                    vehicle: entry.item,
                    locator: entry.slot.locator(),
                    basis: entry.slot.basis(),
                    origin,
                    speed: None,
                    connection: Some(owner.connection),
                    owner: resource.clone(),
                    customization_attached: true,
                    world: false,
                    authority_group: group,
                    authority_components: &components,
                },
                &mut next.authority,
                |name| next.registrations.mesh_level(name).ok(),
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
            let creation = vehicle::EntityCreation {
                prefix: Prefix {
                    parent: Some(Parent {
                        reference: Some(parent),
                    }),
                    blueprint,
                    sub_id: 1,
                    owner: None,
                    asset,
                },
                body,
            };
            let record = world.create_vehicle(
                owner.connection,
                owner.persona,
                owner.participant,
                entry.item,
                creation,
                &entity_content,
            )?;
            let vehicle = record.id;
            output.records.push(record);
            let (scene, _) = world
                .objects()
                .scene_endpoint(layout.scene_key, entry.slot.component_index)
                .ok_or(Error::UnknownObject)?;
            let Some(Initial::SubLevel { fields, .. }) =
                world.objects().get(scene).map(|o| &o.initial)
            else {
                return Err(Error::TypeMismatch);
            };
            let Some(sublevel::Initial::Rpc(rpc)) = fields.get(entry.slot.component_index) else {
                return Err(Error::TypeMismatch);
            };
            let binding = Binding {
                endpoint: Endpoint {
                    scene,
                    selector: rpc.selector,
                    serial: rpc.serial,
                },
                participant: owner.participant,
                vehicle,
            };
            if let Some(binding) = next.pending.offer(binding)? {
                output.bindings.push(binding)
            }
        }
        next.spawned.insert(
            owner.participant,
            Spawned {
                slots: inventory.slots,
                items: inventory.items,
                origin: origin.map(f32::to_bits),
            },
        );
        *self = next;
        *players = world;
        Ok(output)
    }
    /// Garage exit: remove the participant's primary garage car and re-create
    /// it as a world car at the layout's world spawn (official E748 shape:
    /// same bundle scene and gameplay blueprint, sub id 2, driveable chassis).
    pub fn spawn_world(
        &mut self,
        players: &mut Players,
        owner: Owner,
        inventory: &Inventory,
        content: &Content,
        layout: &Layout,
    ) -> Result<WorldSpawn, Error> {
        let spawn = layout.world_spawn().ok_or(Error::Unsupported)?;
        let identity = players
            .owned_identity(owner.connection, owner.persona, owner.participant)
            .ok_or(Error::UnknownObject)?;
        let entry = *layout
            .occupied(inventory.slots, &inventory.items)?
            .first()
            .ok_or(Error::UnknownObject)?;
        let mut next = self.clone();
        let mut world = players.clone();
        let removed = world.remove_vehicle(
            owner.connection,
            owner.persona,
            owner.participant,
            entry.item,
        )?;
        let item = inventory
            .items
            .items
            .get(&entry.item)
            .ok_or(Error::UnknownObject)?;
        let definition = content
            .definition(&item.definition)
            .ok_or(Error::Unsupported)?;
        let prepared = next
            .registrations
            .prepare(&[&definition.bundle])
            .map_err(|_| Error::Unsupported)?;
        let registration = prepared.bindings[0];
        let parent = world
            .objects()
            .scene(registration.content_key)
            .ok_or(Error::UnknownObject)?;
        let blueprint = world
            .objects()
            .scene(layout.item_scene_key)
            .ok_or(Error::UnknownObject)?;
        let resource = vehicle::Resource {
            words: [
                identity.identity.persona as u32,
                (identity.identity.persona >> 32) as u32,
            ],
            name: identity.name,
        };
        let group = next.next_group;
        next.next_group = next.next_group.checked_add(1).ok_or(Error::Bound)?;
        let components = definition
            .profile
            .kinds()
            .iter()
            .enumerate()
            .filter(|(_, kind)| matches!(kind, vehicle::Kind::FourBit))
            .map(|(i, _)| {
                group
                    .checked_mul(512)
                    .and_then(|g| g.checked_add(i as u64 + 1))
                    .ok_or(Error::Bound)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = content.construct(
            Request {
                items: &inventory.items,
                vehicle: entry.item,
                locator: spawn.locator(),
                basis: spawn.basis(),
                origin: [0.; 3],
                speed: Some(0.),
                connection: Some(owner.connection),
                owner: resource,
                customization_attached: false,
                world: true,
                authority_group: group,
                authority_components: &components,
            },
            &mut next.authority,
            |name| next.registrations.mesh_level(name).ok(),
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
        let creation = vehicle::EntityCreation {
            prefix: Prefix {
                parent: Some(Parent {
                    reference: Some(parent),
                }),
                blueprint,
                sub_id: WORLD_SUB_ID,
                owner: None,
                asset,
            },
            body,
        };
        let record = world.create_vehicle(
            owner.connection,
            owner.persona,
            owner.participant,
            entry.item,
            creation,
            &entity_content,
        )?;
        *self = next;
        *players = world;
        Ok(WorldSpawn {
            deleted: vec![removed],
            records: vec![record],
            messages: prepared.messages,
        })
    }
    pub fn acknowledge(
        &mut self,
        players: &Players,
        owner: Owner,
        loaded: Loaded,
    ) -> Result<bool, Error> {
        if loaded.participant != owner.participant {
            return Err(Error::UnknownObject);
        }
        self.pending.acknowledge(loaded, |id| {
            players.owns_participant(owner.connection, owner.persona, id)
        })
    }
    pub fn all_loaded(&self, participant: u16) -> bool {
        self.spawned.get(&participant).is_some_and(|s| {
            self.pending
                .all_loaded(participant, s.slots.values().iter().flatten().count())
        })
    }
    pub fn has_endpoint(&self, scene: u16, selector: u16) -> bool {
        self.pending.has_endpoint(scene, selector)
    }
    pub fn signal_owner(
        &self,
        players: &Players,
        connection: u8,
        persona: u64,
        vehicle: u16,
    ) -> Option<crate::glass::Owner> {
        if !matches!(
            players.objects().get(vehicle)?.initial,
            Initial::Vehicle { .. }
        ) {
            return None;
        }
        for (&participant, spawned) in &self.spawned {
            if spawned.slots.values().iter().flatten().any(|item| {
                players.owned_vehicle(connection, persona, participant, *item) == Some(vehicle)
            }) {
                return Some(crate::glass::Owner {
                    vehicle,
                    participant,
                    local_slot: players.local_slot(connection, persona, participant)?,
                    runtime_index: players.runtime_index(connection, persona, participant)?,
                });
            }
        }
        None
    }
}

#[cfg(test)]
pub(crate) mod tests;
