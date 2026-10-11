// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static garage slot topology and transforms. Inventory occupancy belongs to
//! the account. Parking roles and the exported exit destination come from assets.
use crate::{Error, assets::AssetIndex, scan::Assets, scenes};
use nfs_frostbite::ebx::{Fields, Value};
use serde_json::json;
use std::collections::BTreeSet;

/// Port identifiers understood by this build's garage, persistence and exit code.
/// These identify link/property contracts, never particular scene instances.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Links {
    pub primary_slot: u32,
    pub secondary_slots: u32,
    pub slot_location: u32,
    pub locator_transform: u32,
    pub exit_location: u32,
    pub items: [u32; 5],
}
impl Links {
    pub fn from_json(value: &serde_json::Value) -> Result<Self, Error> {
        let fields = [
            "primary_slot",
            "secondary_slots",
            "slot_location",
            "locator_transform",
            "exit_location",
            "items",
        ];
        let object = value.as_object().ok_or_else(|| bad("link profile"))?;
        if object.len() != fields.len() || fields.iter().any(|f| !object.contains_key(*f)) {
            return Err(bad("link profile fields"));
        }
        let number = |v: &serde_json::Value| {
            v.as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v != 0)
                .ok_or_else(|| bad("link profile ID"))
        };
        let items = value["items"]
            .as_array()
            .filter(|a| a.len() == 5)
            .ok_or_else(|| bad("item link profile"))?
            .iter()
            .map(number)
            .collect::<Result<Vec<_>, _>>()?;
        if items.iter().copied().collect::<BTreeSet<_>>().len() != 5 {
            return Err(bad("duplicate item link profile ID"));
        }
        let result = Self {
            primary_slot: number(&value["primary_slot"])?,
            secondary_slots: number(&value["secondary_slots"])?,
            slot_location: number(&value["slot_location"])?,
            locator_transform: number(&value["locator_transform"])?,
            exit_location: number(&value["exit_location"])?,
            items: items.try_into().map_err(|_| bad("item link count"))?,
        };
        if result.primary_slot == result.secondary_slots {
            return Err(bad("ambiguous slot link profile"));
        }
        Ok(result)
    }
    pub fn to_json(&self) -> serde_json::Value {
        json!({"primary_slot":self.primary_slot,"secondary_slots":self.secondary_slots,
            "slot_location":self.slot_location,"locator_transform":self.locator_transform,
            "exit_location":self.exit_location,"items":self.items})
    }
}

type Key = (String, usize);
fn bad(what: &str) -> Error {
    Error::Content(format!("garage layout: {what}"))
}
fn connection_rows(assets: &mut Assets, scene: &str, field: &str) -> Result<Vec<Fields>, Error> {
    assets
        .objects(scene)?
        .first()
        .and_then(|o| o.fields.get(field))
        .and_then(Value::as_array)
        .filter(|a| a.len() <= scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("connection array"))?
        .iter()
        .map(|v| {
            v.as_fields()
                .cloned()
                .ok_or_else(|| bad("connection fields"))
        })
        .collect()
}
fn id(fields: &Fields, name: &str) -> Result<u32, Error> {
    match fields.get(name) {
        Some(Value::Int32(v)) => Ok(*v as u32),
        Some(Value::UInt32(v)) => Ok(*v),
        _ => Err(bad("connection field ID")),
    }
}
fn target(assets: &mut Assets, scene: &str, fields: &Fields, field: &str) -> Result<Key, Error> {
    let pointer = fields
        .get(field)
        .and_then(Value::as_pointer)
        .ok_or_else(|| bad("connection pointer"))?;
    assets
        .target(scene, pointer)?
        .ok_or_else(|| bad("null connection target"))
}
fn links(
    assets: &mut Assets,
    scene: &str,
    rows: &[Fields],
    source: &Key,
    port: u32,
) -> Result<Vec<Key>, Error> {
    let mut result = Vec::new();
    for row in rows {
        if id(row, "SourceFieldId")? == port && target(assets, scene, row, "Source")? == *source {
            if id(row, "TargetFieldId")? != 0 {
                return Err(bad("slot link is not direct"));
            }
            result.push(target(assets, scene, row, "Target")?);
        }
    }
    Ok(result)
}
fn objects_of_class(
    assets: &mut Assets,
    instances: &[scenes::Instance],
    class: &str,
) -> Result<Vec<Key>, Error> {
    let mut result = Vec::new();
    for i in instances {
        if assets
            .objects(&i.asset)?
            .get(i.index)
            .is_some_and(|o| o.class == class)
        {
            result.push((i.asset.clone(), i.index));
        }
    }
    Ok(result)
}
fn serializer(
    assets: &mut Assets,
    instances: &[scenes::Instance],
    key: &Key,
    class: &str,
) -> Result<usize, Error> {
    if assets
        .objects(&key.0)?
        .get(key.1)
        .is_none_or(|o| o.class != class)
    {
        return Err(bad("linked object has the wrong class"));
    }
    occurrence(instances, key)?
        .serializer
        .filter(|i| *i > 0)
        .ok_or_else(|| bad("linked object has no serializer"))
}

