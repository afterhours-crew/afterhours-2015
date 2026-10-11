// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed participant-logic composition and contextual channel bindings.
//! Values, lifecycle decisions and live object identities belong to the caller.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{
        scenes::{SceneLookup, scene_roots},
        *,
    },
    world::StartupKind,
};
use nfs_frostbite::ebx::Value;
use nfs_world::replication::entity::{Kind, Profile, creation::Asset};
use std::collections::{BTreeMap, BTreeSet};

mod rules;
pub use rules::Rules;

/// An explicit owner context, rather than a global search by channel asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Owner {
    Scene,
    /// A child spawned by a participant-owned manager binds its parent's channels.
    Participant {
        blueprint: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Channel {
    Scene { content_key: u32, serializer: usize },
    Participant { serializer: usize },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub kind: Kind,
    pub channel: Option<Channel>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub asset: Asset,
    pub catalog: Vec<(u32, u32)>,
    pub scene_content_key: u32,
    pub owner: Owner,
    pub bus_count: u16,
    pub fields: Vec<Field>,
}
impl Layout {
    pub fn profile(&self) -> Result<Profile, Error> {
        Profile::new(&self.fields.iter().map(|f| f.kind).collect::<Vec<_>>())
            .map_err(|_| bad("participant serializer profile"))
    }
}

fn shape(nodes: &[Node], rules: &Rules) -> Result<(u16, Vec<(Site, Kind)>), Error> {
    if nodes
        .first()
        .is_none_or(|n| n.object.class != "ParticipantLogicBlueprint")
    {
        return Err(bad("participant blueprint class"));
    }
    let mut buses = 0u16;
    let mut fields = Vec::new();
    for node in nodes {
        let kind = rules.classes.get(&node.object.class).ok_or_else(|| {
            bad(&format!(
                "unsupported participant class {}",
                node.object.class
            ))
        })?;
        if node.object.class.ends_with("Blueprint") {
            let flags = match node.object.fields.get("Flags") {
                Some(Value::UInt16(v)) => u32::from(*v),
                Some(v) => bits32(v)?,
                None => return Err(bad("participant bus flags")),
            };
            if flags & 1 != 0 {
                buses = buses
                    .checked_add(1)
                    .ok_or_else(|| bad("participant bus count"))?;
            }
        }
        if let Some(kind) = kind {
            fields.push((node.site.clone(), *kind));
        }
    }
    if buses == 0
        || fields.first().is_none_or(|(_, k)| *k != Kind::Root)
        || fields.iter().filter(|(_, k)| *k == Kind::Root).count() != 1
    {
        return Err(bad("participant root or bus count"));
    }
    Profile::new(&fields.iter().map(|(_, k)| *k).collect::<Vec<_>>())
        .map_err(|_| bad("participant serializer count"))?;
    Ok((buses, fields))
}

fn channel<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<Key, Error> {
    graph.target_field(site, "ChannelAsset")
}

/// Bind in declared connection order. Repeated channel assets consume separate
/// reference occurrences, and an unrelated compatible channel is not a candidate.
pub(crate) fn bind<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    references: Vec<Endpoint>,
    targets: Vec<Endpoint>,
) -> Result<BTreeMap<Site, Site>, Error> {
    let mut refs = Vec::new();
    for endpoint in references {
        if endpoint.port != 0
            || graph.object(&endpoint.site)?.class != "LinkedSchematicChannelReferenceEntityData"
        {
            return Err(bad("participant root channel endpoint"));
        }
        refs.push((channel(graph, &endpoint.site)?, endpoint.site));
    }
    let mut bound = BTreeMap::new();
    for endpoint in targets {
        if endpoint.port != 0
            || graph.object(&endpoint.site)?.class != "LinkedSchematicChannelEntityData"
        {
            return Err(bad("participant spawner channel endpoint"));
        }
        let asset = channel(graph, &endpoint.site)?;
        let reference = refs
            .iter()
            .find(|(a, site)| *a == asset && !bound.contains_key(site))
            .ok_or_else(|| bad("unmatched participant channel"))?;
        bound.insert(reference.1.clone(), endpoint.site);
    }
    let all: BTreeSet<_> = nodes
        .iter()
        .filter(|n| n.object.class == "LinkedSchematicChannelReferenceEntityData")
        .map(|n| n.site.clone())
        .collect();
    if bound.keys().cloned().collect::<BTreeSet<_>>() != all || bound.len() != refs.len() {
        return Err(bad("unbound or duplicate participant reference"));
    }
    Ok(bound)
}

