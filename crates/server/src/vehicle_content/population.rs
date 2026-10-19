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
    groups: BTreeMap<u16, u64>,
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
    pub deleted: Vec<u16>,
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
    pub fn registrations(&self) -> &registrations::Registry {
        &self.registrations
    }
    pub fn update_registrations(
        &mut self,
        previous: &registrations::Registry,
        next: registrations::Registry,
    ) -> Result<(), Error> {
        if &self.registrations != previous {
            return Err(Error::Shape);
        }
        self.registrations = next;
        Ok(())
    }
    /// Refresh the owned inventory used for later garage/world lifetimes after
    /// a validated purchase or equipment commit. No spawn or authority changes.
    pub fn refresh_inventory(
        &mut self,
        players: &Players,
        connection: u8,
        persona: u64,
        previous: &Inventory,
        next: &Inventory,
    ) -> Result<(), Error> {
        if previous.slots != next.slots {
            return Err(Error::Shape);
        }
        for (&participant, spawned) in &self.spawned {
            if players.owns_participant(connection, persona, participant)
                && (spawned.slots != previous.slots || spawned.items != previous.items)
            {
                return Err(Error::Shape);
            }
        }
        for (&participant, spawned) in &mut self.spawned {
            if players.owns_participant(connection, persona, participant) {
                spawned.items = next.items.clone();
            }
        }
        Ok(())
    }

    pub fn new(messages: &[Message]) -> Result<Self, Error> {
        Ok(Self {
            registrations: registrations::Registry::from_messages(messages)
                .map_err(|_| Error::Shape)?,
            authority: authority::Registry::default(),
            spawned: BTreeMap::new(),
            pending: Pending::default(),
            next_group: 1,
            groups: BTreeMap::new(),
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
        self.spawn_mode(
            players, owner, inventory, content, layout, origin, false, None,
        )
    }
    /// Prepare the selected car in its garage slot while its world lifetime runs.
    pub fn prepare_return(
        &mut self,
        players: &mut Players,
        owner: Owner,
        inventory: Inventory,
        content: &Content,
        layout: &Layout,
    ) -> Result<Output, Error> {
        let origin = self
            .spawned
            .get(&owner.participant)
            .ok_or(Error::UnknownObject)?
            .origin
            .map(f32::from_bits);
        self.spawn_mode(
            players, owner, inventory, content, layout, origin, true, None,
        )
    }
    /// Replace only changed slots after an owned, durable garage transaction.
    /// Preflight the entire replacement on clones before publishing any ghosts.
    pub fn reconcile(
        &mut self,
        players: &mut Players,
        owner: Owner,
        previous: &Inventory,
        inventory: Inventory,
        content: &Content,
        layout: &Layout,
    ) -> Result<Output, Error> {
        let old = self
            .spawned
            .get(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if old.slots != previous.slots
            || old.items != previous.items
            || !self.all_loaded(owner.participant)
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::Shape);
        }
        let origin = old.origin.map(f32::from_bits);
        let changed =
            std::array::from_fn(|i| previous.slots.values()[i] != inventory.slots.values()[i]);
        let mut next = self.clone();
        let mut world = players.clone();
        let mut deleted = Vec::new();
        for entry in layout.occupied(previous.slots, &previous.items)? {
            if !changed[entry.ordinal] {
                continue;
            }
            let (scene, _) = world
                .objects()
                .scene_endpoint(layout.scene_key, entry.slot.component_index)
                .ok_or(Error::UnknownObject)?;
            let Some(Initial::SubLevel { fields, .. }) =
                world.objects().get(scene).map(|o| &o.initial)
            else {
                return Err(Error::TypeMismatch);
            };
            let rpc = fields
                .get(entry.slot.component_index)
                .and_then(sublevel::Initial::rpc)
                .ok_or(Error::TypeMismatch)?;
            next.pending
                .remove_slot(scene, rpc.selector, owner.participant)?;
            let ghost = world.remove_vehicle(
                owner.connection,
                owner.persona,
                owner.participant,
                entry.item,
            )?;
            if let Some(group) = next.groups.remove(&ghost) {
                next.authority.remove_group(group);
            }
            deleted.push(ghost);
        }
        let mut output = next.spawn_mode(
            &mut world,
            owner,
            inventory,
            content,
            layout,
            origin,
            false,
            Some(changed),
        )?;
        output.deleted = deleted;
        *self = next;
        *players = world;
        Ok(output)
    }
    #[allow(clippy::too_many_arguments)]
    fn spawn_mode(
        &mut self,
        players: &mut Players,
        owner: Owner,
        inventory: Inventory,
        content: &Content,
        layout: &Layout,
        origin: [f32; 3],
        returning: bool,
        changed: Option<[bool; 5]>,
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
        if let Some(old) = self
            .spawned
            .get(&owner.participant)
            .filter(|_| changed.is_none())
        {
            if old.slots == inventory.slots
                && old.items == inventory.items
                && old.origin == origin.map(f32::to_bits)
            {
                if !returning {
                    return Ok(Output::default());
                }
            } else {
                return Err(Error::Unsupported);
            }
        } else if returning && changed.is_none() {
            return Err(Error::UnknownObject);
        }
        if !self.spawned.contains_key(&owner.participant) && self.spawned.len() >= 128 {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        let mut world = players.clone();
        let mut output = Output::default();
        let mut occupied = layout.occupied(inventory.slots, &inventory.items)?;
        if let Some(changed) = changed {
            occupied.retain(|entry| changed[entry.ordinal]);
        }
        if returning {
            occupied.truncate(1);
            let entry = occupied.first().ok_or(Error::UnknownObject)?;
            if players
                .staged_vehicle(
                    owner.connection,
                    owner.persona,
                    owner.participant,
                    entry.item,
                )
                .is_some()
            {
                return Ok(Output::default());
            }
        }
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
                // Like the official host, the bundle scene names the root scene.
                let root = world
                    .objects()
                    .scene(sublevel::root::CONTENT_KEY)
                    .map(|ghost| sublevel::Blueprint { ghost, sub_id: 1 });
                let creation = sublevel::passive::creation_with_parent(
                    registration.level,
                    registration.content_key,
                    Serial::new(0).ok_or(Error::Shape)?,
                    root,
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
                    placement: super::Placement::Garage,
                    authority_group: group,
                    authority_components: &components,
                },
                &mut next.authority,
                |name| {
                    next.registrations
                        .resolve_mesh(name, &mut output.messages)
                        .ok()
                },
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
            let create = if returning {
                Players::stage_vehicle
            } else {
                Players::create_vehicle
            };
            let record = create(
                &mut world,
                owner.connection,
                owner.persona,
                owner.participant,
                entry.item,
                creation,
                &entity_content,
            )?;
            let vehicle = record.id;
            next.groups.insert(vehicle, group);
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
            let offered = if returning {
                // The slot still names the pre-exit garage car; replace that
                // exact slot, not a foreign participant's pending load.
                next.pending.replace_slot(binding)?
            } else {
                next.pending.offer(binding)?
            };
            if let Some(binding) = offered {
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
    pub fn finish_return(
        &mut self,
        players: &mut Players,
        owner: Owner,
        inventory: &Inventory,
        layout: &Layout,
    ) -> Result<u16, Error> {
        let entry = *layout
            .occupied(inventory.slots, &inventory.items)?
            .first()
            .ok_or(Error::UnknownObject)?;
        let removed = players.promote_staged_vehicle(
            owner.connection,
            owner.persona,
            owner.participant,
            entry.item,
        )?;
        if let Some(group) = self.groups.remove(&removed) {
            self.authority.remove_group(group);
        }
        Ok(removed)
    }
    /// Garage exit: remove the participant's primary garage car and re-create
    /// it as a world car at the configured spawn, or at its installed garage
    /// resting pose when an exit destination is available (official E748 shape:
    /// same bundle scene and gameplay blueprint, sub id 2, driveable chassis).
    pub fn spawn_world(
        &mut self,
        players: &mut Players,
        owner: Owner,
        inventory: &Inventory,
        content: &Content,
        layout: &Layout,
    ) -> Result<WorldSpawn, Error> {
        if layout.world_spawn().is_none() && layout.exit_teleport().is_none() {
            return Err(Error::Unsupported);
        }
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
        if let Some(group) = next.groups.remove(&removed) {
            next.authority.remove_group(group);
        }
        let item = inventory
            .items
            .items
            .get(&entry.item)
            .ok_or(Error::UnknownObject)?;
        let definition = content
            .definition(&item.definition)
            .ok_or(Error::Unsupported)?;
        // Installation-derived layouts supply the exit teleport, not a sampled
        // pre-teleport root pose. Preserve the owned garage car's resting pose
        // for the swap; the normal readiness handshake moves it to the exit.
        let (position, basis) = match layout.world_spawn() {
            Some(spawn) => (spawn.locator(), spawn.basis()),
            None => (
                definition
                    .resting
                    .position_at(entry.slot.locator())?
                    .map(|v| v.map_or(0., f32::from_bits)),
                entry.slot.basis(),
            ),
        };
        let mut prepared = next
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
                locator: position,
                basis,
                origin: [0.; 3],
                speed: Some(0.),
                connection: Some(owner.connection),
                owner: resource,
                customization_attached: false,
                placement: super::Placement::PlayerWorld,
                authority_group: group,
                authority_components: &components,
            },
            &mut next.authority,
            |name| {
                next.registrations
                    .resolve_mesh(name, &mut prepared.messages)
                    .ok()
            },
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
        next.groups.insert(record.id, group);
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
                    || players.staged_vehicle(connection, persona, participant, *item)
                        == Some(vehicle)
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