fn occurrence<'a>(
    instances: &'a [scenes::Instance],
    key: &Key,
) -> Result<&'a scenes::Instance, Error> {
    let matches: Vec<_> = instances
        .iter()
        .filter(|i| i.asset == key.0 && i.index == key.1)
        .collect();
    if matches.len() != 1 {
        return Err(bad(
            "linked definition has an ambiguous or missing occurrence",
        ));
    }
    Ok(matches[0])
}
fn only<T>(mut values: Vec<T>, what: &str) -> Result<T, Error> {
    if values.len() != 1 {
        return Err(bad(what));
    }
    Ok(values.remove(0))
}
fn transform(assets: &mut Assets, key: &Key) -> Result<[u32; 16], Error> {
    let object = assets
        .objects(&key.0)?
        .get(key.1)
        .ok_or_else(|| bad("locator index"))?;
    if object.class != "LocatorEntityData" {
        return Err(bad("location source is not a locator"));
    }
    let transform = object
        .fields
        .get("Transform")
        .and_then(Value::as_fields)
        .ok_or_else(|| bad("locator transform"))?;
    let mut bits = [0; 16];
    for (r, name) in ["right", "up", "forward", "trans"].iter().enumerate() {
        let vector = transform
            .get(name)
            .and_then(Value::as_fields)
            .ok_or_else(|| bad("transform vector"))?;
        for (c, name) in ["x", "y", "z"].iter().enumerate() {
            bits[r * 4 + c] = match vector.get(name) {
                Some(Value::Float32(v)) if v.is_finite() => v.to_bits(),
                _ => return Err(bad("nonfinite or mistyped transform component")),
            };
        }
    }
    let basis = std::array::from_fn(|r| std::array::from_fn(|c| f32::from_bits(bits[r * 4 + c])));
    nfs_world::replication::vehicle::orientation::quaternion_from_basis(basis)
        .map_err(|_| bad("invalid locator basis"))?;
    Ok(bits)
}

