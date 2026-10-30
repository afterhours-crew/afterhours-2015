// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bind selected integer properties to their actual persistent store columns.
//! Node ordinals and channel schema names never identify a player value.
use super::*;
use crate::scoring::events::Identity;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    pub table: String,
    pub column: String,
    pub player_manager: Identity,
}

fn one<S: Source>(g: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let links = g.links(site, port)?;
    match links.as_slice() {
        [target] if target.port == 0 => Ok(target.site.clone()),
        _ => Err(bad("map integer persistence link missing or ambiguous")),
    }
}

pub(super) fn read<S: Source>(
    g: &mut Graph<S>,
    roots: &[Site],
    selected: &BTreeSet<Identity>,
) -> Result<BTreeMap<Identity, Store>, Error> {
    if selected.len() > MAX_EVENTS || roots.len() > 256 {
        return Err(bad("map integer owner bound"));
    }
    if selected.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut nodes = BTreeMap::new();
    for root in roots {
        for node in g.read(root.clone())? {
            if matches!(
                node.object.class.as_str(),
                "IntPropertyPersistenceInterfaceEntityData"
                    | "PersistentTableUsingStoresEntityData"
            ) {
                if nodes.len() >= 4096 {
                    return Err(bad("map integer persistence node bound"));
                }
                nodes.insert(node.site.clone(), node.object);
            }
        }
    }
    let mut stores = BTreeMap::new();
    for (site, object) in &nodes {
        if object.class != "IntPropertyPersistenceInterfaceEntityData" {
            continue;
        }
        let property = one(g, site, 910_586_961)?;
        let identity = Identity::from(&property);
        if !selected.contains(&identity) {
            continue;
        }
        if g.object(&property)?.class != "IntParticipantPropertyEntityData" {
            return Err(bad("map integer property class"));
        }
        let store = one(g, site, 915_182_239)?;
        let manager = one(g, site, (-1_772_170_871i32) as u32)?;
        if g.object(&manager)?.class != "ParticipantManagerEntityData" {
            return Err(bad("map integer player manager class"));
        }
        if g.object(&store)?.class != "PlayerIntStoreEntityData"
            || stores
                .insert(identity, (store, Identity::from(&manager)))
                .is_some()
        {
            return Err(bad("map integer store class or duplicate owner"));
        }
    }
    if stores.len() != selected.len() {
        return Err(bad("map integer persistence owner missing"));
    }
    let mut result = BTreeMap::new();
    for (table, object) in &nodes {
        if object.class != "PersistentTableUsingStoresEntityData" {
            continue;
        }
        for port in g.link_ports(table)? {
            for link in g.links(table, port)? {
                for (identity, (store, manager)) in &stores {
                    if link.site != *store {
                        continue;
                    }
                    if link.port != 0 {
                        return Err(bad("map integer store port"));
                    }
                    let binding = column(g, table, object, port, manager.clone())?;
                    if result.insert(identity.clone(), binding).is_some() {
                        return Err(bad("map integer store has multiple table bindings"));
                    }
                }
            }
        }
    }
    if result.len() != selected.len() {
        return Err(bad("map integer table binding missing"));
    }
    Ok(result)
}

fn column<S: Source>(
    g: &mut Graph<S>,
    site: &Site,
    object: &Object,
    port: u32,
    player_manager: Identity,
) -> Result<Store, Error> {
    let table = object
        .fields
        .get("TableName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| bad("map integer table name"))?;
    let key = g.target_field(site, "TableAsset")?;
    let schema = g.source.object(&key)?;
    if schema.class != "PersistentTableAsset"
        || schema.fields.get("TableName").and_then(Value::as_str) != Some(table)
        || schema
            .fields
            .get("HasSecondaryKey")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return Err(bad("map integer table schema"));
    }
    let rows = schema
        .fields
        .get("Properties")
        .and_then(Value::as_array)
        .filter(|r| r.len() <= 256)
        .ok_or_else(|| bad("map integer columns"))?;
    let mut found = None;
    for row in rows {
        let fields = row.as_fields().ok_or_else(|| bad("map integer column"))?;
        let column = fields
            .get("Name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or_else(|| bad("map integer column name"))?;
        if name(column) != port {
            continue;
        }
        if fields.get("DataType").and_then(Value::as_enum_name)
            != Some("PersistentPropertyType_Int")
            || found.replace(column.to_owned()).is_some()
        {
            return Err(bad("map integer column type or collision"));
        }
    }
    Ok(Store {
        table: table.to_owned(),
        column: found.ok_or_else(|| bad("map integer column missing"))?,
        player_manager,
    })
}

#[cfg(test)]
mod tests;
