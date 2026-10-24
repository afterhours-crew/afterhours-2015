// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The intro's display slots refer to authored items, not persistent inventory.
use super::*;

// VehicleSlotEntity's authored item link in the supported build.
pub(super) const SLOT_ITEM: u32 = 3_147_314_859;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Slot {
    pub component: usize,
    pub item_component: usize,
    pub definition: [u8; 16],
    pub transform: [u32; 16],
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
pub struct Garage {
    pub scene_key: u32,
    pub creation_scene_key: u32,
    pub component: usize,
    pub slots: [Slot; 3],
}

pub fn extract(index: &mut AssetIndex) -> Result<Garage, Error> {
    let directory = index
        .identity
        .scenes
        .level
        .rsplit_once('/')
        .ok_or_else(|| bad("level path"))?
        .0;
    let scene = format!("{directory}/garage").to_lowercase();
    let rules = index.identity.garage_links.clone();
    Links::from_json(&rules.to_json())?;
    let (_, instances) =
        scenes::construct(&mut index.assets, &index.identity.scenes, &scene, true)?;
    let assets = &mut index.assets;
    let edges = connection_rows(assets, &scene, "LinkConnections")?;
    let properties = connection_rows(assets, &scene, "PropertyConnections")?;
    let mut candidates = vec![];
    for source in objects_of_class(assets, &instances, "GarageEntityData")? {
        let mut primary = links(assets, &scene, &edges, &source, rules.primary_slot)?;
        let secondary = links(assets, &scene, &edges, &source, rules.secondary_slots)?;
        if primary.len() == 1 && secondary.len() == 2 {
            primary.extend(secondary);
            candidates.push((source, primary));
        }
    }
    let (garage, slots) = only(candidates, "missing or ambiguous three-slot intro garage")?;
    if slots.iter().collect::<BTreeSet<_>>().len() != 3 {
        return Err(bad("duplicate starter slot"));
    }
    let component = serializer(assets, &instances, &garage, "GarageEntityData")?;
    let mut result = vec![];
    let mut definitions = BTreeSet::new();
    for slot in slots {
        let component = serializer(assets, &instances, &slot, "VehicleSlotEntityData")?;
        let item = only(
            links(assets, &scene, &edges, &slot, SLOT_ITEM)?,
            "starter default item",
        )?;
        let item_component = serializer(assets, &instances, &item, "ItemEntityData")?;
        let pointer = assets.objects(&item.0)?[item.1]
            .fields
            .get("Item")
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("starter item pointer"))?;
        let key = assets
            .target(&item.0, pointer)?
            .ok_or_else(|| bad("starter item null"))?;
        let object = assets
            .objects(&key.0)?
            .get(key.1)
            .ok_or_else(|| bad("starter definition index"))?;
        if object.class != "RaceVehicleItemData" {
            return Err(bad("starter definition class"));
        }
        let definition = object
            .guid
            .filter(|g| *g != [0; 16])
            .ok_or_else(|| bad("starter definition export"))?;
        if !definitions.insert(definition) {
            return Err(bad("duplicate starter definition"));
        }
        let mut locators = vec![];
        for row in &properties {
            if id(row, "TargetFieldId")? == rules.slot_location
                && target(assets, &scene, row, "Target")? == slot
            {
                if id(row, "SourceFieldId")? != rules.locator_transform || id(row, "Flags")? != 1 {
                    return Err(bad("unsupported starter location property"));
                }
                locators.push(target(assets, &scene, row, "Source")?);
            }
        }
        let locator = only(locators, "starter locator")?;
        occurrence(&instances, &locator)?;
        result.push(Slot {
            component,
            item_component,
            definition,
            transform: transform(assets, &locator)?,
        });
    }
    Ok(Garage {
        scene_key: crate::assets::scene_key(&scene),
        creation_scene_key: crate::assets::scene_key(&format!("{directory}/gameplay")),
        component,
        slots: result.try_into().map_err(|_| bad("starter slot count"))?,
    })
}