impl AssetIndex {
    /// Resolve a registered blueprint through its scene spawner or explicit
    /// participant parent. No values, phases, event decisions or Ghost IDs are
    /// inferred from the layout. Ambiguous spawners and bindings are rejected.
    pub fn participant_layout(
        &mut self,
        scene: &str,
        blueprint: &str,
        owner: Owner,
    ) -> Result<Layout, Error> {
        let scene = scene.to_lowercase();
        let blueprint = blueprint.to_lowercase();
        for name in [&scene, &blueprint].into_iter().chain(match &owner {
            Owner::Scene => None,
            Owner::Participant { blueprint } => Some(blueprint),
        }) {
            if name.is_empty() || name.len() > 512 {
                return Err(bad("participant asset name bound"));
            }
        }
        let rules = Rules::from_json(&self.identity.participant_logic.to_json())?;
        let scene_rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&scene_rules.level)?;
        let bundle = startup
            .iter()
            .position(|e| e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&scene))
            .and_then(|i| u16::try_from(i + 1).ok())
            .ok_or_else(|| bad("participant bundle is not loaded"))?;
        let reference = self.reference(&scene, &blueprint)?;
        let asset = Asset {
            bundle,
            type_id: u16::try_from(reference.class_id).map_err(|_| bad("participant class ID"))?,
            local_index: u16::try_from(reference.local_index)
                .map_err(|_| bad("participant asset ordinal"))?,
        };
        let catalog = self.catalog(&scene)?;
        if let Owner::Participant { blueprint: parent } = &owner {
            self.reference(&scene, parent)?;
        }
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(&blueprint))?;
        let (bus_count, fields) = shape(&nodes, &rules)?;
        let references = graph.links(&fields[0].0, rules.root_channels)?;
        let bindings: BTreeMap<Site, Channel> = match &owner {
            Owner::Scene => {
                let mut scenes = SceneLookup {
                    roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
                    rules: &scene_rules,
                    endpoints: BTreeMap::new(),
                    loaded: BTreeSet::new(),
                };
                let scene_nodes = graph.read(
                    scenes
                        .roots
                        .get(&scene)
                        .ok_or_else(|| bad("participant scene missing"))?
                        .clone(),
                )?;
                let mut spawners = Vec::new();
                for node in &scene_nodes {
                    if rules.spawners.contains(&node.object.class)
                        && graph.target_field(&node.site, "ParticipantLogicBlueprint")?
                            == (blueprint.clone(), 0)
                    {
                        spawners.push(node.site.clone());
                    }
                }
                if spawners.len() != 1 {
                    return Err(bad("missing or ambiguous participant spawner"));
                }
                let targets = graph.links(&spawners[0], rules.spawner_channels)?;
                bind(&mut graph, &nodes, references, targets)?
                    .into_iter()
                    .map(|(source, target)| {
                        let (content_key, serializer) = scenes.endpoint(&mut graph, &target)?;
                        Ok((
                            source,
                            Channel::Scene {
                                content_key,
                                serializer,
                            },
                        ))
                    })
                    .collect::<Result<_, Error>>()?
            }
            Owner::Participant { blueprint: parent } => {
                if parent.eq_ignore_ascii_case(&blueprint) {
                    return Err(bad("participant cannot own itself"));
                }
                let parents = graph.read(Site::root(parent))?;
                let (_, fields) = shape(&parents, &rules)?;
                let mut targets = Vec::new();
                for reference in &references {
                    let wanted = channel(&mut graph, &reference.site)?;
                    let mut found = Vec::new();
                    for node in parents
                        .iter()
                        .filter(|n| n.object.class == "LinkedSchematicChannelEntityData")
                    {
                        if channel(&mut graph, &node.site)? == wanted {
                            found.push(node.site.clone());
                        }
                    }
                    if found.len() != 1 {
                        return Err(bad("missing or ambiguous parent channel"));
                    }
                    targets.push(Endpoint {
                        site: found.remove(0),
                        port: 0,
                    });
                }
                bind(&mut graph, &nodes, references, targets)?
                    .into_iter()
                    .map(|(source, target)| {
                        let serializer = fields
                            .iter()
                            .position(|(site, kind)| *site == target && *kind == Kind::Rpc)
                            .ok_or_else(|| bad("parent channel is not a serializer"))?;
                        Ok((source, Channel::Participant { serializer }))
                    })
                    .collect::<Result<_, Error>>()?
            }
        };
        let fields = fields
            .into_iter()
            .map(|(site, kind)| {
                let channel = bindings.get(&site).cloned();
                if (kind == Kind::RpcReference) != channel.is_some() {
                    return Err(bad("participant channel layout"));
                }
                Ok(Field { kind, channel })
            })
            .collect::<Result<_, Error>>()?;
        let layout = Layout {
            asset,
            catalog,
            scene_content_key: crate::assets::scene_key(&scene),
            owner,
            bus_count,
            fields,
        };
        layout.profile()?;
        Ok(layout)
    }
}

#[cfg(test)]
mod tests;