pub(crate) fn build(index: &mut AssetIndex) -> Result<Vec<u8>, Error> {
    let directory = index
        .identity
        .scenes
        .level
        .rsplit_once('/')
        .ok_or_else(|| bad("level path"))?
        .0;
    let garage = format!("{directory}/garage").to_lowercase();
    let gameplay = format!("{directory}/gameplay").to_lowercase();
    let spawnpoints = format!("{directory}/spawnpoints").to_lowercase();
    let rules = index.identity.garage_links.clone();
    // Caller-supplied profiles are checked just as embedded profiles are.
    Links::from_json(&rules.to_json())?;
    let (_, garage_instances) =
        scenes::construct(&mut index.assets, &index.identity.scenes, &garage, true)?;
    let (_, gameplay_instances) =
        scenes::construct(&mut index.assets, &index.identity.scenes, &gameplay, true)?;
    let (_, spawn_instances) = scenes::construct(
        &mut index.assets,
        &index.identity.scenes,
        &spawnpoints,
        true,
    )?;
    let assets = &mut index.assets;
    let exit = exit_location(assets, &spawnpoints, &spawn_instances, &rules)?;
    let garage_links = connection_rows(assets, &garage, "LinkConnections")?;
    let garage_properties = connection_rows(assets, &garage, "PropertyConnections")?;
    let mut candidates = Vec::new();
    for source in objects_of_class(assets, &garage_instances, "GarageEntityData")? {
        let mut primary = links(assets, &garage, &garage_links, &source, rules.primary_slot)?;
        let secondary = links(
            assets,
            &garage,
            &garage_links,
            &source,
            rules.secondary_slots,
        )?;
        if primary.len() == 1 && secondary.len() == 4 {
            primary.extend(secondary);
            candidates.push(primary);
        }
    }
    let slots = only(candidates, "missing or ambiguous five-slot garage")?;
    if slots.iter().collect::<BTreeSet<_>>().len() != 5 {
        return Err(bad("duplicate garage slot"));
    }
    let persistence = only(
        objects_of_class(
            assets,
            &gameplay_instances,
            "PersistentGarageVehiclesEntityData",
        )?,
        "missing or ambiguous persistent garage",
    )?;
    let item_links = connection_rows(assets, &gameplay, "LinkConnections")?;
    let mut main = Vec::new();
    let mut used_items = BTreeSet::new();
    for (ordinal, slot) in slots.iter().enumerate() {
        let component = serializer(assets, &garage_instances, slot, "VehicleSlotEntityData")?;
        let item = only(
            links(
                assets,
                &gameplay,
                &item_links,
                &persistence,
                rules.items[ordinal],
            )?,
            "missing or ambiguous item binding",
        )?;
        if !used_items.insert(item.clone()) {
            return Err(bad("duplicate item slot"));
        }
        let item_component = serializer(assets, &gameplay_instances, &item, "ItemEntityData")?;
        let mut locators = Vec::new();
        for property in &garage_properties {
            if id(property, "TargetFieldId")? == rules.slot_location
                && target(assets, &garage, property, "Target")? == *slot
            {
                if id(property, "SourceFieldId")? != rules.locator_transform
                    || id(property, "Flags")? != 1
                {
                    return Err(bad("unsupported location property"));
                }
                locators.push(target(assets, &garage, property, "Source")?);
            }
        }
        let locator = only(locators, "missing or ambiguous locator")?;
        occurrence(&garage_instances, &locator)?;
        let bits = transform(assets, &locator)?;
        main.push(json!({"ordinal":ordinal,"component_index":component,"item_component_index":item_component,"transform_bits":bits}));
    }
    serde_json::to_vec(&json!({"format":"nfs-garage-population","version":1,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"blueprint":garage,"item_blueprint":gameplay,
        "main":main,"exit_teleport":{"transform_bits":exit}}))
        .map_err(|_|bad("JSON encoding"))
}

/// Follow the scene's exported destination, rather than choosing an arbitrary
/// TeleportLocation entity or a locator by its position in the partition.
fn exit_location(
    assets: &mut Assets,
    scene: &str,
    instances: &[scenes::Instance],
    rules: &Links,
) -> Result<[u32; 16], Error> {
    let root = assets.objects(scene)?[0].fields.clone();
    let interface = target(assets, scene, &root, "Interface")?;
    let descriptor = assets.objects(&interface.0)?[interface.1].clone();
    if descriptor.class != "InterfaceDescriptorData" {
        return Err(bad("exit interface class"));
    }
    let fields = descriptor
        .fields
        .get("Fields")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("exit interface fields"))?;
    let mut exports = Vec::new();
    for field in fields {
        let field = field
            .as_fields()
            .ok_or_else(|| bad("exit interface field"))?;
        if id(field, "Id")? == rules.exit_location {
            exports.push(field);
        }
    }
    let export = only(exports, "missing or ambiguous exit export")?;
    if !matches!(export.get("AccessType"), Some(Value::Enum { value: 0, .. })) {
        return Err(bad("exit field is not a source export"));
    }
    let properties = connection_rows(assets, scene, "PropertyConnections")?;
    let mut sources = Vec::new();
    for property in properties {
        if id(&property, "TargetFieldId")? == rules.exit_location
            && target(assets, scene, &property, "Target")? == interface
        {
            if id(&property, "SourceFieldId")? != rules.locator_transform
                || id(&property, "Flags")? != 17
            {
                return Err(bad("unsupported exit location property"));
            }
            sources.push(target(assets, scene, &property, "Source")?);
        }
    }
    let locator = only(sources, "missing or ambiguous exit locator")?;
    occurrence(instances, &locator)?;
    transform(assets, &locator)
}

#[cfg(test)]
mod tests;
