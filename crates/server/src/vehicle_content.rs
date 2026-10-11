// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{Failure, content::BUILD};
use nfs_world::{
    items::{Catalog, Collection, Guid},
    replication::{Error, vehicle::*},
};
use serde_json::Value;
use std::{collections::BTreeMap, fs::File, io::Read, path::Path, sync::Arc};

pub mod layout;
mod parse;
mod parts;
pub mod population;
pub mod registrations;

const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Chassis vehicle mode of the official world car (E748).
pub const WORLD_MODE: u8 = 3;
/// Mode timer that encodes as the official saturated value (63 of 6 bits).
pub const WORLD_MODE_TIME: f32 = 3.0;
#[derive(Clone, Debug)]
pub struct GarageContent {
    pub vehicles: Content,
    pub layout: layout::Layout,
}
#[derive(Clone, Debug)]
pub struct Definition {
    pub blueprint: String,
    pub bundle: String,
    pub asset_type: u16,
    pub asset_index: u16,
    pub catalog: Vec<(u16, u16)>,
    pub profile: Profile,
    root: root::Blueprint,
    resting: placement::RestingConfig,
    team: u8,
    mode: u8,
    parts: BTreeMap<usize, bool>,
    wheel_baseline: [f32; 2],
    appearance: appearance::Defaults,
    meshes: Vec<mesh::Component>,
    health: health::State,
}

#[derive(Clone, Debug)]
pub struct Content {
    definitions: BTreeMap<Guid, Arc<Definition>>,
    custom: customization::Definitions,
    nos: nos::Definitions,
    wheels: wheel_customization::Definitions,
    appearance: appearance::Definitions,
    meshes: mesh::Definitions,
}
pub struct Request<'a> {
    pub items: &'a Collection,
    pub vehicle: u64,
    pub locator: [f32; 3],
    pub basis: [[f32; 3]; 3],
    pub origin: [f32; 3],
    pub speed: Option<f32>,
    pub connection: Option<u8>,
    pub owner: Resource,
    pub customization_attached: bool,
    /// World car: the locator is the exact root position (no resting offset)
    /// and the chassis is driveable (mode 3, settled mode timer), as the
    /// official world car (E748).
    pub world: bool,
    pub authority_group: u64,
    pub authority_components: &'a [u64],
}

