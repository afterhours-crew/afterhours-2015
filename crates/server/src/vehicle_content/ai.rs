// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared vehicle attachment for host-owned AI participants. The caller includes
//! Players, content registrations and authority in its frame transaction.
use super::{Content, Placement, Request, registrations};
use nfs_world::{
    content::Message,
    items::Collection,
    replication::{
        Error, Record,
        entity::creation::{Asset, Catalog, Content as EntityContent, Parent, Prefix},
        players::{Players, ai},
        sublevel, vehicle,
    },
};

pub(crate) struct Spawn<'a> {
    pub connection: u8,
    pub persona: u64,
    pub slot: u64,
    pub key: u64,
    pub manager_scene: u32,
    pub provider: &'a nfs_content::rivals::Persona,
    pub preset: &'a Collection,
    pub locator: [f32; 3],
    pub basis: [[f32; 3]; 3],
    pub speed: f32,
}

pub(crate) struct Attached {
    pub messages: Vec<Message>,
    pub records: Vec<Record>,
    pub tracked: nfs_world::world_activation::Notification,
    pub ids: ai::Ids,
}

pub(crate) fn attach(
    content: &Content,
    world: &mut Players,
    registrations: &mut registrations::Registry,
    authority: &mut vehicle::authority::Registry,
    request: Spawn<'_>,
) -> Result<Attached, Error> {
    let definition = content
        .definition(&request.provider.preset.vehicle)
        .ok_or(Error::Shape)?;
    let prepared = registrations
        .prepare(&[&definition.bundle])
        .map_err(|_| Error::Shape)?;
    let registration = *prepared.bindings.first().ok_or(Error::Shape)?;
    let mut messages = prepared.messages;
    let mut records = Vec::new();
    let parent = if let Some(scene) = world.objects().scene(registration.content_key) {
        scene
    } else {
        let scene_content = sublevel::Content::new(vec![(
            registration.content_key,
            sublevel::Profile::new(false, &[sublevel::Kind::Noop])?,
        )])?;
        let root = world
            .objects()
            .scene(sublevel::root::CONTENT_KEY)
            .map(|ghost| sublevel::Blueprint { ghost, sub_id: 1 });
        let creation = sublevel::passive::creation_with_parent(
            registration.level,
            registration.content_key,
            nfs_protocol::world::rpc::Serial::new(0).ok_or(Error::Shape)?,
            root,
            &scene_content,
        )?;
        let scenes = world.create_scenes(&[registration.level], vec![creation], &scene_content)?;
        let id = scenes.first().ok_or(Error::Shape)?.id;
        records.extend(scenes);
        id
    };
    let blueprint = world
        .objects()
        .scene(request.manager_scene)
        .ok_or(Error::UnknownObject)?;
    let components = definition
        .profile
        .kinds()
        .iter()
        .enumerate()
        .filter(|(_, kind)| matches!(kind, vehicle::Kind::FourBit))
        .map(|(i, _)| {
            request
                .key
                .checked_mul(512)
                .and_then(|v| v.checked_add(i as u64 + 1))
                .ok_or(Error::Bound)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let body = content.construct_with_overrides(
        Request {
            items: request.preset,
            vehicle: 1,
            locator: request.locator,
            basis: request.basis,
            origin: [0.; 3],
            speed: Some(request.speed),
            connection: None,
            owner: vehicle::Resource {
                words: [0; 2],
                name: vec![],
            },
            // A displayed customization car does not participate in world physics.
            customization_attached: false,
            placement: Placement::Rival,
            authority_group: request.key,
            authority_components: &components,
        },
        request.provider.driving,
        authority,
        |name| registrations.resolve_mesh(name, &mut messages).ok(),
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
    let attached = world.attach_ai_slot_vehicle(
        request.connection,
        request.persona,
        request.slot,
        request.key,
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
    )?;
    records.extend(attached.records);
    Ok(Attached {
        messages,
        records,
        tracked: attached.tracked,
        ids: attached.ids,
    })
}
