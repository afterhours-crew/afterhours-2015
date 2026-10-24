// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    Definition, Rules, defaults,
    fields::*,
    graph::{self, Node, unique},
};
use crate::{
    Error, assets::AssetIndex, items::ITEM_SYSTEM, scan::Assets, scenes::MAX_GRAPH_OBJECTS,
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::{
    items::{Catalog, DefinitionClass, MAX_DEFINITIONS},
    replication::vehicle::{MAX_MESHES, MAX_SERIALIZERS},
};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

fn meshes(
    assets: &mut Assets,
    root: &Key,
    blueprint: &Object,
    nodes: &[Node],
    rules: &Rules,
) -> Result<Vec<Json>, Error> {
    let source = &unique(nodes, "MeshCustomizationNetworkComponentData")?.key;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for value in array(&blueprint.fields, "LinkConnections", MAX_GRAPH_OBJECTS)? {
        let link = value.as_fields().ok_or_else(|| bad("link connection"))?;
        if word(link, "SourceFieldId")? != rules.mesh_link {
            continue;
        }
        if resolve(assets, &root.0, pointer(link, "Source")?)? != *source {
            continue;
        }
        if word(link, "TargetFieldId")? != 0 {
            return Err(bad("indirect mesh link"));
        }
        let key = resolve(assets, &root.0, pointer(link, "Target")?)?;
        if !seen.insert(key.clone()) || nodes.iter().filter(|n| n.key == key).count() != 1 {
            return Err(bad("duplicate or unconstructed mesh target"));
        }
        let object = object(assets, &key)?;
        let extra = rules
            .mesh_classes
            .get(&object.class)
            .ok_or_else(|| bad("mesh component class"))?;
        let mut variants = Vec::new();
        for value in array(&object.fields, "CustomizableMeshInfos", 256)? {
            let key = resolve(
                assets,
                &key.0,
                value.as_pointer().ok_or_else(|| bad("mesh info pointer"))?,
            )?;
            let info = object_for_mesh(assets, &key)?;
            let guids = array(&info.fields, "BlueprintGuids", 256)?;
            let first = guids
                .first()
                .map(|v| match v {
                    Value::Guid(guid) => Ok(nfs_frostbite::hex(guid)),
                    _ => Err(bad("mesh variant GUID")),
                })
                .transpose()?;
            let bundle = if boolean(&info.fields, "BundledWithComponent")? {
                None
            } else {
                Some(text(
                    structure(&info.fields, "OwningBundleReference")?,
                    "Name",
                )?)
            };
            variants.push(json!({"guid":first,"bundle":bundle}));
        }
        rows.push(json!({"index_bits":integer(&object.fields,"BitsRequiredForIndex")?,"extra":extra,"variants":variants}));
        if rows.len() > MAX_MESHES {
            return Err(bad("mesh component bound"));
        }
    }
    if rows.is_empty() {
        return Err(bad("mesh links missing"));
    }
    Ok(rows)
}
fn object_for_mesh(assets: &mut Assets, key: &Key) -> Result<Object, Error> {
    let object = object(assets, key)?;
    if object.class != "CustomizableMeshInfo" {
        return Err(bad("mesh info class"));
    }
    Ok(object)
}

fn profile(
    index: &mut AssetIndex,
    bundle: &str,
    aliases: &[String],
    rules: &Rules,
) -> Result<Json, Error> {
    let (root, blueprint, nodes) = composition(index, bundle, rules)?;
    let meshes = meshes(&mut index.assets, &root, &blueprint, &nodes, rules)?;
    let mut row = defaults::build(&mut index.assets, &nodes)?;
    let fixed = fields(index, bundle, &root, &blueprint, &nodes, &meshes, rules)?;
    row.as_object_mut()
        .expect("defaults object")
        .extend(fixed.as_object().expect("profile object").clone());
    row["definitions"] = json!(aliases);
    row["meshes"] = json!(meshes);
    let info = super::info::read(&graph::unique(&nodes, "RaceVehicleEntityData")?.object)?;
    row["vehicle_info"] = json!({"bundle": info.bundle, "short_name": info.short_name.as_str()});
    Definition::from_json(&row)
        .map_err(|e| bad(&format!("invalid derived profile {bundle}: {e:?}")))?;
    Ok(row)
}

pub(super) fn composition(
    index: &mut AssetIndex,
    bundle: &str,
    rules: &Rules,
) -> Result<(Key, Object, Vec<Node>), Error> {
    let bundle_key = (bundle.to_lowercase(), 0);
    let bundle_object = object(&mut index.assets, &bundle_key)?;
    if bundle_object.class != "BlueprintBundle" {
        return Err(bad("vehicle bundle class"));
    }
    let (root, blueprint) = follow(
        &mut index.assets,
        &bundle_key.0,
        &bundle_object.fields,
        "Blueprint",
        "RaceVehicleBlueprint",
    )?;
    let nodes = graph::read(&mut index.assets, root.clone(), rules)?;
    Ok((root, blueprint, nodes))
}

pub(super) fn fields(
    index: &mut AssetIndex,
    bundle: &str,
    root: &Key,
    blueprint: &Object,
    nodes: &[Node],
    meshes: &[Json],
    rules: &Rules,
) -> Result<Json, Error> {
    let mut serializers = Vec::new();
    let mut parts = Vec::new();
    let mut buses = Vec::new();
    for node in nodes {
        if rules.bus_classes.contains(&node.object.class) {
            buses.push(json!({"path":node.path,"flags":word(&node.object.fields,"Flags")?}));
        }
        let Some(kind) = &rules.classes[&node.object.class] else {
            continue;
        };
        let mut serializer = json!({"kind":kind});
        match kind.as_str() {
            "root" => {
                if word(&blueprint.fields, "Flags")? & 1 == 0 {
                    return Err(bad("root property owner is absent"));
                }
                serializer["property_owner"] = json!(true);
            }
            "part" => {
                let variants = array(&node.object.fields, "HealthStates", 256)?.len();
                serializer["variants"] = json!(variants);
                parts.push(json!({"ordinal":serializers.len(),"variants":variants,
                    "networkable":boolean(&node.object.fields,"IsNetworkable")?}));
            }
            "mesh" => {
                serializer["entries"] = json!(
                    meshes
                        .iter()
                        .map(|m| json!({"index_bits":m["index_bits"],"extra":m["extra"]}))
                        .collect::<Vec<_>>()
                )
            }
            _ => (),
        }
        serializers.push(serializer);
        if serializers.len() > MAX_SERIALIZERS {
            return Err(bad("serializer bound"));
        }
    }
    let blueprint_name = text(&blueprint.fields, "Name")?;
    let reference = index.reference(bundle, &root.0)?;
    let catalog = index.catalog(bundle)?;
    Ok(json!({"blueprint":blueprint_name,"bundle":bundle,
        "asset":{"type_id":reference.class_id,"local_index":reference.local_index},"catalog":catalog,
        "profile":serializers,"parts":parts,"buses":buses}))
}

pub(crate) fn build(index: &mut AssetIndex, classes: &Catalog) -> Result<Vec<u8>, Error> {
    let rules = index.identity.vehicles.clone();
    let system = index
        .assets
        .objects(ITEM_SYSTEM)?
        .first()
        .cloned()
        .ok_or_else(|| bad("item system"))?;
    let mut bundles = BTreeMap::<String, Vec<String>>::new();
    let mut seen = BTreeSet::new();
    for value in array(&system.fields, "Items", MAX_DEFINITIONS)? {
        let key = resolve(
            &mut index.assets,
            ITEM_SYSTEM,
            value.as_pointer().ok_or_else(|| bad("item pointer"))?,
        )?;
        let item = object(&mut index.assets, &key)?;
        if item.class != "RaceVehicleItemData" {
            continue;
        }
        let guid = item.guid.ok_or_else(|| bad("unexported vehicle item"))?;
        if classes.class(&guid).ok() != Some(DefinitionClass::RaceVehicleItemData)
            || !seen.insert(guid)
        {
            return Err(bad("vehicle alias catalog mismatch or duplicate"));
        }
        let (_, reference) = follow(
            &mut index.assets,
            &key.0,
            &item.fields,
            "RaceVehicle",
            "RaceVehicleReference",
        )?;
        let bundle = text(structure(&reference.fields, "BundleReference")?, "Name")?;
        if !bundle.to_lowercase().starts_with(&rules.bundle_prefix) {
            continue;
        }
        bundles
            .entry(bundle)
            .or_default()
            .push(nfs_frostbite::hex(&guid));
    }
    if bundles.is_empty() || bundles.len() > 512 {
        return Err(bad("player vehicle count"));
    }
    let mut rows = Vec::new();
    let mut names = BTreeSet::new();
    for (bundle, mut aliases) in bundles {
        aliases.sort();
        let row = profile(index, &bundle, &aliases, &rules)?;
        if !names.insert(row["blueprint"].as_str().expect("name").to_lowercase()) {
            return Err(bad("duplicate blueprint"));
        }
        rows.push(row);
    }
    let bytes = serde_json::to_vec(&json!({"format":"nfs-vehicle-content","version":2,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"vehicles":rows}))
    .map_err(|_| bad("JSON encoding"))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(bad("output size bound"));
    }
    Ok(bytes)
}
