// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scalar participant properties named by accessors (#138, E1000/E1023). An
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
const PARTICIPANT_PROVIDER: u32 = 3_062_566_331;

pub(crate) fn event_providers<S: Source>(
    source: &mut S,
    blueprint: &str,
) -> Result<Vec<(Identity, Option<Identity>)>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if !matches!(
            node.object.class.as_str(),
            "ParticipantProviderToParticipantEventEntityData"
                | "ParticipantProviderEventFilterEntityData"
                | "ParticipantSetAccessorEntityData"
        ) {
            continue;
        }
        let links = graph.links(&node.site, PARTICIPANT_PROVIDER)?;
        let provider = match links.as_slice() {
            [] => None,
            [link] if link.port == 0 => Some(Identity::from(&link.site)),
            _ => return Err(bad("participant event provider links")),
        };
        result.push((Identity::from(&node.site), provider));
    }
    Ok(result)
}
pub(crate) fn sets<S: Source>(
    source: &mut S,
    blueprint: &str,
) -> Result<Vec<(Identity, Option<Property>)>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "ParticipantSetAccessorEntityData" {
            continue;
        }
        let key = if graph.links(&node.site, 772_610_520)?.is_empty() {
            None
        } else {
            Some(identifier(&mut graph, &node.site, 772_610_520)?)
        };
        result.push((Identity::from(&node.site), key));
    }
    Ok(result)
}
pub(crate) fn accessor(class: &str) -> Option<(u32, nfs_world::scoring::properties::Kind)> {
    use nfs_world::scoring::properties::Kind;
    match class {
        ACCESSOR => Some((IDENTIFIER, Kind::Integer)),
        "FloatParticipantPropertyAccessorEntityData" => Some((4_068_038_185, Kind::Float)),
        "BoolParticipantPropertyAccessorEntityData" => Some((2_526_680_631, Kind::Boolean)),
        _ => None,
    }
}
/// Accessor occurrences one blueprint may hold (local bound).
pub const MAX_ACCESSORS: usize = 256;

/// One property: a port of a schematic channel asset.
pub use nfs_world::scoring::participant_properties::Property;

/// The property an accessor names: its single identifier link to a channel
/// reference port, which the referenced channel asset declares.
pub(crate) fn property<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<Property, Error> {
    if graph.object(site)?.class != ACCESSOR {
        return Err(bad("participant property accessor class"));
    }
    identifier(graph, site, IDENTIFIER)
}

pub(crate) fn identifier<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<Property, Error> {
    let mut rows = graph.links(site, port)?;
    if rows.len() != 1 {
        return Err(bad("participant property identifier link"));
    }
    channel_endpoint(graph, &rows.remove(0))
}

pub(crate) fn channel_endpoint<S: Source>(
    graph: &mut Graph<S>,
    endpoint: &crate::graph::Endpoint,
) -> Result<Property, Error> {
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
        read(&mut self.assets, blueprint, false)
    }

    /// Accessors whose provider is the enclosing participant logic's own
    /// participant. Other provider kinds require an explicit host binding.
    pub fn owned_integer_participant_properties(
        &mut self,
        blueprint: &str,
    ) -> Result<Vec<(Identity, Property)>, Error> {
        read(&mut self.assets, blueprint, true)
    }

    /// All scalar accessors bound to the enclosing participant logic.
    pub fn owned_participant_properties(
        &mut self,
        blueprint: &str,
    ) -> Result<Vec<(Identity, Property)>, Error> {
        read_kinds(&mut self.assets, blueprint, true, true)
    }
}

fn read<S: Source>(
    source: &mut S,
    blueprint: &str,
    own_participant: bool,
) -> Result<Vec<(Identity, Property)>, Error> {
    read_kinds(source, blueprint, own_participant, false)
}

fn read_kinds<S: Source>(
    source: &mut S,
    blueprint: &str,
    own_participant: bool,
    all_kinds: bool,
) -> Result<Vec<(Identity, Property)>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("participant property blueprint name"));
    }
    let mut graph = Graph::new(source);
    let nodes = graph.read(Site::root(blueprint))?;
    let mut result = Vec::new();
    for node in nodes.iter().filter(|n| {
        n.object.class == ACCESSOR
            || (all_kinds
                && (accessor(&n.object.class).is_some()
                    || n.object.class == "SortAndRankIntParticipantPropertyEntityData"))
    }) {
        if result.len() >= MAX_ACCESSORS {
            return Err(bad("participant property accessor bound"));
        }
        if own_participant {
            let providers = graph.links(&node.site, PARTICIPANT_PROVIDER)?;
            let [provider] = providers.as_slice() else {
                return Err(bad("accessor participant provider count"));
            };
            if provider.port != 0
                || graph.object(&provider.site)?.class
                    != "ParticipantLogicParticipantProviderEntityData"
            {
                return Err(bad("accessor is not bound to its own participant"));
            }
        }
        result.push((
            Identity::from(&node.site),
            identifier(
                &mut graph,
                &node.site,
                if node.object.class == "SortAndRankIntParticipantPropertyEntityData" {
                    233_976_021
                } else {
                    accessor(&node.object.class).unwrap().0
                },
            )?,
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
