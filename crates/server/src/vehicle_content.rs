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
pub use nfs_content::vehicle_profiles::Definition;

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

#[derive(Clone, Debug)]
pub struct EquipmentPresentation {
    pub body: Body,
    pub registrations: registrations::Registry,
    pub messages: Vec<nfs_world::content::Message>,
}

impl Content {
    pub fn validate_rim_size(
        &self,
        items: &Collection,
        vehicle: u64,
        item: u64,
        size: u8,
    ) -> Result<(), Error> {
        let car = items.items.get(&vehicle).ok_or(Error::UnknownObject)?;
        let rim = items.items.get(&item).ok_or(Error::UnknownObject)?;
        let definition = self
            .definition(&car.definition)
            .ok_or(Error::UnknownObject)?;
        if rim.owner != vehicle
            || !car.children.contains(&item)
            || !self
                .wheels
                .allows_size(&rim.definition, size, definition.wheel_baseline)
        {
            return Err(Error::Bound);
        }
        Ok(())
    }
    /// Build and encode all inventory-derived components before an equipment
    /// transaction commits. Motion, authority and initial creation are excluded.
    pub fn equipment(
        &self,
        items: &Collection,
        vehicle: u64,
        owner: Resource,
        registrations: &registrations::Registry,
    ) -> Result<EquipmentPresentation, Error> {
        let item = items.items.get(&vehicle).ok_or(Error::UnknownObject)?;
        let d = self
            .definition(&item.definition)
            .ok_or(Error::Unsupported)?;
        let custom = self.customization(items, vehicle)?;
        let wheels = wheel_customization::State::from_vehicle(
            items,
            &self.wheels,
            vehicle,
            d.wheel_baseline,
        )?;
        let appearance = appearance::State::from_vehicle(
            items,
            &self.appearance,
            vehicle,
            &d.appearance,
            owner,
        )?;
        let mut registrations = registrations.clone();
        let mut messages = Vec::new();
        let mesh = mesh::State::from_vehicle(
            items,
            &self.meshes,
            vehicle,
            &d.meshes,
            wheels.tire_indices(),
            |name| registrations.resolve_mesh(name, &mut messages).ok(),
        )?;
        let updates = d
            .profile
            .kinds()
            .iter()
            .map(|kind| {
                Ok(match kind {
                    Kind::Chassis => Some(Update::Chassis(Box::new(ChassisUpdate {
                        physics: None,
                        flag: None,
                        custom: Some(custom.clone()),
                        pair: None,
                    }))),
                    Kind::Tuning => Some(wheels.update(true)),
                    Kind::Appearance => Some(appearance.update(63)?),
                    Kind::Mesh(_) => Some(mesh.update(true)),
                    _ => None,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let body = Body {
            creation: None,
            updates,
        };
        body.encode(&d.profile)?;
        Ok(EquipmentPresentation {
            body,
            registrations,
            messages,
        })
    }
    pub fn transmission_values(&self, items: &Collection, vehicle: u64) -> Result<[u8; 2], Error> {
        self.custom.tuning().transmission_values(items, vehicle)
    }
    pub fn load(path: &Path, classes: &Catalog) -> Result<Self, Failure> {
        Self::load_with_parts(path, classes, None)
    }
    /// Version 2 separates vehicle profiles from part definitions. Legacy
    /// version 1 continues using its embedded definitions even with a game dir.
    pub fn load_with_parts(
        path: &Path,
        classes: &Catalog,
        parts: Option<&Path>,
    ) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Failure::BodyLimit);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?;
        let parts = if value["version"] == 2 {
            Some(
                nfs_content::vehicle_parts::Catalog::load(
                    parts.ok_or(Failure::ProfileConfig)?,
                    classes,
                )
                .map_err(|_| Failure::ProfileConfig)?,
            )
        } else {
            None
        };
        Self::from_json_with_parts(&value, classes, parts.as_ref())
    }
    pub fn from_json(value: &Value, classes: &Catalog) -> Result<Self, Failure> {
        Self::from_json_with_parts(value, classes, None)
    }
    pub fn from_json_with_parts(
        value: &Value,
        classes: &Catalog,
        parts: Option<&nfs_content::vehicle_parts::Catalog>,
    ) -> Result<Self, Failure> {
        use parse::*;
        let parsed = match value["version"].as_u64() {
            Some(1) => {
                keys(
                    value,
                    &["format", "version", "build_sha256", "parts", "vehicles"],
                    &[],
                )?;
                nfs_content::vehicle_parts::Catalog::from_rows(&value["parts"], classes).map_err(
                    |e| match e {
                        nfs_content::vehicle_parts::ParseError::Invalid => Failure::ProfileConfig,
                        nfs_content::vehicle_parts::ParseError::Limit => Failure::BodyLimit,
                    },
                )?
            }
            Some(2) => {
                keys(
                    value,
                    &["format", "version", "build_sha256", "vehicles"],
                    &[],
                )?;
                parts
                    .filter(|p| p.matches_classes(classes))
                    .ok_or(Failure::ProfileConfig)?
                    .clone()
            }
            _ => return Err(Failure::ProfileConfig),
        };
        if value["format"] != "nfs-vehicle-content" || value["build_sha256"] != BUILD {
            return Err(Failure::ProfileConfig);
        }
        let nfs_content::vehicle_parts::Catalog {
            customization: custom,
            nos,
            wheels,
            appearance,
            meshes,
            ..
        } = parsed;
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
    /// The nitrous item definition installed on `vehicle`, the one whose
    /// variant the car replicates (E921).
    pub fn nitrous_definition(&self, items: &Collection, vehicle: u64) -> Option<Guid> {
        let state = nos::State::from_vehicle(items, &self.nos, vehicle).ok()?;
        self.nos.guid_for_variant(state.variant?)
    }
    pub fn nitrous_setting(&self, items: &Collection, vehicle: u64) -> Option<u8> {
        let state = nos::State::from_vehicle(items, &self.nos, vehicle).ok()?;
        state.variant.map(|_| state.setting)
    }
    pub fn customization(&self, items: &Collection, vehicle: u64) -> Result<Customization, Error> {
        Customization::from_vehicle(items, &self.custom, vehicle)
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
                Kind::FourBit | Kind::Empty => Some(Update::Noop),
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
