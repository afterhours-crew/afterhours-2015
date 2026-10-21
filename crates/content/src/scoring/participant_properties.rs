// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Integer participant properties named by accessors (#138, E1000). An
//! `IntParticipantPropertyAccessorEntityData` links its identifier to one port
//! of a schematic channel; accessors in different blueprints that name the
//! same channel port read and write the same property of their participant
//! (the event tracking logic writes the category totals the results logic
//! reads).
use super::{events::Identity, field};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::ebx::Value;

pub const ACCESSOR: &str = "IntParticipantPropertyAccessorEntityData";
/// The accessor's identifier link field (installed link rows; E1000).
pub const IDENTIFIER: u32 = 2_249_073_098;
/// Accessor occurrences one blueprint may hold (local bound).
pub const MAX_ACCESSORS: usize = 256;

/// One property: a port of a schematic channel asset.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Property {
    pub channel: String,
    pub port: u32,
}

/// The property an accessor names: its single identifier link to a channel
/// reference port, which the referenced channel asset declares.
pub(crate) fn property<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<Property, Error> {
    if graph.object(site)?.class != ACCESSOR {
        return Err(bad("participant property accessor class"));
    }
    let mut rows = graph.links(site, IDENTIFIER)?;
    if rows.len() != 1 {
        return Err(bad("participant property identifier link"));
    }
    let endpoint = rows.remove(0);
    if endpoint.port == 0
        || graph.object(&endpoint.site)?.class != "LinkedSchematicChannelReferenceEntityData"
    {
        return Err(bad("participant property identifier endpoint"));
    }
    let key = graph.target_field(&endpoint.site, "ChannelAsset")?;
    let asset = graph.source.object(&key)?;
    if asset.class != "SchematicChannelAsset" {
        return Err(bad("participant property channel asset"));
    }
    let declared = field(&asset.fields, "Links")?
        .as_array()
        .ok_or_else(|| bad("participant property channel links"))?
        .iter()
        .filter_map(Value::as_fields)
        .filter_map(|row| row.get("Id"))
        .filter(|id| bits32(id).is_ok_and(|id| id == endpoint.port))
        .count();
    if declared != 1 {
        return Err(bad("participant property port declaration"));
    }
    Ok(Property {
        channel: key.0,
        port: endpoint.port,
    })
}

/// The property a synchronized integer mirrors: its input is the `OutputValue`
/// of an accessor. Other synchronized values mirror none.
pub(crate) fn mirrored<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
) -> Result<Option<Property>, Error> {
    if graph.object(site)?.class != "SyncedIntEntityData" {
        return Ok(None);
    }
    let sources = graph.inputs(site, nfs_world::scoring::synced::IN)?;
    let [source] = sources.as_slice() else {
        return Ok(None);
    };
    if source.port != nfs_world::scoring::nodes::OUTPUT_VALUE
        || graph.object(&source.site)?.class != ACCESSOR
    {
        return Ok(None);
    }
    property(graph, &source.site).map(Some)
}

impl AssetIndex {
    /// Every integer participant property accessor of a blueprint, in
    /// composition traversal order, with the property it names.
    pub fn integer_participant_properties(
        &mut self,
        blueprint: &str,
    ) -> Result<Vec<(Identity, Property)>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("participant property blueprint name"));
        }
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(blueprint))?;
        let mut result = Vec::new();
        for node in nodes.iter().filter(|n| n.object.class == ACCESSOR) {
            if result.len() >= MAX_ACCESSORS {
                return Err(bad("participant property accessor bound"));
            }
            result.push((
                Identity::from(&node.site),
                property(&mut graph, &node.site)?,
            ));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