impl Content {
    pub fn load(path: &Path, classes: &Catalog) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(
            &serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?,
            classes,
        )
    }
    pub fn from_json(value: &Value, classes: &Catalog) -> Result<Self, Failure> {
        use parse::*;
        keys(
            value,
            &["format", "version", "build_sha256", "parts", "vehicles"],
            &[],
        )?;
        if value["format"] != "nfs-vehicle-content"
            || value["version"] != 1
            || value["build_sha256"] != BUILD
        {
            return Err(Failure::ProfileConfig);
        }
        let p = &value["parts"];
        keys(
            p,
            &["customization", "nos", "wheels", "appearance", "mesh"],
            &[],
        )?;
        let (custom, nos, wheels, appearance, meshes) = parts::parse(p, classes)?;
        let mut definitions = BTreeMap::new();
        let mut names = std::collections::BTreeSet::new();
        for row in array(&value["vehicles"], 512)? {
            let definition = Arc::new(parse::definition(row)?);
            if !names.insert(definition.blueprint.clone()) {
                return Err(Failure::ProfileConfig);
            }
            let aliases = array(&row["definitions"], 512)?;
            if aliases.is_empty() {
                return Err(Failure::ProfileConfig);
            }
            for alias in aliases {
                let id = guid(alias)?;
                if classes.class(&id).map_err(|_| Failure::ProfileConfig)?
                    != nfs_world::items::DefinitionClass::RaceVehicleItemData
                    || definitions.len() >= nfs_world::items::MAX_DEFINITIONS
                    || definitions.insert(id, definition.clone()).is_some()
                {
                    return Err(Failure::ProfileConfig);
                }
            }
        }
        if definitions.is_empty() {
            return Err(Failure::ProfileConfig);
        }
        Ok(Self {
            definitions,
            custom,
            nos,
            wheels,
            appearance,
            meshes,
        })
    }
    pub fn definition(&self, guid: &Guid) -> Option<&Definition> {
        self.definitions.get(guid).map(Arc::as_ref)
    }
    pub fn construct(
        &self,
        request: Request<'_>,
        authority: &mut authority::Registry,
        resolve_level: impl FnMut(&str) -> Option<u16>,
    ) -> Result<Body, Error> {
        let item = request
            .items
            .items
            .get(&request.vehicle)
            .ok_or(Error::UnknownObject)?;
        let d = self
            .definition(&item.definition)
            .ok_or(Error::Unsupported)?;
        let mut root = d.root.instantiate()?;
        root.set_player(request.connection);
        root.set_team(d.team)?;
        if request.customization_attached {
            root.attach_customization();
        }
        let position = if request.world {
            request.locator
        } else {
            d.resting
                .position_at(request.locator)?
                .map(|v| v.map_or(0., f32::from_bits))
        };
        let spawn = initialize::Spawn::new(position, request.basis[2], request.speed)?;
        let mut pending = authority.clone();
        let creation = initialize::fresh(
            &d.profile,
            initialize::Items {
                collection: request.items,
                vehicle: request.vehicle,
                customization: &self.custom,
                nos: &self.nos,
            },
            initialize::Lifecycle {
                root: &root,
                pose: initialize::Pose {
                    position,
                    basis: request.basis,
                    origin: request.origin,
                },
                spawn: &spawn,
                setup: None,
                authority_group: request.authority_group,
                authority_components: request.authority_components,
            },
            &mut pending,
        )?;
        let custom = Customization::from_vehicle(request.items, &self.custom, request.vehicle)?;
        let mut chassis = chassis::State::new(
            position,
            orientation::quaternion_from_basis(request.basis)?,
            if request.world { WORLD_MODE } else { d.mode },
            custom,
        )?;
        if request.world {
            let mut controls = chassis::Controls::new(WORLD_MODE)?;
            controls.mode_time = WORLD_MODE_TIME;
            chassis.set_motion(chassis.motion().clone(), controls)?;
        }
        let wheels = wheel_customization::State::from_vehicle(
            request.items,
            &self.wheels,
            request.vehicle,
            d.wheel_baseline,
        )?;
        let appearance = appearance::State::from_vehicle(
            request.items,
            &self.appearance,
            request.vehicle,
            &d.appearance,
            request.owner,
        )?;
        let mesh = mesh::State::from_vehicle(
            request.items,
            &self.meshes,
            request.vehicle,
            &d.meshes,
            wheels.tire_indices(),
            resolve_level,
        )?;
        let mut updates = Vec::with_capacity(d.profile.kinds().len());
        for (ordinal, kind) in d.profile.kinds().iter().enumerate() {
            let update = match kind {
                Kind::Root { .. } => Some(root.update(true)?),
                Kind::Chassis => Some(chassis.update(request.origin, 11)?),
                Kind::Part { variants } => components::Part::new(*variants)?
                    .update(*d.parts.get(&ordinal).ok_or(Error::Shape)?, true),
                Kind::TripleNibbles => Some(lifecycle::Effects::default().update(true)),
                Kind::Bool => Some(lifecycle::PlayerEntry::default().update(true)),
                Kind::Wheel => Some(components::Wheel::default().update(true)),
                Kind::Index => Some(nos::Runtime::new(1.)?.update(3)?),
                Kind::Tuning => Some(wheels.update(true)),
                Kind::Tagged => Some(lifecycle::LockOn::default().update(true)),
                Kind::NibblesGuid => Some(lifecycle::Audio::default().update(true)),
                Kind::FourBit => Some(Update::Noop),
                Kind::Appearance => Some(appearance.update(63)?),
                Kind::Mesh(_) => Some(mesh.update(true)),
                Kind::GuidFloat => Some(d.health.update(3)?),
            };
            updates.push(update);
        }
        let body = Body {
            creation: Some(creation),
            updates,
        };
        body.encode(&d.profile)?;
        *authority = pending;
        Ok(body)
    }
}

#[cfg(test)]
pub(crate) mod tests;
