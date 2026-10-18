// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ephemeral authored vehicle setups. These never grant items to an account,
//! allocate persistent identities or consult a player's purchase/unlock state.
use crate::Error;
use nfs_services::purchase::Offers;
use nfs_world::replication::vehicle::wheel_customization;
use nfs_world_core::items::{
    Collection, DefinitionClass, Derived, Guid, InventoryCatalog, MAX_ITEMS,
};

pub const MAX_PARTS: usize = 128;
const MAX_PRESET_ITEMS: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RimStyle {
    pub selection: Option<u32>,
    pub primary_color: Option<[f32; 3]>,
    pub primary_material: Option<[f32; 3]>,
    pub secondary_color: Option<[f32; 3]>,
    /// Retained authored metadata. The native style applicator uses the primary
    /// material for both channels; this field does not override that behavior.
    pub secondary_material: Option<[f32; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub parts: Vec<Guid>,
    pub color: Option<[f32; 3]>,
    pub material: Option<[f32; 3]>,
    pub window_tint: Option<f32>,
    pub license_plate: Option<[u8; 8]>,
    /// Caster, front/rear camber, height, rake, front/rear toe, front/rear track.
    pub alignment: [Option<u8>; 9],
    pub rims: [RimStyle; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub vehicle: Guid,
    pub color: Option<[f32; 3]>,
    pub style: Option<Style>,
    pub additional: Vec<Guid>,
}

fn bad() -> Error {
    Error::Content("rival vehicle preset".into())
}
fn finite(value: Option<[f32; 3]>) -> bool {
    value.is_none_or(|v| v.iter().all(|x| x.is_finite() && x.abs() <= 1_000_000.))
}

impl Preset {
    /// The returned collection owns a private, repeatable identity space rooted
    /// at 1. It is construction input, never a persistent inventory transaction.
    pub fn instantiate(
        &self,
        catalog: &InventoryCatalog,
        slots: &Offers,
        wheels: &wheel_customization::Definitions,
    ) -> Result<Collection, Error> {
        if !finite(self.color) || self.additional.len() > MAX_PARTS {
            return Err(bad());
        }
        let mut built = catalog
            .instantiate_vehicle(self.vehicle, 1)
            .map_err(|e| Error::Content(format!("rival root construction: {e}")))?;
        if let Some(color) = self.color {
            let Derived::RaceVehicle { default_paint, .. } =
                &mut built.collection.items.get_mut(&1).ok_or_else(bad)?.derived
            else {
                return Err(bad());
            };
            *default_paint = color.map(f32::to_bits);
        }
        if let Some(style) = &self.style {
            style.validate()?;
            for part in &style.parts {
                equip(
                    &mut built.collection,
                    &mut built.next_id,
                    *part,
                    catalog,
                    slots,
                )?;
            }
            style.apply(&mut built.collection, slots, wheels)?;
        }
        // Explicit persona additions are equipped AFTER style values, just as
        // the shared vehicle spawner does. Definition AdditionalItems are not
        // recursively fabricated as owned children.
        for part in &self.additional {
            equip(
                &mut built.collection,
                &mut built.next_id,
                *part,
                catalog,
                slots,
            )?;
        }
        built.collection.roots = built.collection.items.keys().copied().collect();
        built
            .collection
            .encode(catalog.bindings())
            .map_err(|_| bad())?;
        Ok(built.collection)
    }
}

fn equip(
    items: &mut Collection,
    next_id: &mut u64,
    definition: Guid,
    catalog: &InventoryCatalog,
    slots: &Offers,
) -> Result<(), Error> {
    let part_id = *next_id;
    let new = catalog
        .instantiate_owned(definition, 1, part_id)
        .map_err(|e| {
            Error::Content(format!(
                "rival part {} construction: {e}",
                nfs_frostbite::hex(&definition)
            ))
        })?;
    if items
        .items
        .len()
        .checked_add(new.collection.items.len())
        .is_none_or(|n| n > MAX_PRESET_ITEMS.min(MAX_ITEMS))
    {
        return Err(bad());
    }
    *next_id = new.next_id;
    items.items.extend(new.collection.items);
    let same = |a: u64, b: u64| {
        slots
            .same_slot(
                catalog,
                &items.items[&a].definition,
                &items.items[&b].definition,
            )
            .map_err(|e| Error::Content(format!("rival slot comparison: {e:?}")))
    };
    let is_kit = |id: u64| {
        catalog
            .definition(&items.items[&id].definition)
            .map(|d| d.class == DefinitionClass::BodyKitItemData)
            .map_err(|_| bad())
    };
    let mut children = items.items[&1].children.clone();
    if is_kit(part_id)? {
        let previous = children
            .iter()
            .copied()
            .map(|id| Ok((id, is_kit(id)?)))
            .collect::<Result<Vec<_>, Error>>()?
            .into_iter()
            .find_map(|(id, kit)| kit.then_some(id));
        let kit_children = &items.items[&part_id].children;
        let mut retained = Vec::new();
        for id in children {
            if Some(id) == previous {
                continue;
            }
            let mut covered = false;
            for &child in kit_children {
                covered |= same(id, child)?;
            }
            if !covered {
                retained.push(id);
            }
        }
        retained.push(part_id);
        if previous.is_some() {
            // Native kit replacement restores uncovered mandatory stock slots.
            for &id in &items.items[&1].defaults {
                let d = catalog
                    .definition(&items.items[&id].definition)
                    .map_err(|_| bad())?;
                if d.flags.optional || !nfs_services::purchase::visual_class(d.class) {
                    continue;
                }
                let mut covered = false;
                for &other in retained.iter().chain(kit_children) {
                    covered |= same(id, other)?;
                }
                if !covered {
                    retained.push(id);
                }
            }
        }
        children = retained;
    } else {
        let mut replacement = None;
        for (i, &id) in children.iter().enumerate() {
            if same(id, part_id)? {
                replacement = Some(i);
                break;
            }
        }
        if let Some(i) = replacement {
            children.remove(i);
        }
        children.push(part_id);
    }
    items.items.get_mut(&1).ok_or_else(bad)?.children = children;
    Ok(())
}

impl Style {
    pub fn validate(&self) -> Result<(), Error> {
        if self.parts.len() > MAX_PARTS
            || !finite(self.color)
            || !finite(self.material)
            || self
                .window_tint
                .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
            || self.alignment.iter().flatten().any(|&v| v > 10)
            || self.rims.iter().any(|r| {
                !finite(r.primary_color)
                    || !finite(r.primary_material)
                    || !finite(r.secondary_color)
                    || !finite(r.secondary_material)
                    || r.selection.is_some_and(|v| v > i32::MAX as u32)
            })
        {
            return Err(bad());
        }
        Ok(())
    }

    fn apply(
        &self,
        items: &mut Collection,
        slots: &Offers,
        wheels: &wheel_customization::Definitions,
    ) -> Result<(), Error> {
        let car = items.items.get_mut(&1).ok_or_else(bad)?;
        let Derived::RaceVehicle {
            default_paint,
            material,
            window_tint,
            default_license_plate_text,
            ..
        } = &mut car.derived
        else {
            return Err(bad());
        };
        if let Some(v) = self.color {
            *default_paint = v.map(f32::to_bits);
        }
        if let Some(v) = self.material {
            *material = v.map(f32::to_bits);
        }
        if let Some(v) = self.window_tint {
            *window_tint = v.to_bits();
        }
        if let Some(v) = self.license_plate {
            *default_license_plate_text = v;
        }
        // The native applicator visits direct installed children, not owned
        // spares or nested kit children.
        for id in car.children.clone() {
            let item = items.items.get_mut(&id).ok_or_else(bad)?;
            match &mut item.derived {
                Derived::ControlArmTuning {
                    caster_setting,
                    camber_front_setting,
                    camber_rear_setting,
                    ride_height_height_setting,
                    ride_height_rake_setting,
                    toe_front_setting,
                    toe_rear_setting,
                    track_width_front_setting,
                    track_width_rear_setting,
                } => {
                    for (target, value) in [
                        caster_setting,
                        camber_front_setting,
                        camber_rear_setting,
                        ride_height_height_setting,
                        ride_height_rake_setting,
                        toe_front_setting,
                        toe_rear_setting,
                        track_width_front_setting,
                        track_width_rear_setting,
                    ]
                    .into_iter()
                    .zip(self.alignment)
                    {
                        if let Some(value) = value {
                            *target = u32::from(value);
                        }
                    }
                }
                Derived::Rims {
                    rim_selection,
                    primary_paint,
                    primary_material,
                    secondary_paint,
                    secondary_material,
                } => {
                    let rear = slots
                        .location(&item.definition)
                        .ok_or_else(|| Error::Content("rival rim location missing".into()))?
                        != 0;
                    let style = &self.rims[usize::from(rear)];
                    if let Some(v) = style.selection {
                        let count = wheels
                            .rim_variant_count(&item.definition)
                            .ok_or_else(|| Error::Content("rival rim variants missing".into()))?;
                        // The native signed count-minus-one clamp stores -1
                        // for fixed-size rims with an empty options array.
                        *rim_selection = if count == 0 {
                            u32::MAX
                        } else {
                            v.min(count as u32 - 1)
                        };
                    }
                    if let Some(v) = style.primary_color {
                        *primary_paint = v.map(f32::to_bits);
                    }
                    if let Some(v) = style.secondary_color {
                        *secondary_paint = v.map(f32::to_bits);
                    }
                    if let Some(v) = style.primary_material {
                        *primary_material = v.map(f32::to_bits);
                        *secondary_material = v.map(f32::to_bits);
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
