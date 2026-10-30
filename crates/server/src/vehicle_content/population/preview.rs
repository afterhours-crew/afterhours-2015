// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewCars {
    pub garage: nfs_content::garage::starter::Garage,
    /// Each collection is a separate ephemeral car rooted at item 1.
    pub items: [Arc<Collection>; 3],
}
#[derive(Clone, Debug)]
pub(super) struct Spawned {
    source: Arc<PreviewCars>,
    pub bindings: Vec<Binding>,
}
impl Population {
    /// Publish all authored choices atomically. Display ownership never enters
    /// the inventory or driving-car maps, even when local item IDs coincide.
    pub fn spawn_previews(
        &mut self,
        players: &mut Players,
        owner: Owner,
        source: Arc<PreviewCars>,
        content: &Content,
    ) -> Result<Output, Error> {
        if players
            .owned_actor(owner.connection, owner.persona, owner.participant)
            .is_none()
        {
            return Err(Error::UnknownObject);
        }
        if let Some(previous) = self.previews.get(&owner.participant) {
            return if previous.source == source {
                Ok(Output::default())
            } else {
                Err(Error::Shape)
            };
        }
        if self.previews.len() >= 128 {
            return Err(Error::Bound);
        }
        let mut slots = std::collections::BTreeSet::new();
        for (slot, items) in source.garage.slots.iter().zip(&source.items) {
            if slot.component > 511
                || !slots.insert(slot.component)
                || items.items.len() > MAX_TEMPORARY_ITEMS
                || items.roots.len() > MAX_TEMPORARY_ITEMS
            {
                return Err(Error::Bound);
            }
            if items
                .items
                .get(&1)
                .is_none_or(|i| i.owner != 0 || i.definition != slot.definition)
            {
                return Err(Error::Shape);
            }
        }
        let mut next = self.clone();
        let mut world = players.clone();
        let mut out = Output::default();
        for (slot, items) in source.garage.slots.iter().zip(&source.items) {
            let (scene, selector) = world
                .objects()
                .scene_endpoint(source.garage.scene_key, slot.component)
                .ok_or(Error::UnknownObject)?;
            let Some(Initial::SubLevel { fields, .. }) =
                world.objects().get(scene).map(|o| &o.initial)
            else {
                return Err(Error::TypeMismatch);
            };
            let rpc = fields
                .get(slot.component)
                .and_then(sublevel::Initial::rpc)
                .ok_or(Error::TypeMismatch)?;
            let serial = rpc.serial;
            if fields
                .get(slot.item_component)
                .and_then(sublevel::Initial::rpc)
                .is_none()
            {
                return Err(Error::TypeMismatch);
            }
            let batch = next.world_vehicle(
                &mut world,
                owner,
                VehicleSource {
                    items,
                    vehicle: 1,
                    position: slot.locator(),
                    basis: slot.basis(),
                    lifetime: Lifetime::Preview(slot.component as u16),
                },
                content,
                source.garage.creation_scene_key,
            )?;
            let vehicle = world
                .preview_vehicle(
                    owner.connection,
                    owner.persona,
                    owner.participant,
                    slot.component as u16,
                )
                .ok_or(Error::UnknownObject)?;
            out.records.extend(batch.records);
            out.messages.extend(batch.messages);
            let binding = Binding {
                endpoint: Endpoint {
                    scene,
                    selector,
                    serial,
                },
                participant: owner.participant,
                vehicle,
            };
            if let Some(binding) = next.preview_pending.offer(binding)? {
                out.bindings.push(binding);
            }
        }
        next.previews.insert(
            owner.participant,
            Spawned {
                source,
                bindings: out.bindings.clone(),
            },
        );
        *self = next;
        *players = world;
        Ok(out)
    }
    pub fn previews_loaded(&self, participant: u16) -> bool {
        self.previews.get(&participant).is_some_and(|p| {
            self.preview_pending
                .all_loaded(participant, p.bindings.len())
        })
    }
    pub fn remove_previews(
        &mut self,
        players: &mut Players,
        owner: Owner,
    ) -> Result<Vec<u16>, Error> {
        let source = self
            .previews
            .get(&owner.participant)
            .ok_or(Error::UnknownObject)?
            .clone();
        let mut next = self.clone();
        let mut world = players.clone();
        let mut deleted = vec![];
        for (slot, binding) in source.source.garage.slots.iter().zip(source.bindings) {
            next.preview_pending.remove_slot(
                binding.endpoint.scene,
                binding.endpoint.selector,
                owner.participant,
            )?;
            let id = world.remove_preview_vehicle(
                owner.connection,
                owner.persona,
                owner.participant,
                slot.component as u16,
            )?;
            if id != binding.vehicle {
                return Err(Error::Shape);
            }
            if let Some(group) = next.groups.remove(&id) {
                next.authority.remove_group(group);
            }
            deleted.push(id);
        }
        next.previews.remove(&owner.participant);
        *self = next;
        *players = world;
        Ok(deleted)
    }
}
