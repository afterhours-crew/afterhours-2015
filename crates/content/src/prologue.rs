// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored first-drive inputs. Live identities, ownership and sequence execution
//! remain caller state; this reader never selects a channel by global asset name.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Node, Site, Source, scenes::scene_roots},
};
use nfs_frostbite::ebx::Value;
use serde_json::{Value as Json, json};
mod arrival;
pub mod composite;
pub mod movies;
pub mod sequences;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Links {
    pub first_drive: String,
    pub item_definition: u32,
    pub teleport_location: u32,
    pub teleport_locator: u32,
}
fn bad(what: &str) -> Error {
    Error::Content(format!("prologue start: {what}"))
}
impl Links {
    pub fn from_json(v: &Json) -> Result<Self, Error> {
        let fields = [
            "first_drive",
            "item_definition",
            "teleport_location",
            "teleport_locator",
        ];
        let o = v
            .as_object()
            .filter(|o| o.len() == fields.len() && fields.iter().all(|k| o.contains_key(*k)))
            .ok_or_else(|| bad("link fields"))?;
        let first_drive = o["first_drive"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 512
                    && !s.starts_with('/')
                    && !s.contains("..")
                    && s.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'/' | b'_')
                    })
            })
            .ok_or_else(|| bad("first-drive path"))?
            .to_owned();
        let mut ports = vec![];
        for field in &fields[1..] {
            let p = o[*field]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n != 0 && !ports.contains(n))
                .ok_or_else(|| bad("link port"))?;
            ports.push(p);
        }
        Ok(Self {
            first_drive,
            item_definition: ports[0],
            teleport_location: ports[1],
            teleport_locator: ports[2],
        })
    }
    pub fn to_json(&self) -> Json {
        json!({"first_drive":self.first_drive,"item_definition":self.item_definition,"teleport_location":self.teleport_location,"teleport_locator":self.teleport_locator})
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    /// Right, up and forward axes in the native transform convention.
    pub basis: [[f32; 3]; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub objective: u64,
    pub vehicle_item: [u8; 16],
    pub opening_blueprint: String,
    pub opening_guid: [u8; 16],
    pub teleport: Pose,
}

/// A story capability with one arrival path. Ambiguous conditional arrivals
/// require selecting the reached branch before this reader can be used.
#[derive(Clone, Debug, PartialEq)]
pub struct Destination {
    pub objective: u64,
    pub opening_blueprint: String,
    pub opening_guid: [u8; 16],
    pub teleport: Pose,
    pub reference: crate::scoring::events::Identity,
}
fn unique<'a>(nodes: &'a [Node], class: &str) -> Result<&'a Site, Error> {
    let mut found = nodes.iter().filter(|n| n.object.class == class);
    let site = &found
        .next()
        .ok_or_else(|| bad(&format!("missing {class}")))?
        .site;
    if found.next().is_some() {
        return Err(bad(&format!("ambiguous {class}")));
    }
    Ok(site)
}
fn endpoint<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    class: &str,
) -> Result<Site, Error> {
    let mut ends = graph.links(site, port)?;
    if ends.len() != 1 || ends[0].port != 0 || graph.object(&ends[0].site)?.class != class {
        return Err(bad(&format!("expected one direct {class}")));
    }
    Ok(ends.remove(0).site)
}
fn exported<S: Source>(
    graph: &mut Graph<S>,
    key: &crate::graph::Key,
    class: &str,
) -> Result<[u8; 16], Error> {
    let o = graph.source.object(key)?;
    if o.class != class {
        return Err(bad(&format!("expected {class}, found {}", o.class)));
    }
    o.guid
        .filter(|g| *g != [0; 16])
        .ok_or_else(|| bad("export GUID"))
}
fn read_destination<S: Source>(
    graph: &mut Graph<S>,
    links: &Links,
    progression: &crate::progression::Rules,
    scene: Site,
    conditional: bool,
) -> Result<(Destination, Vec<Node>), Error> {
    Links::from_json(&links.to_json())?;
    let progression = crate::progression::Rules::from_json(&progression.to_json())?;
    let branch = graph.read(Site::root(&progression.intro))?;
    let mut starts = branch
        .iter()
        .filter(|n| n.site.key == (links.first_drive.clone(), 0));
    let first = starts
        .next()
        .ok_or_else(|| bad("first-drive occurrence"))?
        .site
        .clone();
    if graph.object(&first)?.class != "LogicPrefabBlueprint" {
        return Err(bad("first-drive capability class"));
    }
    if starts.next().is_some() {
        return Err(bad("multiple first-drive occurrences"));
    }
    let nodes = branch
        .iter()
        .filter(|n| n.site.bus == first.bus && n.site.parents == first.parents)
        .cloned()
        .collect::<Vec<_>>();
    let (referer, setter) = if conditional {
        arrival::before_launcher(graph, &nodes)?
    } else {
        (
            unique(&nodes, "StateSequenceRefererEntityData")?.clone(),
            unique(&nodes, "SetTeleportLocationEntityData")?.clone(),
        )
    };
    let sequence = graph.target_field(&referer, "Sequence")?;
    let opening_guid = exported(graph, &sequence, "ParticipantStateSequenceBlueprint")?;
    let stable = endpoint(
        graph,
        unique(&nodes, "RaceObjectiveEntityData")?,
        progression.ports["stable_id"],
        "StableUniqueIdEntityData",
    )?;
    let stable = graph.object(&stable)?;
    let objective = match stable.fields.get("StableUniqueId64") {
        Some(Value::UInt64(n)) => *n,
        Some(Value::Int64(n)) => *n as u64,
        _ => return Err(bad("objective identity")),
    };
    if objective == 0 {
        return Err(bad("zero objective identity"));
    }

    let scene_nodes = graph.read(scene)?;
    let mut spawners = vec![];
    for n in scene_nodes
        .iter()
        .filter(|n| n.object.class == "ParticipantLogicSpawnerEntityData")
    {
        if graph.target_field(&n.site, "ParticipantLogicBlueprint")?
            == (progression.intro.clone(), 0)
        {
            spawners.push(n.site.clone());
        }
    }
    if spawners.len() != 1 {
        return Err(bad("intro spawner occurrence"));
    }
    let refs = graph.links(
        unique(&branch, "ParticipantLogicEntityData")?,
        progression.ports["root_channels"],
    )?;
    let targets = graph.links(&spawners[0], progression.ports["spawner_channels"])?;
    let bindings = crate::participant_logic::bind(graph, &branch, refs, targets)?;
    let mut channel = graph.links(&setter, links.teleport_location)?;
    if channel.len() != 1 {
        return Err(bad("teleport channel"));
    }
    let channel = channel.remove(0);
    let target = bindings
        .get(&channel.site)
        .ok_or_else(|| bad("unbound teleport channel"))?;
    let location = endpoint(graph, target, channel.port, "TeleportLocationEntityData")?;
    let locator = endpoint(
        graph,
        &location,
        links.teleport_locator,
        "LocatorEntityData",
    )?;
    let pose = crate::events::spatial::location(graph, &locator)?;
    Ok((
        Destination {
            objective,
            opening_blueprint: sequence.0,
            opening_guid,
            teleport: Pose {
                position: pose.position,
                basis: pose.basis,
            },
            reference: crate::scoring::events::Identity::from(&referer),
        },
        nodes,
    ))
}

