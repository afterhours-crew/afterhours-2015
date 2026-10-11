// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Failure, parse::*};
use nfs_world::{
    content::bindings::name_key,
    garage::Slots,
    items::{Collection, Derived, OWNED},
    replication::Error,
};
use serde_json::Value;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Slot {
    pub component_index: usize,
    pub item_component_index: usize,
    transform: [u32; 16],
}
impl Slot {
    pub fn locator(&self) -> [f32; 3] {
        std::array::from_fn(|i| f32::from_bits(self.transform[12 + i]))
    }
    pub fn basis(&self) -> [[f32; 3]; 3] {
        std::array::from_fn(|r| std::array::from_fn(|c| f32::from_bits(self.transform[r * 4 + c])))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub scene_key: u32,
    pub item_scene_key: u32,
    slots: [Slot; 5],
    /// Root pose of the player's car when it leaves the garage (exact root
    /// position, not a resting locator). Optional deployment content.
    world_spawn: Option<Slot>,
    /// Destination of the host teleport after garage exit: the SpawnPoints
    /// TeleportLocation transform. Optional deployment content.
    exit_teleport: Option<Slot>,
}
#[derive(Clone, Copy, Debug)]
pub struct Occupied<'a> {
    pub ordinal: usize,
    pub item: u64,
    pub slot: &'a Slot,
}
impl Layout {
    pub fn load(path: &Path) -> Result<Self, Failure> {
        let mut raw = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(16385)
            .read_to_end(&mut raw)
            .map_err(|_| Failure::Output)?;
        if raw.len() > 16384 {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(&serde_json::from_slice(&raw).map_err(|_| Failure::ProfileConfig)?)
    }
    pub fn from_json(v: &Value) -> Result<Self, Failure> {
        keys(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "blueprint",
                "item_blueprint",
                "main",
            ],
            &["world_spawn", "exit_teleport"],
        )?;
        if v["format"] != "nfs-garage-population"
            || v["version"] != 1
            || v["build_sha256"] != super::BUILD
        {
            return Err(Failure::ProfileConfig);
        }
        let scene_key =
            name_key(text(&v["blueprint"])?.as_bytes()).map_err(|_| Failure::ProfileConfig)?;
        let item_scene_key =
            name_key(text(&v["item_blueprint"])?.as_bytes()).map_err(|_| Failure::ProfileConfig)?;
        if scene_key == item_scene_key {
            return Err(Failure::ProfileConfig);
        }
        let mut slots = Vec::new();
        let mut slot_indices = BTreeSet::new();
        let mut item_indices = BTreeSet::new();
        for (ordinal, row) in array(&v["main"], 5)?.iter().enumerate() {
            keys(
                row,
                &[
                    "ordinal",
                    "component_index",
                    "item_component_index",
                    "transform_bits",
                ],
                &[],
            )?;
            let component_index = uint::<usize>(&row["component_index"])?;
            let item_component_index = uint::<usize>(&row["item_component_index"])?;
            let transform: [u32; 16] = array(&row["transform_bits"], 16)?
                .iter()
                .map(uint)
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| Failure::ProfileConfig)?;
            if uint::<usize>(&row["ordinal"])? != ordinal
                || !(1..512).contains(&component_index)
                || !(1..512).contains(&item_component_index)
                || !slot_indices.insert(component_index)
                || !item_indices.insert(item_component_index)
                || transform.iter().any(|b| !f32::from_bits(*b).is_finite())
                || [3, 7, 11, 15].iter().any(|i| transform[*i] != 0)
            {
                return Err(Failure::ProfileConfig);
            }
            let slot = Slot {
                component_index,
                item_component_index,
                transform,
            };
            nfs_world::replication::vehicle::orientation::quaternion_from_basis(slot.basis())
                .map_err(|_| Failure::ProfileConfig)?;
            slots.push(slot);
        }
        let transform_slot = |row: &Value| -> Result<Slot, Failure> {
            keys(row, &["transform_bits"], &[])?;
            let transform: [u32; 16] = array(&row["transform_bits"], 16)?
                .iter()
                .map(uint)
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| Failure::ProfileConfig)?;
            if transform.iter().any(|b| !f32::from_bits(*b).is_finite())
                || [3, 7, 11, 15].iter().any(|i| transform[*i] != 0)
            {
                return Err(Failure::ProfileConfig);
            }
            let slot = Slot {
                component_index: 0,
                item_component_index: 0,
                transform,
            };
            nfs_world::replication::vehicle::orientation::quaternion_from_basis(slot.basis())
                .map_err(|_| Failure::ProfileConfig)?;
            Ok(slot)
        };
        let exit_teleport = v.get("exit_teleport").map(transform_slot).transpose()?;
        let world_spawn = v.get("world_spawn").map(transform_slot).transpose()?;
        Ok(Self {
            scene_key,
            item_scene_key,
            slots: slots.try_into().map_err(|_| Failure::ProfileConfig)?,
            world_spawn,
            exit_teleport,
        })
    }
    pub fn world_spawn(&self) -> Option<&Slot> {
        self.world_spawn.as_ref()
    }
    pub fn exit_teleport(&self) -> Option<&Slot> {
        self.exit_teleport.as_ref()
    }
    pub fn occupied<'a>(
        &'a self,
        slots: Slots,
        items: &Collection,
    ) -> Result<Vec<Occupied<'a>>, Error> {
        slots
            .values()
            .iter()
            .enumerate()
            .filter_map(|(ordinal, item)| item.map(|item| (ordinal, item)))
            .map(|(ordinal, item)| {
                let value = items.items.get(&item).ok_or(Error::UnknownObject)?;
                if !items.roots.contains(&item)
                    || value.id != item
                    || value.owner != 0
                    || value.state & OWNED == 0
                    || !matches!(value.derived, Derived::RaceVehicle { .. })
                {
                    return Err(Error::Shape);
                }
                Ok(Occupied {
                    ordinal,
                    item,
                    slot: &self.slots[ordinal],
                })
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests;