fn read<S: Source>(
    graph: &mut Graph<S>,
    links: &Links,
    progression: &crate::progression::Rules,
    scene: Site,
) -> Result<Start, Error> {
    let (destination, nodes) = read_destination(graph, links, progression, scene, false)?;
    let item = endpoint(
        graph,
        unique(&nodes, "ItemSelectorEntityData")?,
        links.item_definition,
        "ItemEntityData",
    )?;
    let vehicle = graph.target_field(&item, "Item")?;
    let vehicle_item = exported(graph, &vehicle, "RaceVehicleItemData")?;
    Ok(Start {
        objective: destination.objective,
        vehicle_item,
        opening_blueprint: destination.opening_blueprint,
        opening_guid: destination.opening_guid,
        teleport: destination.teleport,
    })
}

/// Read only the selected intro instance and its explicitly bound scene channels.
pub fn extract(index: &mut AssetIndex) -> Result<Start, Error> {
    let links = index.identity.prologue_links.clone();
    let progression = index.identity.progression.clone();
    let rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&rules.level)?;
    let mut graph = Graph::new(&mut index.assets);
    let roots = scene_roots(&mut graph, &rules.level, &startup)?;
    let root = roots
        .get(&progression.scene)
        .ok_or_else(|| bad("progression scene is not loaded"))?
        .clone();
    read(&mut graph, &links, &progression, root)
}

/// Read a selected capability through its occurrence in the intro participant
/// and that participant's bound scene, including the exact arrival referer.
pub fn destination(index: &mut AssetIndex, capability: &str) -> Result<Destination, Error> {
    destination_path(index, capability, false)
}
/// Resolve the not-yet-arrived branch of a Boolean-gated story launcher.
/// The caller owns the latch and must run this arrival before starting its mode.
pub fn destination_before_launcher(
    index: &mut AssetIndex,
    capability: &str,
) -> Result<Destination, Error> {
    destination_path(index, capability, true)
}
fn destination_path(
    index: &mut AssetIndex,
    capability: &str,
    conditional: bool,
) -> Result<Destination, Error> {
    let mut links = index.identity.prologue_links.clone();
    links.first_drive = capability.into();
    Links::from_json(&links.to_json())?;
    let progression = index.identity.progression.clone();
    let rules = index.identity.scenes.clone();
    let startup = index.startup_entries(&rules.level)?;
    let mut graph = Graph::new(&mut index.assets);
    let roots = scene_roots(&mut graph, &rules.level, &startup)?;
    let root = roots
        .get(&progression.scene)
        .ok_or_else(|| bad("progression scene is not loaded"))?
        .clone();
    Ok(read_destination(&mut graph, &links, &progression, root, conditional)?.0)
}

#[cfg(test)]
mod tests;
